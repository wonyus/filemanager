pub mod path_mapping;
pub mod recursive;
pub mod retry;
pub mod scheduler;
pub mod settings;
pub mod state;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::{
    domain::error::{AppError, PublicError},
    dto::transfer::{
        ClearTransferHistoryRequest, ListTransfersRequest, StartTransferRequest,
        TransferChannelMessage, TransferDetails, TransferEndpoint, TransferHistoryPage,
        TransferItem, TransferJob, TransferOperation, TransferProgress, TransferResult,
        TransferStatus, TransferSummary, UploadMetadata, DTO_SCHEMA_VERSION,
    },
    infrastructure::database::Database,
};

use self::retry::RetryPolicy;
use self::scheduler::{JobPriority, ScheduledJob, SchedulerConfig, TransferScheduler};

#[derive(Clone)]
struct RuntimeJob {
    job: TransferJob,
    request: StartTransferRequest,
    cancel_requested: bool,
    pause_requested: bool,
    last_progress_at: Option<Instant>,
    last_progress_bytes: u64,
    ewma_speed_bps: Option<f64>,
}

/// Session-scoped transfer coordinator. Provider-specific workers are layered
/// on top of this manager; all commands still go through this state machine.
#[derive(Clone)]
pub struct TransferManager {
    jobs: Arc<RwLock<HashMap<Uuid, RuntimeJob>>>,
    events: broadcast::Sender<TransferChannelMessage>,
    scheduler: TransferScheduler,
    retry_policy: RetryPolicy,
    database: Option<Database>,
}

impl Default for TransferManager {
    fn default() -> Self {
        Self::new(4)
    }
}

impl TransferManager {
    pub fn new(max_concurrent_jobs: usize) -> Self {
        Self::build(max_concurrent_jobs, None)
    }

    /// Construct a manager backed by the local SQLite history database.
    /// Keeping this constructor separate preserves lightweight unit tests and
    /// callers that intentionally use an in-memory session manager.
    pub fn new_with_database(max_concurrent_jobs: usize, database: Database) -> Self {
        Self::build(max_concurrent_jobs, Some(database))
    }

    fn build(max_concurrent_jobs: usize, database: Option<Database>) -> Self {
        let (events, _) = broadcast::channel(256);
        let max_concurrent_jobs = max_concurrent_jobs.clamp(1, 16);
        let scheduler = TransferScheduler::new(SchedulerConfig {
            max_concurrent_jobs,
            ..SchedulerConfig::default()
        });
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            events,
            scheduler,
            retry_policy: RetryPolicy::default(),
            database,
        }
    }

    async fn persist_snapshot(&self, job: &TransferJob, request: &StartTransferRequest) {
        let Some(database) = self.database.as_ref() else {
            return;
        };
        if let Err(error) = database.save_transfer(job, request).await {
            // Transfer execution remains session-safe if the history disk is
            // temporarily unavailable.  The warning is actionable in the
            // diagnostics log and avoids leaking a provider credential.
            tracing::warn!(transfer_id = %job.id, error = %error, "unable to persist transfer snapshot");
        }
        if job.status.is_terminal() {
            let (retention_days, max_jobs) = request
                .settings_snapshot
                .as_ref()
                .map(|settings| {
                    (
                        u32::from(settings.transfer_history_days),
                        settings.transfer_history_max_jobs,
                    )
                })
                .unwrap_or((30, 1_000));
            if let Err(error) = database
                .prune_transfer_history(retention_days, max_jobs)
                .await
            {
                tracing::warn!(error = %error, "unable to prune transfer history");
            }
        }
    }

    /// Persist the provider multipart handle before any part is uploaded.
    /// Managers without an attached database remain usable for isolated unit
    /// tests and intentionally treat this as a no-op.
    pub async fn persist_multipart_upload(
        &self,
        transfer_id: Uuid,
        profile_id: Option<&str>,
        bucket: &str,
        object_key: &str,
        upload_id: &str,
        part_size: u64,
    ) -> Result<(), AppError> {
        match self.database.as_ref() {
            Some(database) => {
                database
                    .create_multipart_upload(
                        transfer_id,
                        profile_id,
                        bucket,
                        object_key,
                        upload_id,
                        part_size,
                    )
                    .await
            }
            None => Ok(()),
        }
    }

    pub async fn persist_multipart_part(
        &self,
        transfer_id: Uuid,
        part_number: u32,
        etag: &str,
        size_bytes: u64,
    ) -> Result<(), AppError> {
        match self.database.as_ref() {
            Some(database) => {
                database
                    .record_multipart_part(transfer_id, part_number, etag, size_bytes)
                    .await
            }
            None => Ok(()),
        }
    }

    pub async fn clear_multipart_upload(&self, transfer_id: Uuid) -> Result<(), AppError> {
        if let Some(database) = self.database.as_ref() {
            database.clear_multipart_upload(transfer_id).await?;
        }
        Ok(())
    }

    /// Recover durable jobs after opening the app.  Active jobs are first
    /// marked Interrupted in one database update; no provider request is
    /// resumed automatically.  Interrupted rows remain retryable because the
    /// sanitized request is hydrated into the session manager.
    pub async fn recover_from_database(&self) -> Result<usize, AppError> {
        let Some(database) = self.database.as_ref() else {
            return Ok(0);
        };
        let marked = database.mark_active_transfers_interrupted().await?;
        let persisted = database.list_transfers().await?;
        let mut jobs = self.jobs.write().await;
        for transfer in persisted {
            jobs.entry(transfer.job.id).or_insert_with(|| RuntimeJob {
                cancel_requested: false,
                pause_requested: false,
                last_progress_at: None,
                last_progress_bytes: 0,
                ewma_speed_bps: None,
                job: transfer.job,
                request: transfer.request,
            });
        }
        Ok(marked as usize)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TransferChannelMessage> {
        self.events.subscribe()
    }

    pub fn max_concurrent_jobs(&self) -> usize {
        self.scheduler.max_concurrent_jobs()
    }

    pub fn set_max_concurrent_jobs(&self, max_concurrent_jobs: usize) -> usize {
        self.scheduler.set_max_concurrent_jobs(max_concurrent_jobs)
    }

    pub fn scheduler(&self) -> TransferScheduler {
        self.scheduler.clone()
    }

    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry_policy
    }

    pub async fn create(&self, request: StartTransferRequest) -> Result<TransferJob, AppError> {
        validate_request(&request)?;
        if request.schema_version != DTO_SCHEMA_VERSION {
            return Err(AppError::Validation(format!(
                "unsupported transfer schema version: {}",
                request.schema_version
            )));
        }
        let now = Utc::now().to_rfc3339();
        let job = TransferJob {
            schema_version: DTO_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            operation: request.operation,
            profile_id: request.profile_id.clone(),
            source: request.source.clone(),
            destination: request.destination.clone(),
            status: TransferStatus::Queued,
            collision_policy: request.collision_policy,
            total_bytes: request.total_bytes,
            transferred_bytes: 0,
            total_items: request.total_items,
            completed_items: 0,
            failed_items: 0,
            speed_bps: None,
            eta_seconds: None,
            retry_count: 0,
            created_at: now,
            started_at: None,
            finished_at: None,
            error: None,
        };
        self.jobs.write().await.insert(
            job.id,
            RuntimeJob {
                job: job.clone(),
                request: request.clone(),
                cancel_requested: false,
                pause_requested: false,
                last_progress_at: None,
                last_progress_bytes: 0,
                ewma_speed_bps: None,
            },
        );
        if let Err(error) = self
            .scheduler
            .enqueue(job.id, priority_for_operation(job.operation))
            .await
        {
            self.jobs.write().await.remove(&job.id);
            return Err(AppError::Validation(format!(
                "unable to queue transfer: {error:?}"
            )));
        }
        self.persist_snapshot(&job, &request).await;
        let _ = self
            .events
            .send(TransferChannelMessage::Snapshot(job.clone()));
        Ok(job)
    }

    /// Claim the next queued job.  The returned permit must be held by the
    /// worker for the lifetime of the remote operation; dropping it releases a
    /// global concurrency slot and wakes another job.
    pub async fn acquire_next(&self) -> Option<ScheduledJob> {
        let permit = self.scheduler.acquire_next().await?;
        let _ = self.transition(permit.id, TransferStatus::Planning).await;
        Some(permit)
    }

    /// Starts one explicitly requested job while holding a global scheduler
    /// permit. The caller must retain the returned lease for the complete
    /// worker lifetime; dropping it releases the concurrency slot.
    pub async fn begin(&self, id: Uuid) -> Result<(TransferJob, ScheduledJob), AppError> {
        let permit = self.scheduler.acquire_specific(id).await.map_err(|error| {
            AppError::TransferStateConflict(format!("unable to acquire transfer slot: {error:?}"))
        })?;
        if let Err(error) = self.transition(id, TransferStatus::Planning).await {
            drop(permit);
            return Err(error);
        }
        match self.transition(id, TransferStatus::Running).await {
            Ok(job) => Ok((job, permit)),
            Err(error) => {
                drop(permit);
                Err(error)
            }
        }
    }

    pub async fn get(&self, id: Uuid) -> Option<TransferJob> {
        self.jobs
            .read()
            .await
            .get(&id)
            .map(|runtime| runtime.job.clone())
    }

    pub async fn request(&self, id: Uuid) -> Option<StartTransferRequest> {
        self.jobs
            .read()
            .await
            .get(&id)
            .map(|runtime| runtime.request.clone())
    }

    pub async fn has_active_for_profile(&self, profile_id: &str) -> bool {
        self.jobs.read().await.values().any(|runtime| {
            runtime.job.status.is_active() && runtime.job.profile_id.as_deref() == Some(profile_id)
        })
    }

    pub async fn active_count(&self) -> usize {
        self.jobs
            .read()
            .await
            .values()
            .filter(|runtime| runtime.job.status.is_active())
            .count()
    }

    pub async fn list(&self, include_active: bool) -> Vec<TransferSummary> {
        let mut values = self
            .jobs
            .read()
            .await
            .values()
            .filter(|runtime| include_active || !runtime.job.status.is_active())
            .map(|runtime| TransferSummary {
                schema_version: DTO_SCHEMA_VERSION,
                id: runtime.job.id,
                operation: runtime.job.operation,
                status: runtime.job.status,
                transferred_bytes: runtime.job.transferred_bytes,
                total_bytes: runtime.job.total_bytes,
                completed_items: runtime.job.completed_items,
                total_items: runtime.job.total_items,
                failed_items: runtime.job.failed_items,
                speed_bps: runtime.job.speed_bps,
                eta_seconds: runtime.job.eta_seconds,
                created_at: runtime.job.created_at.clone(),
                finished_at: runtime.job.finished_at.clone(),
            })
            .collect::<Vec<_>>();
        values.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        values
    }

    pub async fn list_page(
        &self,
        request: ListTransfersRequest,
    ) -> Result<TransferHistoryPage, AppError> {
        if request.schema_version != DTO_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported transfer schema version".to_string(),
            ));
        }
        let mut summaries = self.list(request.include_active).await;
        if let Some(profile_id) = request.profile_id.as_deref() {
            let jobs = self.jobs.read().await;
            summaries.retain(|summary| {
                jobs.get(&summary.id)
                    .and_then(|runtime| runtime.job.profile_id.as_deref())
                    == Some(profile_id)
            });
        }
        let total = summaries.len() as u64;
        let limit = request.limit.clamp(1, 500);
        let offset = request.offset.min(total as u32);
        let items = summaries
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        Ok(TransferHistoryPage {
            schema_version: DTO_SCHEMA_VERSION,
            items,
            total,
            limit,
            offset,
        })
    }

    pub async fn details(&self, id: Uuid) -> Result<TransferDetails, AppError> {
        let job = self
            .get(id)
            .await
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        let items = match self.database.as_ref() {
            Some(database) => database.list_transfer_items(id).await?,
            None => Vec::new(),
        };
        Ok(TransferDetails {
            schema_version: DTO_SCHEMA_VERSION,
            job,
            items,
        })
    }

    pub async fn replace_transfer_items(
        &self,
        transfer_id: Uuid,
        items: &[TransferItem],
    ) -> Result<(), AppError> {
        if let Some(database) = self.database.as_ref() {
            database.replace_transfer_items(transfer_id, items).await?;
        }
        Ok(())
    }

    pub async fn update_transfer_item(
        &self,
        transfer_id: Uuid,
        item_id: &str,
        status: TransferStatus,
        bytes_completed: u64,
        error: Option<&PublicError>,
        cleanup_required: bool,
    ) -> Result<(), AppError> {
        if let Some(database) = self.database.as_ref() {
            database
                .update_transfer_item(
                    transfer_id,
                    item_id,
                    status,
                    bytes_completed,
                    error,
                    cleanup_required,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn clear_history(
        &self,
        request: ClearTransferHistoryRequest,
    ) -> Result<usize, AppError> {
        if request.schema_version != DTO_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported transfer schema version".to_string(),
            ));
        }
        let before = request.before.as_deref();
        let ids = self
            .jobs
            .read()
            .await
            .iter()
            .filter(|(_, runtime)| {
                if !runtime.job.status.is_terminal() {
                    return false;
                }
                if !request.include_failed
                    && matches!(
                        runtime.job.status,
                        TransferStatus::Failed | TransferStatus::CompletedWithWarnings
                    )
                {
                    return false;
                }
                before
                    .map(|value| runtime.job.finished_at.as_deref().unwrap_or_default() < value)
                    .unwrap_or(true)
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        if let Some(database) = self.database.as_ref() {
            database.delete_transfers(&ids).await?;
        }
        let count = ids.len();
        let mut jobs = self.jobs.write().await;
        for id in ids {
            jobs.remove(&id);
        }
        Ok(count)
    }

    pub async fn retry(&self, id: Uuid) -> Result<TransferJob, AppError> {
        let (request, old_retry_count, status) = {
            let jobs = self.jobs.read().await;
            let runtime = jobs
                .get(&id)
                .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
            (
                runtime.request.clone(),
                runtime.job.retry_count,
                runtime.job.status,
            )
        };
        if !status.is_terminal() {
            return Err(AppError::Validation(
                "only finished transfers can be retried".to_string(),
            ));
        }
        let mut next = self.create(request.clone()).await?;
        let (next_job, next_request) = {
            let mut jobs = self.jobs.write().await;
            if let Some(runtime) = jobs.get_mut(&next.id) {
                runtime.job.retry_count = old_retry_count.saturating_add(1);
                next = runtime.job.clone();
                (runtime.job.clone(), runtime.request.clone())
            } else {
                (next.clone(), request)
            }
        };
        self.persist_snapshot(&next_job, &next_request).await;
        Ok(next)
    }

    pub async fn transition(
        &self,
        id: Uuid,
        next: TransferStatus,
    ) -> Result<TransferJob, AppError> {
        let mut jobs = self.jobs.write().await;
        let runtime = jobs
            .get_mut(&id)
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        let current = runtime.job.status;
        runtime.job.status = state::transition(current, next).map_err(AppError::Validation)?;
        if next == TransferStatus::Retrying {
            runtime.job.retry_count = runtime.job.retry_count.saturating_add(1);
        }
        if matches!(next, TransferStatus::Running) && runtime.job.started_at.is_none() {
            runtime.job.started_at = Some(Utc::now().to_rfc3339());
        }
        if next.is_terminal() {
            runtime.job.finished_at = Some(Utc::now().to_rfc3339());
        }
        let job = runtime.job.clone();
        let request = runtime.request.clone();
        drop(jobs);
        self.persist_snapshot(&job, &request).await;
        let _ = self.events.send(TransferChannelMessage::StateChanged(next));
        let _ = self
            .events
            .send(TransferChannelMessage::Snapshot(job.clone()));
        Ok(job)
    }

    pub async fn request_pause(&self, id: Uuid) -> Result<TransferJob, AppError> {
        let current = self
            .get(id)
            .await
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        if current.status == TransferStatus::Queued {
            return Err(AppError::Validation(
                "queued transfers cannot be paused".to_string(),
            ));
        }
        let pause_supported = match current.operation {
            TransferOperation::DeleteObjects => self.request(id).await.is_some_and(|request| {
                request.recursive
                    || matches!(
                        &request.source,
                        TransferEndpoint::Remote { key, .. } if key.ends_with('/')
                    )
            }),
            TransferOperation::CreateFolder
            | TransferOperation::UploadFile
            | TransferOperation::CopyObject
            | TransferOperation::MoveObject => false,
            TransferOperation::DownloadFile => true,
            TransferOperation::UploadDirectory
            | TransferOperation::DownloadPrefix
            | TransferOperation::CopyPrefix
            | TransferOperation::MovePrefix => true,
        };
        if !pause_supported {
            return Err(AppError::Validation(
                "pause is not supported for this transfer".to_string(),
            ));
        }
        let job = self.transition(id, TransferStatus::Pausing).await?;
        let mut jobs = self.jobs.write().await;
        if let Some(runtime) = jobs.get_mut(&id) {
            runtime.pause_requested = true;
        }
        Ok(job)
    }

    pub async fn request_resume(&self, id: Uuid) -> Result<TransferJob, AppError> {
        {
            let mut jobs = self.jobs.write().await;
            if let Some(runtime) = jobs.get_mut(&id) {
                runtime.pause_requested = false;
            }
        }
        self.transition(id, TransferStatus::Running).await
    }

    pub async fn request_cancel(&self, id: Uuid) -> Result<TransferJob, AppError> {
        let current = self
            .get(id)
            .await
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        if current.status == TransferStatus::Queued {
            self.scheduler.cancel_queued(id).await;
            let mut jobs = self.jobs.write().await;
            if let Some(runtime) = jobs.get_mut(&id) {
                runtime.cancel_requested = true;
            }
            drop(jobs);
            return self.transition(id, TransferStatus::Cancelled).await;
        }
        {
            let mut jobs = self.jobs.write().await;
            let runtime = jobs
                .get_mut(&id)
                .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
            runtime.cancel_requested = true;
        }
        self.transition(id, TransferStatus::Cancelling).await
    }

    pub async fn complete_cancel(&self, id: Uuid) -> Result<TransferJob, AppError> {
        self.transition(id, TransferStatus::Cancelled).await
    }

    pub async fn is_cancel_requested(&self, id: Uuid) -> bool {
        self.jobs
            .read()
            .await
            .get(&id)
            .map(|runtime| runtime.cancel_requested)
            .unwrap_or(true)
    }

    pub async fn update_progress(
        &self,
        id: Uuid,
        transferred_bytes: u64,
        completed_items: u64,
        speed_bps: Option<u64>,
        eta_seconds: Option<u64>,
    ) -> Result<TransferProgress, AppError> {
        let mut jobs = self.jobs.write().await;
        let runtime = jobs
            .get_mut(&id)
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        if !runtime.job.status.is_active() {
            return Err(AppError::Validation(
                "cannot update a finished transfer".to_string(),
            ));
        }
        let now = Instant::now();
        let sample_speed = runtime.last_progress_at.and_then(|previous| {
            let elapsed = now.duration_since(previous).as_secs_f64();
            if elapsed <= 0.0 {
                None
            } else {
                Some(transferred_bytes.saturating_sub(runtime.last_progress_bytes) as f64 / elapsed)
            }
        });
        if let Some(sample_speed) = sample_speed {
            // Five-second EWMA: alpha approaches one for a long gap and
            // remains smooth for the normal 5 Hz progress cadence.
            let alpha = 1.0
                - (-now
                    .duration_since(runtime.last_progress_at.unwrap_or(now))
                    .as_secs_f64()
                    / 5.0)
                    .exp();
            runtime.ewma_speed_bps = Some(
                runtime
                    .ewma_speed_bps
                    .map(|previous| previous + alpha * (sample_speed - previous))
                    .unwrap_or(sample_speed),
            );
        }
        runtime.last_progress_at = Some(now);
        runtime.last_progress_bytes = transferred_bytes;
        let computed_speed = runtime
            .ewma_speed_bps
            .filter(|value| value.is_finite() && *value >= 1.0)
            .map(|value| value.round() as u64);
        let effective_speed = speed_bps.or(computed_speed);
        let computed_eta = runtime.job.total_bytes.and_then(|total| {
            effective_speed
                .filter(|speed| *speed > 0 && total > transferred_bytes)
                .map(|speed| (total - transferred_bytes).div_ceil(speed))
        });
        let effective_eta = eta_seconds.or(computed_eta);
        runtime.job.transferred_bytes = transferred_bytes;
        runtime.job.completed_items = completed_items;
        runtime.job.speed_bps = effective_speed;
        runtime.job.eta_seconds = effective_eta;
        let job = runtime.job.clone();
        let request = runtime.request.clone();
        let progress = TransferProgress {
            schema_version: DTO_SCHEMA_VERSION,
            transfer_id: id,
            status: runtime.job.status,
            transferred_bytes,
            total_bytes: runtime.job.total_bytes,
            completed_items,
            total_items: runtime.job.total_items,
            speed_bps: effective_speed,
            eta_seconds: effective_eta,
        };
        drop(jobs);
        self.persist_snapshot(&job, &request).await;
        let _ = self
            .events
            .send(TransferChannelMessage::Progress(progress.clone()));
        Ok(progress)
    }

    pub async fn set_totals(
        &self,
        id: Uuid,
        total_items: Option<u64>,
        total_bytes: Option<u64>,
    ) -> Result<TransferJob, AppError> {
        let mut jobs = self.jobs.write().await;
        let runtime = jobs
            .get_mut(&id)
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        if !runtime.job.status.is_active() {
            return Err(AppError::Validation(
                "cannot update totals for a finished transfer".to_string(),
            ));
        }
        runtime.job.total_items = total_items;
        runtime.job.total_bytes = total_bytes;
        let job = runtime.job.clone();
        let request = runtime.request.clone();
        drop(jobs);
        self.persist_snapshot(&job, &request).await;
        let _ = self
            .events
            .send(TransferChannelMessage::Snapshot(job.clone()));
        Ok(job)
    }

    pub async fn set_error(&self, id: Uuid, error: AppError) -> Result<(), AppError> {
        let public_error = PublicError::from(error);
        let mut jobs = self.jobs.write().await;
        let runtime = jobs
            .get_mut(&id)
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        runtime.job.error = Some(public_error);
        let snapshot = runtime.job.clone();
        let request = runtime.request.clone();
        drop(jobs);
        self.persist_snapshot(&snapshot, &request).await;
        let _ = self.events.send(TransferChannelMessage::Snapshot(snapshot));
        Ok(())
    }

    pub async fn finish(
        &self,
        id: Uuid,
        status: TransferStatus,
        failed_items: u64,
        cleanup_required_items: u64,
    ) -> Result<TransferResult, AppError> {
        if !matches!(
            status,
            TransferStatus::Completed
                | TransferStatus::CompletedWithWarnings
                | TransferStatus::Failed
                | TransferStatus::Cancelled
        ) {
            return Err(AppError::Validation(
                "finish requires a terminal transfer state".to_string(),
            ));
        }
        self.transition(id, status).await?;
        let (final_job, final_request) = {
            let mut jobs = self.jobs.write().await;
            if let Some(runtime) = jobs.get_mut(&id) {
                runtime.job.failed_items = failed_items;
                (runtime.job.clone(), runtime.request.clone())
            } else {
                return Err(AppError::Unknown(format!(
                    "transfer disappeared while finishing: {id}"
                )));
            }
        };
        self.persist_snapshot(&final_job, &final_request).await;
        let result = TransferResult {
            schema_version: DTO_SCHEMA_VERSION,
            transfer_id: id,
            status: final_job.status,
            completed_items: final_job.completed_items,
            failed_items,
            cleanup_required_items,
            error: final_job.error.clone(),
        };
        let _ = self
            .events
            .send(TransferChannelMessage::Snapshot(final_job));
        let _ = self
            .events
            .send(TransferChannelMessage::Finished(result.clone()));
        Ok(result)
    }

    /// Used by shutdown handling: active session jobs become Interrupted and
    /// can be retried manually after restart.
    pub async fn interrupt_active(&self) -> usize {
        let ids = self
            .jobs
            .read()
            .await
            .values()
            .filter(|runtime| runtime.job.status.is_active())
            .map(|runtime| runtime.job.id)
            .collect::<Vec<_>>();
        let mut changed = 0;
        for id in ids {
            if self
                .transition(id, TransferStatus::Interrupted)
                .await
                .is_ok()
            {
                changed += 1;
            }
        }
        changed
    }

    pub async fn wait_for_pause(&self, id: Uuid) -> Result<(), AppError> {
        loop {
            let runtime = self
                .jobs
                .read()
                .await
                .get(&id)
                .cloned()
                .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
            if runtime.cancel_requested {
                return Ok(());
            }
            if !runtime.pause_requested && runtime.job.status != TransferStatus::Paused {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Reach a cooperative pause boundary before starting another item or
    /// provider request. The worker acknowledges `Pausing` as `Paused`, then
    /// waits until the command transitions it back to `Running`.
    /// Return whether this checkpoint crossed a pause boundary. Callers that
    /// stream a resumable response can use that signal to reopen the request
    /// with a validated Range after the user resumes.
    pub async fn checkpoint(&self, id: Uuid) -> Result<bool, AppError> {
        if self.is_cancel_requested(id).await {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        let current = self
            .get(id)
            .await
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        let paused = matches!(
            current.status,
            TransferStatus::Pausing | TransferStatus::Paused
        );
        if current.status == TransferStatus::Pausing {
            self.transition(id, TransferStatus::Paused).await?;
        }
        self.wait_for_pause(id).await?;
        if self.is_cancel_requested(id).await {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        Ok(paused)
    }
}

fn priority_for_operation(operation: TransferOperation) -> JobPriority {
    match operation {
        TransferOperation::CreateFolder
        | TransferOperation::UploadFile
        | TransferOperation::DownloadFile
        | TransferOperation::CopyObject
        | TransferOperation::MoveObject => JobPriority::INTERACTIVE,
        TransferOperation::UploadDirectory
        | TransferOperation::DownloadPrefix
        | TransferOperation::CopyPrefix
        | TransferOperation::MovePrefix
        | TransferOperation::DeleteObjects => JobPriority::USER_TRANSFER,
    }
}

fn validate_request(request: &StartTransferRequest) -> Result<(), AppError> {
    if let Some(metadata) = request.metadata.as_ref() {
        let upload_operation = matches!(
            request.operation,
            TransferOperation::UploadFile | TransferOperation::UploadDirectory
        );
        if !upload_operation
            && !(request.replace_metadata
                && matches!(
                    request.operation,
                    TransferOperation::CopyObject
                        | TransferOperation::CopyPrefix
                        | TransferOperation::MoveObject
                        | TransferOperation::MovePrefix
                ))
        {
            return Err(AppError::Validation(
                "metadata is only valid for upload or replacement-copy operations".to_string(),
            ));
        }
        validate_upload_metadata(metadata)?;
    }
    if request.replace_metadata
        && !matches!(
            request.operation,
            TransferOperation::CopyObject
                | TransferOperation::CopyPrefix
                | TransferOperation::MoveObject
                | TransferOperation::MovePrefix
        )
    {
        return Err(AppError::Validation(
            "replaceMetadata is only valid for copy or move operations".to_string(),
        ));
    }
    if request.total_items == Some(0) {
        return Err(AppError::Validation(
            "totalItems must be greater than zero".to_string(),
        ));
    }
    if request.total_items.is_some_and(|items| items > 1_000_000) {
        return Err(AppError::Validation(
            "totalItems exceeds the supported transfer limit".to_string(),
        ));
    }
    match request.operation {
        TransferOperation::CreateFolder => {
            validate_remote_endpoint(
                &request.source,
                request.profile_id.as_deref(),
                "folder destination",
            )?;
            if request.destination.is_some() {
                return Err(AppError::Validation(
                    "create-folder operation cannot have a destination".to_string(),
                ));
            }
        }
        TransferOperation::UploadFile | TransferOperation::UploadDirectory => {
            validate_local_endpoint(&request.source, "upload source")?;
            let destination = request.destination.as_ref().ok_or_else(|| {
                AppError::Validation("upload destination must be remote".to_string())
            })?;
            validate_remote_endpoint(
                destination,
                request.profile_id.as_deref(),
                "upload destination",
            )?;
        }
        TransferOperation::DownloadFile | TransferOperation::DownloadPrefix => {
            validate_remote_endpoint(
                &request.source,
                request.profile_id.as_deref(),
                "download source",
            )?;
            let destination = request.destination.as_ref().ok_or_else(|| {
                AppError::Validation("download destination must be local".to_string())
            })?;
            validate_local_endpoint(destination, "download destination")?;
        }
        TransferOperation::CopyObject
        | TransferOperation::CopyPrefix
        | TransferOperation::MoveObject
        | TransferOperation::MovePrefix => {
            validate_remote_endpoint(&request.source, request.profile_id.as_deref(), "source")?;
            let destination = request.destination.as_ref().ok_or_else(|| {
                AppError::Validation("copy/move destination must be remote".to_string())
            })?;
            validate_remote_endpoint(destination, request.profile_id.as_deref(), "destination")?;
        }
        TransferOperation::DeleteObjects => {
            validate_remote_endpoint(
                &request.source,
                request.profile_id.as_deref(),
                "delete source",
            )?;
            if request.destination.is_some() {
                return Err(AppError::Validation(
                    "delete operation cannot have a destination".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_upload_metadata(metadata: &UploadMetadata) -> Result<(), AppError> {
    for (field, value, max_bytes) in [
        ("contentType", metadata.content_type.as_deref(), 256_usize),
        (
            "contentDisposition",
            metadata.content_disposition.as_deref(),
            1_024_usize,
        ),
        (
            "cacheControl",
            metadata.cache_control.as_deref(),
            1_024_usize,
        ),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty()
                || value.len() > max_bytes
                || value.chars().any(char::is_control)
            {
                return Err(AppError::Validation(format!("{field} is invalid")));
            }
        }
    }
    if metadata.user_metadata.len() > 100 {
        return Err(AppError::Validation(
            "userMetadata cannot contain more than 100 entries".to_string(),
        ));
    }
    let total_bytes = metadata
        .user_metadata
        .iter()
        .map(|(key, value)| key.len().saturating_add(value.len()))
        .sum::<usize>();
    if total_bytes > 8_192
        || metadata.user_metadata.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 128
                || !key.is_ascii()
                || value.len() > 2_048
                || key.chars().any(char::is_control)
                || value.chars().any(char::is_control)
        })
    {
        return Err(AppError::Validation(
            "userMetadata contains an invalid key or value".to_string(),
        ));
    }
    Ok(())
}

fn validate_local_endpoint(endpoint: &TransferEndpoint, field: &str) -> Result<(), AppError> {
    match endpoint {
        TransferEndpoint::Local { path }
            if !path.trim().is_empty() && !path.contains('\0') && is_absolute_local_path(path) =>
        {
            Ok(())
        }
        TransferEndpoint::Local { .. } => Err(AppError::Validation(format!(
            "{field} path must be non-empty and contain no NUL bytes"
        ))),
        TransferEndpoint::Remote { .. } => {
            Err(AppError::Validation(format!("{field} must be local")))
        }
    }
}

fn is_absolute_local_path(value: &str) -> bool {
    let path = std::path::Path::new(value);
    path.is_absolute()
        || value.starts_with("\\\\")
        || value.starts_with("//")
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\')
            && value.as_bytes()[0].is_ascii_alphabetic())
}

fn validate_remote_endpoint(
    endpoint: &TransferEndpoint,
    expected_profile_id: Option<&str>,
    field: &str,
) -> Result<(), AppError> {
    let TransferEndpoint::Remote {
        profile_id,
        bucket,
        key,
    } = endpoint
    else {
        return Err(AppError::Validation(format!("{field} must be remote")));
    };
    if profile_id.trim().is_empty() || bucket.trim().is_empty() || key.contains('\0') {
        return Err(AppError::Validation(format!(
            "{field} contains an invalid remote location"
        )));
    }
    if expected_profile_id != Some(profile_id.as_str()) {
        return Err(AppError::Validation(format!(
            "{field} profile does not match the transfer profile"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::transfer::{CollisionPolicy, TransferEndpoint};

    fn request() -> StartTransferRequest {
        StartTransferRequest {
            schema_version: DTO_SCHEMA_VERSION,
            operation: TransferOperation::UploadFile,
            profile_id: Some("profile-1".to_string()),
            source: TransferEndpoint::Local {
                path: "C:/tmp/file.txt".to_string(),
            },
            destination: Some(TransferEndpoint::Remote {
                profile_id: "profile-1".to_string(),
                bucket: "bucket".to_string(),
                key: "file.txt".to_string(),
            }),
            collision_policy: CollisionPolicy::Ask,
            total_bytes: Some(10),
            total_items: Some(1),
            confirmation: None,
            recursive: false,
            preserve_root: false,
            replace_metadata: false,
            metadata: None,
            settings_snapshot: None,
        }
    }

    #[tokio::test]
    async fn manager_schedules_updates_and_finishes_a_job() {
        let manager = TransferManager::new(1);
        let job = manager.create(request()).await.unwrap();
        assert_eq!(job.status, TransferStatus::Queued);
        let permit = manager.acquire_next().await.unwrap();
        assert_eq!(permit.id, job.id);
        manager
            .transition(job.id, TransferStatus::Running)
            .await
            .unwrap();
        let progress = manager
            .update_progress(job.id, 10, 1, Some(100), Some(0))
            .await
            .unwrap();
        assert_eq!(progress.transferred_bytes, 10);
        let result = manager
            .finish(job.id, TransferStatus::Completed, 0, 0)
            .await
            .unwrap();
        assert_eq!(result.status, TransferStatus::Completed);
        drop(permit);
        let page = manager
            .list_page(ListTransfersRequest::default())
            .await
            .unwrap();
        assert_eq!(page.total, 1);
    }

    #[tokio::test]
    async fn checkpoint_waits_for_resume_and_reports_pause_boundary() {
        use tokio::time::{sleep, timeout, Duration};

        let manager = TransferManager::new(1);
        let mut request = request();
        request.operation = TransferOperation::DownloadFile;
        request.source = TransferEndpoint::Remote {
            profile_id: "profile-1".to_string(),
            bucket: "bucket".to_string(),
            key: "file.txt".to_string(),
        };
        request.destination = Some(TransferEndpoint::Local {
            path: "C:/tmp/file.txt".to_string(),
        });
        let job = manager.create(request).await.unwrap();
        let permit = manager.acquire_next().await.unwrap();
        manager
            .transition(job.id, TransferStatus::Running)
            .await
            .unwrap();
        manager.request_pause(job.id).await.unwrap();
        let waiting = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.checkpoint(job.id).await.unwrap() })
        };
        timeout(Duration::from_secs(1), async {
            loop {
                if manager
                    .get(job.id)
                    .await
                    .is_some_and(|value| value.status == TransferStatus::Paused)
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        manager.request_resume(job.id).await.unwrap();
        assert!(timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap());
        drop(permit);
    }

    #[tokio::test]
    async fn queued_cancel_does_not_consume_a_scheduler_slot() {
        use tokio::time::{timeout, Duration};

        let manager = TransferManager::new(1);
        let job = manager.create(request()).await.unwrap();
        let cancelled = manager.request_cancel(job.id).await.unwrap();
        assert_eq!(cancelled.status, TransferStatus::Cancelled);
        assert!(timeout(Duration::from_millis(20), manager.acquire_next())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn retry_creates_a_new_queued_job_with_incremented_count() {
        let manager = TransferManager::new(1);
        let job = manager.create(request()).await.unwrap();
        let permit = manager.acquire_next().await.unwrap();
        manager
            .transition(job.id, TransferStatus::Running)
            .await
            .unwrap();
        manager
            .finish(job.id, TransferStatus::Failed, 1, 0)
            .await
            .unwrap();
        drop(permit);
        let retry = manager.retry(job.id).await.unwrap();
        assert_eq!(retry.status, TransferStatus::Queued);
        assert_eq!(retry.retry_count, 1);
    }

    #[test]
    fn validation_rejects_cross_profile_copy_and_wrong_endpoint_shapes() {
        let mut cross_profile = request();
        cross_profile.operation = TransferOperation::CopyObject;
        cross_profile.source = TransferEndpoint::Remote {
            profile_id: "profile-1".to_string(),
            bucket: "bucket".to_string(),
            key: "source.txt".to_string(),
        };
        cross_profile.destination = Some(TransferEndpoint::Remote {
            profile_id: "profile-2".to_string(),
            bucket: "bucket".to_string(),
            key: "destination.txt".to_string(),
        });
        assert!(validate_request(&cross_profile).is_err());

        let mut delete = cross_profile;
        delete.operation = TransferOperation::DeleteObjects;
        delete.destination = None;
        assert!(validate_request(&delete).is_ok());
    }

    #[tokio::test]
    async fn transfer_runtime_migration_applies_to_a_fresh_database() {
        use crate::infrastructure::database::Database;
        use std::path::PathBuf;

        let path: PathBuf = std::env::temp_dir().join(format!(
            "s3-file-manager-transfer-migration-{}.sqlite",
            Uuid::new_v4()
        ));
        let database = Database::connect(&path)
            .await
            .expect("all embedded migrations should apply");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn durable_history_recovers_running_jobs_as_interrupted() {
        use crate::domain::profile::{ConnectionProfile, SecretReference};
        use crate::domain::provider::{AddressingStyle, CredentialMode, ProviderType};
        use crate::infrastructure::database::Database;
        use chrono::Utc;
        use std::path::PathBuf;

        let path: PathBuf = std::env::temp_dir().join(format!(
            "s3-file-manager-transfer-recovery-{}.sqlite",
            Uuid::new_v4()
        ));
        let database = Database::connect(&path).await.unwrap();
        let profile_id = Uuid::new_v4();
        database
            .insert_profile(&ConnectionProfile {
                id: profile_id,
                name: "Recovery test".to_string(),
                provider: ProviderType::CustomS3,
                endpoint: Some("https://s3.example.test".to_string()),
                region: "us-east-1".to_string(),
                credential_mode: CredentialMode::Static,
                access_key_id: Some("access-key".to_string()),
                secret_reference: Some(SecretReference::new(profile_id, "test")),
                session_reference: None,
                default_bucket: Some("bucket".to_string()),
                root_prefix: None,
                addressing_style: AddressingStyle::Path,
                allow_insecure_http: false,
                favorite: false,
                favorite_order: 0,
                revision: 1,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let manager = TransferManager::new_with_database(1, database.clone());
        let mut original_request = request();
        original_request.profile_id = Some(profile_id.to_string());
        original_request.destination = Some(TransferEndpoint::Remote {
            profile_id: profile_id.to_string(),
            bucket: "bucket".to_string(),
            key: "file.txt".to_string(),
        });
        original_request.confirmation = Some("one-shot-confirmation".to_string());
        let job = manager.create(original_request).await.unwrap();
        let permit = manager.acquire_next().await.unwrap();
        manager
            .transition(job.id, TransferStatus::Running)
            .await
            .unwrap();
        drop(permit);

        let recovered = TransferManager::new_with_database(1, database.clone());
        assert_eq!(recovered.recover_from_database().await.unwrap(), 1);
        let interrupted = recovered.get(job.id).await.unwrap();
        assert_eq!(interrupted.status, TransferStatus::Interrupted);
        let persisted = database.list_transfers().await.unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].job.id, job.id);
        assert!(persisted[0].request.confirmation.is_none());

        let retry = recovered.retry(job.id).await.unwrap();
        assert_eq!(retry.status, TransferStatus::Queued);
        assert_eq!(retry.retry_count, 1);

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
