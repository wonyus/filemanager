use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use async_trait::async_trait;
use aws_sdk_s3::{
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart, Delete, MetadataDirective, ObjectIdentifier},
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, Semaphore},
    task::JoinSet,
};
use uuid::Uuid;

const MIN_MULTIPART_PART_BYTES: u64 = 5 * 1024 * 1024;
// A 640 MiB upper bound supports the S3 5 TiB object limit within 10,000
// parts while keeping each bounded upload buffer far below a whole object.
const MAX_IN_MEMORY_PART_BYTES: u64 = 640 * 1024 * 1024;
const SINGLE_COPY_LIMIT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MULTIPART_COPY_PART_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MULTIPART_PARTS: u64 = 10_000;

fn effective_multipart_part_size(configured: u64, object_size: u64) -> Result<u64, AppError> {
    let configured = configured.clamp(MIN_MULTIPART_PART_BYTES, 5 * 1024 * 1024 * 1024);
    let required = object_size.div_ceil(MAX_MULTIPART_PARTS);
    let part_size = configured.max(required).min(MAX_IN_MEMORY_PART_BYTES);
    if object_size.div_ceil(part_size) > MAX_MULTIPART_PARTS {
        return Err(AppError::Validation(
            "object exceeds the S3 multipart object-size limit".to_string(),
        ));
    }
    Ok(part_size)
}

fn multipart_copy_ranges(source_size: u64) -> Result<Vec<(i32, u64, u64)>, AppError> {
    let part_size = effective_multipart_part_size(MULTIPART_COPY_PART_BYTES, source_size)?;
    let part_count = source_size.div_ceil(part_size);
    Ok((0..part_count)
        .map(|index| {
            let start = index.saturating_mul(part_size);
            let end = (start + part_size - 1).min(source_size - 1);
            (i32::try_from(index + 1).unwrap_or(i32::MAX), start, end)
        })
        .collect())
}

/// Validate a destructive-operation acknowledgement at the Rust boundary.
/// For ordinary deletes the token is the literal `DELETE`; recursive deletes
/// additionally pass their exact `DELETE <prefix>` token once the plan has
/// been enumerated. Keeping this check independent of the UI prevents direct
/// IPC callers from bypassing the confirmation requirement.
fn require_delete_confirmation(actual: &str, expected: Option<&str>) -> Result<(), AppError> {
    if actual.trim().is_empty() {
        return Err(AppError::Validation(
            "delete confirmation required; confirm the destructive operation to continue"
                .to_string(),
        ));
    }
    if let Some(expected) = expected {
        if actual != expected {
            return Err(AppError::Validation(format!(
                "delete confirmation is invalid; enter `{expected}` to continue"
            )));
        }
    }
    Ok(())
}

use crate::{
    application::profile_service::ProfileService,
    domain::{
        error::{is_credential_expired_message, AppError, PublicError},
        profile::ConnectionProfile,
    },
    dto::{
        settings::SettingsSnapshot,
        transfer::{
            CollisionPolicy, StartTransferRequest, TransferEndpoint, TransferItem, TransferJob,
            TransferOperation, TransferStatus, UploadMetadata,
        },
    },
    infrastructure::{
        credentials::{resolve_profile_credentials, CredentialStore},
        s3::S3ClientManager,
    },
    transfer::{
        path_mapping::{map_key_to_local, validate_local_path_length},
        recursive::{
            execute_recursive, is_reparse_point, plan_download_prefix, plan_remote_prefix,
            plan_upload_directory_with_options, write_mapping_manifest, CancellationFlag,
            RecursiveExecutor, RecursiveItem, RecursivePlan, RemoteObject,
        },
        retry::{classify_error, RetryPolicy},
        settings::SettingsService,
        TransferManager,
    },
};

#[derive(Debug, Clone, Copy)]
struct ExecutionOutcome {
    transferred_bytes: u64,
    completed_items: u64,
    failed_items: u64,
    cleanup_required_items: u64,
    status: TransferStatus,
    skipped: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct DownloadResumeMetadata {
    bucket: String,
    key: String,
    total_bytes: u64,
    etag: Option<String>,
}

impl ExecutionOutcome {
    fn completed(transferred_bytes: u64, completed_items: u64) -> Self {
        Self {
            transferred_bytes,
            completed_items,
            failed_items: 0,
            cleanup_required_items: 0,
            status: TransferStatus::Completed,
            skipped: false,
        }
    }

    fn skipped() -> Self {
        Self {
            transferred_bytes: 0,
            completed_items: 1,
            failed_items: 0,
            cleanup_required_items: 0,
            status: TransferStatus::Completed,
            skipped: true,
        }
    }
}

/// Bridges the state machine to provider/local I/O.  Commands only create a
/// job through this service; the worker owns secrets and streams in Rust.
#[derive(Clone)]
pub struct TransferService {
    profiles: Arc<ProfileService>,
    credentials: Arc<dyn CredentialStore>,
    clients: Arc<S3ClientManager>,
    manager: Arc<TransferManager>,
    settings: Arc<SettingsService>,
}

impl TransferService {
    pub fn new(
        profiles: Arc<ProfileService>,
        credentials: Arc<dyn CredentialStore>,
        clients: Arc<S3ClientManager>,
        manager: Arc<TransferManager>,
        settings: Arc<SettingsService>,
    ) -> Self {
        Self {
            profiles,
            credentials,
            clients,
            manager,
            settings,
        }
    }

    pub async fn start(&self, mut request: StartTransferRequest) -> Result<TransferJob, AppError> {
        authorize_request(&request)?;
        if request.operation == TransferOperation::UploadFile {
            if let TransferEndpoint::Local { path } = &request.source {
                let source_metadata = fs::metadata(path).await?;
                if !source_metadata.is_file() {
                    return Err(AppError::Validation(
                        "upload source must be a regular file".to_string(),
                    ));
                }
                request.total_bytes = Some(source_metadata.len());
            }
        }
        request.settings_snapshot = Some(self.settings.get().await);
        let job = self.manager.create(request.clone()).await?;
        let worker = self.clone();
        tokio::spawn(async move {
            worker.run(job.id, request).await;
        });
        Ok(job)
    }

    pub async fn retry(&self, id: Uuid) -> Result<TransferJob, AppError> {
        let previous = self
            .manager
            .get(id)
            .await
            .ok_or_else(|| AppError::Unknown(format!("transfer not found: {id}")))?;
        if previous.status == TransferStatus::CompletedWithWarnings
            && matches!(
                previous.operation,
                TransferOperation::MoveObject | TransferOperation::MovePrefix
            )
        {
            self.manager.mark_cleanup_retry(id).await?;
        }
        let job = self.manager.retry(id).await?;
        let request = self.manager.request(job.id).await.ok_or_else(|| {
            AppError::Unknown("retried transfer request was not retained".to_string())
        })?;
        let worker = self.clone();
        tokio::spawn(async move {
            worker.run(job.id, request).await;
        });
        Ok(job)
    }

    async fn run(&self, id: Uuid, request: StartTransferRequest) {
        let result = async {
            let (_started, _scheduler_permit) = self.manager.begin(id).await?;
            if self.manager.is_cancel_requested(id).await {
                return Err(AppError::TransferStateConflict(
                    "transfer cancelled".to_string(),
                ));
            }
            let mut settings = request.settings_snapshot.clone().unwrap_or_default();
            let request_profile_id = request
                .profile_id
                .clone()
                .or_else(|| match &request.source {
                    TransferEndpoint::Remote { profile_id, .. } => Some(profile_id.clone()),
                    TransferEndpoint::Local { .. } => None,
                });
            let _profile_request_permit = if let Some(profile_id) = request_profile_id.as_deref() {
                // A multipart job can issue one SDK request per part worker.
                // Reserve that width from the profile budget and clamp the
                // worker count when the configured profile limit is lower.
                let part_concurrency = settings
                    .per_job_part_concurrency
                    .clamp(1, 16)
                    .min(settings.per_profile_request_limit.clamp(1, 32));
                settings.per_job_part_concurrency = part_concurrency;
                settings.part_concurrency = u32::from(part_concurrency);
                self.clients
                    .configure_request_limit(profile_id, settings.per_profile_request_limit)
                    .await;
                Some(
                    self.clients
                        .acquire_requests(profile_id, usize::from(part_concurrency))
                        .await?,
                )
            } else {
                None
            };
            let outcome = self.execute_with_retry(id, &request, &settings).await?;
            if self.manager.is_cancel_requested(id).await {
                return Err(AppError::TransferStateConflict(
                    "transfer cancelled".to_string(),
                ));
            }
            self.manager
                .update_progress(
                    id,
                    outcome.transferred_bytes,
                    outcome.completed_items,
                    None,
                    None,
                )
                .await?;
            Ok::<ExecutionOutcome, AppError>(outcome)
        }
        .await;

        match result {
            Ok(outcome) => {
                if let Err(error) = self
                    .manager
                    .finish(
                        id,
                        outcome.status,
                        outcome.failed_items,
                        outcome.cleanup_required_items,
                    )
                    .await
                {
                    let _ = self.manager.set_error(id, error).await;
                    let _ = self.manager.transition(id, TransferStatus::Failed).await;
                }
            }
            Err(error) if self.manager.is_cancel_requested(id).await => {
                let _ = self.manager.set_error(id, error).await;
                if self.manager.complete_cancel(id).await.is_err() {
                    let _ = self.manager.transition(id, TransferStatus::Failed).await;
                }
            }
            Err(error) => {
                let _ = self.manager.set_error(id, error).await;
                if self
                    .manager
                    .finish(id, TransferStatus::Failed, 1, 0)
                    .await
                    .is_err()
                {
                    let _ = self.manager.transition(id, TransferStatus::Failed).await;
                }
            }
        }
    }

    async fn execute_with_retry(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let policy = RetryPolicy {
            retry_limit: u32::from(settings.retry_limit),
            base_delay: std::time::Duration::from_millis(settings.retry_base_delay_ms),
            max_delay: std::time::Duration::from_millis(settings.retry_max_delay_ms),
        };
        let mut retry_number = 0_u32;
        loop {
            match self.execute(id, request, settings).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) => {
                    if self.manager.is_cancel_requested(id).await {
                        return Err(error);
                    }
                    if !policy.should_retry(retry_number, classify_error(&error)) {
                        return Err(error);
                    }
                    retry_number = retry_number.saturating_add(1);
                    let _ = self.manager.transition(id, TransferStatus::Retrying).await;
                    let delay = policy.jittered_delay_for_retry(retry_number - 1);
                    tokio::time::sleep(delay).await;
                    if self.manager.is_cancel_requested(id).await {
                        return Err(AppError::TransferStateConflict(
                            "transfer cancelled".to_string(),
                        ));
                    }
                    let _ = self.manager.transition(id, TransferStatus::Running).await;
                }
            }
        }
    }

    async fn execute(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        match request.operation {
            TransferOperation::CreateFolder => self.create_folder(id, request).await,
            TransferOperation::UploadFile => self.upload_file(id, request, settings).await,
            TransferOperation::DownloadFile => self.download_file(id, request, settings).await,
            TransferOperation::CopyObject => self.copy_object(id, request).await,
            TransferOperation::MoveObject => {
                if request.cleanup_only {
                    self.cleanup_move_object(id, request).await
                } else {
                    self.move_object(id, request).await
                }
            }
            TransferOperation::DeleteObjects => self.delete_object(id, request, settings).await,
            TransferOperation::UploadDirectory => {
                self.upload_directory(id, request, settings).await
            }
            TransferOperation::DownloadPrefix => self.download_prefix(id, request, settings).await,
            TransferOperation::CopyPrefix => self.copy_prefix(id, request, settings).await,
            TransferOperation::MovePrefix => {
                if request.cleanup_only {
                    self.cleanup_move_prefix(id, request, settings).await
                } else {
                    self.move_prefix(id, request, settings).await
                }
            }
        }
    }

    async fn upload_file(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        self.manager.checkpoint(id).await?;
        let source = local_path(&request.source)?;
        let (profile, bucket, mut key) = self.remote_target(request.destination.as_ref()).await?;
        let metadata = fs::metadata(&source).await?;
        if !metadata.is_file() {
            return Err(AppError::Validation(
                "upload source must be a file".to_string(),
            ));
        }
        let size = metadata.len();
        if request.total_bytes.is_some_and(|planned| planned != size) {
            return Err(AppError::LocalFileChanged);
        }
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        if request.collision_policy == CollisionPolicy::Rename {
            key = unique_remote_key(&client, &bucket, &key).await?;
        }
        let collision_policy = if request.collision_policy == CollisionPolicy::Rename {
            CollisionPolicy::Fail
        } else {
            request.collision_policy
        };
        if let Err(error) = ensure_collision(&client, &bucket, &key, collision_policy).await {
            if request.collision_policy == CollisionPolicy::Skip
                && matches!(error, AppError::TransferStateConflict(_))
            {
                return Ok(ExecutionOutcome::completed(0, 1));
            }
            return Err(error);
        }
        if size >= settings.multipart_threshold_bytes {
            let profile_id = profile.id.to_string();
            self.upload_multipart(
                id,
                Some(&profile_id),
                &client,
                &bucket,
                &key,
                &source,
                size,
                settings,
                request.metadata.as_ref(),
            )
            .await?;
        } else {
            let body = ByteStream::from_path(&source)
                .await
                .map_err(|error| AppError::Provider(format!("local upload stream: {error}")))?;
            let mut upload = client
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .content_length(size as i64);
            if let Some(value) = upload_content_type(&source, request.metadata.as_ref()) {
                upload = upload.content_type(value);
            }
            if let Some(metadata) = request.metadata.as_ref() {
                if let Some(value) = metadata.content_disposition.as_deref() {
                    upload = upload.content_disposition(value);
                }
                if let Some(value) = metadata.cache_control.as_deref() {
                    upload = upload.cache_control(value);
                }
                for (name, value) in &metadata.user_metadata {
                    upload = upload.metadata(name, value);
                }
            }
            upload
                .body(body)
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            self.manager
                .update_progress(id, size, 1, None, None)
                .await?;
        }
        verify_local_upload_snapshot(&source, size, metadata.modified().ok()).await?;
        Ok(ExecutionOutcome::completed(size, 1))
    }

    async fn create_folder(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
    ) -> Result<ExecutionOutcome, AppError> {
        self.manager.checkpoint(id).await?;
        let (profile, bucket, key) = self.remote_target(Some(&request.source)).await?;
        let marker = if key.ends_with('/') {
            key
        } else {
            format!("{key}/")
        };
        if marker.is_empty() {
            return Err(AppError::Validation(
                "folder name must be non-empty".to_string(),
            ));
        }
        if profile.root_prefix.as_deref() == Some(marker.as_str()) {
            return Err(AppError::Validation(
                "cannot recreate the profile root prefix as a folder".to_string(),
            ));
        }
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let base_object = marker.trim_end_matches('/');
        if prefix_has_objects(&client, &bucket, &marker).await?
            || object_exists(&client, &bucket, base_object).await?
        {
            return Err(AppError::Validation(
                "folder or object already exists at the requested path".to_string(),
            ));
        }
        client
            .put_object()
            .bucket(&bucket)
            .key(&marker)
            .content_length(0)
            .body(ByteStream::from(Vec::<u8>::new()))
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        Ok(ExecutionOutcome::completed(0, 1))
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_multipart(
        &self,
        id: Uuid,
        profile_id: Option<&str>,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
        source: &Path,
        size: u64,
        settings: &SettingsSnapshot,
        metadata: Option<&UploadMetadata>,
    ) -> Result<(), AppError> {
        let part_size = effective_multipart_part_size(settings.initial_part_size_bytes, size)?;
        let mut upload_builder = client.create_multipart_upload().bucket(bucket).key(key);
        if let Some(value) = upload_content_type(source, metadata) {
            upload_builder = upload_builder.content_type(value);
        }
        if let Some(metadata) = metadata {
            if let Some(value) = metadata.content_disposition.as_deref() {
                upload_builder = upload_builder.content_disposition(value);
            }
            if let Some(value) = metadata.cache_control.as_deref() {
                upload_builder = upload_builder.cache_control(value);
            }
            for (name, value) in &metadata.user_metadata {
                upload_builder = upload_builder.metadata(name, value);
            }
        }
        let upload = upload_builder
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();

        // Persist the provider handle before uploading the first part. If the
        // local checkpoint cannot be written, abort the provider upload so an
        // orphaned multipart session is not created by a local disk failure.
        if let Err(error) = self
            .manager
            .persist_multipart_upload(id, profile_id, bucket, key, &upload_id, part_size)
            .await
        {
            let _ = client
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            return Err(error);
        }

        let part_concurrency = usize::from(settings.per_job_part_concurrency.clamp(1, 16));
        let permits = Arc::new(Semaphore::new(part_concurrency));
        let mut in_flight = JoinSet::new();
        let mut completed = BTreeMap::<i32, (String, u64)>::new();
        let mut transferred = 0_u64;
        let mut part_number = 1_i32;

        async fn join_part(
            in_flight: &mut JoinSet<Result<(i32, String, u64), AppError>>,
        ) -> Result<(i32, String, u64), AppError> {
            let joined = in_flight
                .join_next()
                .await
                .ok_or_else(|| AppError::Unknown("multipart part queue ended early".to_string()))?;
            joined.map_err(|error| {
                AppError::Unknown(format!("multipart part task failed: {error}"))
            })?
        }

        async fn record_part(
            manager: &TransferManager,
            transfer_id: Uuid,
            completed: &mut BTreeMap<i32, (String, u64)>,
            transferred: &mut u64,
            part: (i32, String, u64),
        ) -> Result<(), AppError> {
            let (part_number, etag, size_bytes) = part;
            manager
                .persist_multipart_part(
                    transfer_id,
                    u32::try_from(part_number).map_err(|_| {
                        AppError::Unknown("multipart part number overflow".to_string())
                    })?,
                    &etag,
                    size_bytes,
                )
                .await?;
            completed.insert(part_number, (etag, size_bytes));
            *transferred = transferred.saturating_add(size_bytes);
            manager
                .update_progress(transfer_id, *transferred, 0, None, None)
                .await?;
            Ok(())
        }

        let result = async {
            let mut file = File::open(source).await?;
            loop {
                self.manager.checkpoint(id).await?;
                let mut buffer = vec![0_u8; part_size as usize];
                let mut read = 0_usize;
                while read < buffer.len() {
                    let count = file.read(&mut buffer[read..]).await?;
                    if count == 0 {
                        break;
                    }
                    read += count;
                }
                if read == 0 {
                    break;
                }
                buffer.truncate(read);
                let permit = permits.clone().acquire_owned().await.map_err(|_| {
                    AppError::TransferStateConflict(
                        "multipart part scheduler is unavailable".to_string(),
                    )
                })?;
                let part_id = part_number;
                let part_bytes = read as u64;
                let part_bucket = bucket.to_string();
                let part_key = key.to_string();
                let part_upload_id = upload_id.clone();
                let part_client = client.clone();
                in_flight.spawn(async move {
                    let _permit = permit;
                    let output = part_client
                        .upload_part()
                        .bucket(part_bucket)
                        .key(part_key)
                        .upload_id(part_upload_id)
                        .part_number(part_id)
                        .body(ByteStream::from(buffer))
                        .send()
                        .await
                        .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                    let etag = output.e_tag().map(ToString::to_string).ok_or_else(|| {
                        AppError::Provider("provider did not return a part ETag".to_string())
                    })?;
                    Ok((part_id, etag, part_bytes))
                });
                part_number += 1;
                if in_flight.len() >= part_concurrency {
                    let part = join_part(&mut in_flight).await?;
                    record_part(&self.manager, id, &mut completed, &mut transferred, part).await?;
                }
            }
            while !in_flight.is_empty() {
                let part = join_part(&mut in_flight).await?;
                record_part(&self.manager, id, &mut completed, &mut transferred, part).await?;
            }
            let completed_parts = completed
                .iter()
                .map(|(part_number, (etag, _))| {
                    CompletedPart::builder()
                        .part_number(*part_number)
                        .e_tag(etag.clone())
                        .build()
                })
                .collect::<Vec<_>>();
            let multipart = CompletedMultipartUpload::builder()
                .set_parts(Some(completed_parts))
                .build();
            client
                .complete_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .multipart_upload(multipart)
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            let _ = client
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            if let Err(error) = self.manager.clear_multipart_upload(id).await {
                tracing::warn!(transfer_id = %id, error = %error, "unable to clear aborted multipart checkpoint");
            }
        } else if let Err(error) = self.manager.clear_multipart_upload(id).await {
            // The provider object is already complete; retain the successful
            // transfer result and surface the stale local checkpoint only in
            // diagnostics so the user is not asked to upload it again.
            tracing::warn!(transfer_id = %id, error = %error, "unable to clear completed multipart checkpoint");
        }
        result
    }

    async fn download_file(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        self.manager.checkpoint(id).await?;
        let (profile, bucket, key) = self.remote_target(Some(&request.source)).await?;
        let destination_directory_hint = request
            .destination
            .as_ref()
            .is_some_and(local_endpoint_has_directory_hint);
        let mut destination = local_path(request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("download destination is required".to_string())
        })?)?;
        ensure_local_path_not_reparse(&destination)?;
        // A directory picked for a multi-selection download is resolved to a
        // deterministic, sanitized leaf before collision handling. A
        // Save-file picker returns a non-directory path and keeps its explicit
        // user-selected filename unchanged.
        if destination.is_dir() || destination_directory_hint {
            let leaf = key
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::Validation("object key has no file name".to_string()))?;
            destination = map_key_to_local(&destination, "", leaf)?;
            ensure_local_path_not_reparse(&destination)?;
        }
        if destination.exists() && request.collision_policy != CollisionPolicy::Replace {
            if request.collision_policy == CollisionPolicy::Skip {
                return Ok(ExecutionOutcome::completed(0, 1));
            }
            if request.collision_policy == CollisionPolicy::Rename {
                destination = unique_local_destination(&destination)?;
            } else {
                return Err(AppError::Validation(
                    "destination already exists".to_string(),
                ));
            }
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
            ensure_local_path_not_reparse(&destination)?;
        }
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let partial = partial_path(&destination, id);
        let partial_metadata = partial.with_extension("s3fm-meta");
        let head = client
            .head_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let total = head
            .content_length()
            .and_then(|value| u64::try_from(value).ok());
        let remote_etag = head.e_tag().map(ToString::to_string);
        let mut offset = 0_u64;
        if partial.exists() {
            let valid_resume = if let Ok(value) = fs::read_to_string(&partial_metadata).await {
                serde_json::from_str::<DownloadResumeMetadata>(&value)
                    .ok()
                    .is_some_and(|metadata| {
                        metadata.bucket == bucket
                            && metadata.key == key
                            && total.is_some_and(|value| metadata.total_bytes == value)
                            && metadata.etag == remote_etag
                    })
            } else {
                false
            };
            if valid_resume {
                offset = fs::metadata(&partial).await?.len();
                if total.is_some_and(|value| offset >= value) {
                    offset = 0;
                    let _ = fs::remove_file(&partial).await;
                }
            } else {
                let _ = fs::remove_file(&partial).await;
                let _ = fs::remove_file(&partial_metadata).await;
            }
        }
        if let Some(total) = total {
            let required = total.saturating_sub(offset);
            check_available_disk_space(
                destination.parent().unwrap_or_else(|| Path::new(".")),
                required,
            )?;
        }
        let resume_metadata = DownloadResumeMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            total_bytes: total.unwrap_or_default(),
            etag: remote_etag,
        };
        fs::write(
            &partial_metadata,
            serde_json::to_vec(&resume_metadata).map_err(|error| {
                AppError::Unknown(format!("resume metadata encoding failed: {error}"))
            })?,
        )
        .await?;
        let result: Result<u64, AppError> = async {
            loop {
                self.manager.checkpoint(id).await?;
                let mut get_request = client.get_object().bucket(&bucket).key(&key);
                if offset > 0 {
                    get_request = get_request.range(format!("bytes={offset}-"));
                }
                let output = get_request
                    .send()
                    .await
                    .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                let response_size = output
                    .content_length()
                    .and_then(|value| u64::try_from(value).ok())
                    .unwrap_or_default();
                if let Some(expected) = total {
                    if offset.saturating_add(response_size) != expected {
                        return Err(AppError::Provider(
                            "download resume identity or content range did not match the remote object"
                                .to_string(),
                        ));
                    }
                }
                let mut file = if offset > 0 {
                    fs::OpenOptions::new().append(true).open(&partial).await?
                } else {
                    File::create(&partial).await?
                };
                let mut stream = output.body.into_async_read();
                let mut buffer = vec![0_u8; 1024 * 1024];
                let mut paused = false;
                loop {
                    if self.manager.checkpoint(id).await? {
                        paused = true;
                        break;
                    }
                    let count = stream.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    file.write_all(&buffer[..count]).await?;
                    offset = offset.saturating_add(count as u64);
                    self.manager
                        .update_progress(id, offset, 0, None, None)
                        .await?;
                }
                file.flush().await?;
                if paused {
                    // The checkpoint waits for resume. Reopen the provider
                    // response from the new local offset rather than reading
                    // from a stale response stream.
                    continue;
                }
                if let Some(expected) = total {
                    if offset != expected {
                        return Err(AppError::Provider(format!(
                            "download size mismatch: expected {expected} bytes, received {offset}"
                        )));
                    }
                }
                break;
            }
            Ok(offset)
        }
        .await;
        match result {
            Ok(transferred) => {
                if destination.exists() {
                    if let Err(error) = fs::remove_file(&destination).await {
                        if !settings.keep_partial_downloads {
                            let _ = fs::remove_file(&partial).await;
                        }
                        return Err(error.into());
                    }
                }
                if let Err(error) = fs::rename(&partial, &destination).await {
                    if !settings.keep_partial_downloads {
                        let _ = fs::remove_file(&partial).await;
                    }
                    return Err(error.into());
                }
                let _ = fs::remove_file(&partial_metadata).await;
                Ok(ExecutionOutcome::completed(total.unwrap_or(transferred), 1))
            }
            Err(error) => {
                if !settings.keep_partial_downloads {
                    let _ = fs::remove_file(&partial).await;
                    let _ = fs::remove_file(&partial_metadata).await;
                }
                Err(error)
            }
        }
    }

    async fn copy_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
    ) -> Result<ExecutionOutcome, AppError> {
        self.manager.checkpoint(id).await?;
        let (source_profile, source_bucket, source_key) =
            self.remote_target(Some(&request.source)).await?;
        let mut destination = match request.destination.as_ref() {
            Some(TransferEndpoint::Remote {
                profile_id,
                bucket,
                key,
            }) => {
                if profile_id != &source_profile.id.to_string() {
                    return Err(AppError::UnsupportedProviderFeature(
                        "cross-profile copy is not supported".to_string(),
                    ));
                }
                authorize_key(&source_profile, bucket, key)?;
                (bucket.clone(), key.clone())
            }
            _ => {
                return Err(AppError::Validation(
                    "copy destination must be remote".to_string(),
                ))
            }
        };
        if source_bucket == destination.0 && source_key == destination.1 {
            return Err(AppError::Validation(
                "copy/move source and destination must differ".to_string(),
            ));
        }
        let credentials =
            resolve_profile_credentials(self.credentials.as_ref(), &source_profile).await?;
        let client = self
            .clients
            .get_or_create(&source_profile, &credentials)
            .await?;
        if request.collision_policy == CollisionPolicy::Rename {
            destination.1 = unique_remote_key(&client, &destination.0, &destination.1).await?;
        }
        if let Err(error) = ensure_collision(
            &client,
            &destination.0,
            &destination.1,
            request.collision_policy,
        )
        .await
        {
            if request.collision_policy == CollisionPolicy::Skip
                && matches!(error, AppError::TransferStateConflict(_))
            {
                return Ok(ExecutionOutcome::skipped());
            }
            return Err(error);
        }
        let source_head = client
            .head_object()
            .bucket(&source_bucket)
            .key(&source_key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let source_size = source_head.content_length().unwrap_or(0).max(0) as u64;
        let source_etag = source_head.e_tag().map(ToString::to_string);
        let multipart_copy = source_size > SINGLE_COPY_LIMIT_BYTES;
        if multipart_copy {
            let source_profile_id = source_profile.id.to_string();
            self.copy_multipart(
                id,
                Some(&source_profile_id),
                &client,
                &source_bucket,
                &source_key,
                &destination.0,
                &destination.1,
                source_size,
                &source_head,
                request.replace_metadata,
                request.metadata.as_ref(),
            )
            .await?;
        } else {
            let mut copy = client
                .copy_object()
                .copy_source(encode_copy_source(&source_bucket, &source_key))
                .bucket(&destination.0)
                .key(&destination.1);
            if request.replace_metadata {
                copy = copy.metadata_directive(MetadataDirective::Replace);
                if let Some(metadata) = request.metadata.as_ref() {
                    if let Some(value) = metadata.content_type.as_deref() {
                        copy = copy.content_type(value);
                    }
                    if let Some(value) = metadata.content_disposition.as_deref() {
                        copy = copy.content_disposition(value);
                    }
                    if let Some(value) = metadata.cache_control.as_deref() {
                        copy = copy.cache_control(value);
                    }
                    for (name, value) in &metadata.user_metadata {
                        copy = copy.metadata(name, value);
                    }
                }
            }
            copy.send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        }
        if self.manager.is_cancel_requested(id).await {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        let verified_head = client
            .head_object()
            .bucket(&destination.0)
            .key(&destination.1)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let verified = verified_head.content_length().unwrap_or(0).max(0) as u64;
        if source_size != verified
            || (!multipart_copy
                && source_etag.is_some()
                && verified_head.e_tag().map(ToString::to_string) != source_etag)
            || !checksums_match(&source_head, &verified_head)
            || if request.replace_metadata {
                !replacement_metadata_matches(&verified_head, request.metadata.as_ref())
            } else {
                !metadata_matches(&source_head, &verified_head)
            }
        {
            return Err(AppError::Provider("copy verification failed".to_string()));
        }
        Ok(ExecutionOutcome::completed(source_size, 1))
    }

    #[allow(clippy::too_many_arguments)]
    async fn copy_multipart(
        &self,
        id: Uuid,
        profile_id: Option<&str>,
        client: &aws_sdk_s3::Client,
        source_bucket: &str,
        source_key: &str,
        destination_bucket: &str,
        destination_key: &str,
        source_size: u64,
        source_head: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
        replace_metadata: bool,
        replacement: Option<&UploadMetadata>,
    ) -> Result<(), AppError> {
        let part_size = effective_multipart_part_size(MULTIPART_COPY_PART_BYTES, source_size)?;
        let ranges = multipart_copy_ranges(source_size)?;
        let mut create = client
            .create_multipart_upload()
            .bucket(destination_bucket)
            .key(destination_key);
        if !replace_metadata {
            if let Some(value) = source_head.content_type() {
                create = create.content_type(value);
            }
            if let Some(value) = source_head.content_disposition() {
                create = create.content_disposition(value);
            }
            if let Some(value) = source_head.cache_control() {
                create = create.cache_control(value);
            }
            if let Some(value) = source_head.content_encoding() {
                create = create.content_encoding(value);
            }
            if let Some(value) = source_head.content_language() {
                create = create.content_language(value);
            }
            #[allow(deprecated)]
            if let Some(value) = source_head.expires().cloned() {
                create = create.expires(value);
            }
            if let Some(value) = source_head.website_redirect_location() {
                create = create.website_redirect_location(value);
            }
            if let Some(metadata) = source_head.metadata() {
                for (key, value) in metadata {
                    create = create.metadata(key, value);
                }
            }
        } else if let Some(metadata) = replacement {
            if let Some(value) = metadata.content_type.as_deref() {
                create = create.content_type(value);
            }
            if let Some(value) = metadata.content_disposition.as_deref() {
                create = create.content_disposition(value);
            }
            if let Some(value) = metadata.cache_control.as_deref() {
                create = create.cache_control(value);
            }
            for (key, value) in &metadata.user_metadata {
                create = create.metadata(key, value);
            }
        }
        let upload = create
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();
        if let Err(error) = self
            .manager
            .persist_multipart_upload(
                id,
                profile_id,
                destination_bucket,
                destination_key,
                &upload_id,
                part_size,
            )
            .await
        {
            let _ = client
                .abort_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .send()
                .await;
            return Err(error);
        }
        let source = encode_copy_source(source_bucket, source_key);
        let part_concurrency = usize::from(
            self.settings
                .get()
                .await
                .per_job_part_concurrency
                .clamp(1, 16),
        );
        let permits = Arc::new(Semaphore::new(part_concurrency));
        let result = async {
            let mut in_flight = JoinSet::new();
            let mut parts = BTreeMap::<i32, (String, u64)>::new();
            let mut transferred = 0_u64;
            for (part_number, start, end) in ranges {
                self.manager.checkpoint(id).await?;
                let bytes = end.saturating_sub(start).saturating_add(1);
                let permit = permits.clone().acquire_owned().await.map_err(|_| {
                    AppError::TransferStateConflict(
                        "multipart copy scheduler is unavailable".to_string(),
                    )
                })?;
                let copy_client = client.clone();
                let copy_bucket = destination_bucket.to_string();
                let copy_key = destination_key.to_string();
                let copy_upload_id = upload_id.clone();
                let copy_source = source.clone();
                let copy_range = format!("bytes={start}-{end}");
                let copy_part_number = part_number;
                in_flight.spawn(async move {
                    let _permit = permit;
                    let output = copy_client
                        .upload_part_copy()
                        .bucket(copy_bucket)
                        .key(copy_key)
                        .upload_id(copy_upload_id)
                        .part_number(copy_part_number)
                        .copy_source(copy_source)
                        .copy_source_range(copy_range)
                        .send()
                        .await
                        .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                    let etag = output
                        .copy_part_result()
                        .and_then(|result| result.e_tag())
                        .map(ToString::to_string)
                        .ok_or_else(|| {
                            AppError::Provider(
                                "provider did not return a copy part ETag".to_string(),
                            )
                        })?;
                    Ok::<(i32, String, u64), AppError>((copy_part_number, etag, bytes))
                });
                if in_flight.len() >= part_concurrency {
                    let joined = in_flight.join_next().await.ok_or_else(|| {
                        AppError::Unknown("multipart copy queue ended early".to_string())
                    })?;
                    let (part, etag, size_bytes) = joined.map_err(|error| {
                        AppError::Unknown(format!("multipart copy task failed: {error}"))
                    })??;
                    self.manager
                        .persist_multipart_part(
                            id,
                            u32::try_from(part).map_err(|_| {
                                AppError::Unknown("multipart part number overflow".to_string())
                            })?,
                            &etag,
                            size_bytes,
                        )
                        .await?;
                    parts.insert(part, (etag, size_bytes));
                    transferred = transferred.saturating_add(size_bytes);
                    self.manager
                        .update_progress(id, transferred, 0, None, None)
                        .await?;
                }
            }
            while !in_flight.is_empty() {
                let joined = in_flight.join_next().await.ok_or_else(|| {
                    AppError::Unknown("multipart copy queue ended early".to_string())
                })?;
                let (part, etag, size_bytes) = joined.map_err(|error| {
                    AppError::Unknown(format!("multipart copy task failed: {error}"))
                })??;
                self.manager
                    .persist_multipart_part(
                        id,
                        u32::try_from(part).map_err(|_| {
                            AppError::Unknown("multipart part number overflow".to_string())
                        })?,
                        &etag,
                        size_bytes,
                    )
                    .await?;
                parts.insert(part, (etag, size_bytes));
                transferred = transferred.saturating_add(size_bytes);
                self.manager
                    .update_progress(id, transferred, 0, None, None)
                    .await?;
            }
            let completed_parts = parts
                .iter()
                .map(|(part, (etag, _))| {
                    CompletedPart::builder()
                        .part_number(*part)
                        .e_tag(etag.clone())
                        .build()
                })
                .collect::<Vec<_>>();
            client
                .complete_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(completed_parts))
                        .build(),
                )
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            let _ = client
                .abort_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .send()
                .await;
            if let Err(error) = self.manager.clear_multipart_upload(id).await {
                tracing::warn!(transfer_id = %id, error = %error, "unable to clear aborted multipart copy checkpoint");
            }
        } else if let Err(error) = self.manager.clear_multipart_upload(id).await {
            tracing::warn!(transfer_id = %id, error = %error, "unable to clear completed multipart copy checkpoint");
        }
        result
    }

    async fn cleanup_move_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
    ) -> Result<ExecutionOutcome, AppError> {
        self.manager.checkpoint(id).await?;
        let (source_profile, source_bucket, source_key) =
            self.remote_target(Some(&request.source)).await?;
        let destination = request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("move cleanup destination is required".to_string())
        })?;
        let (destination_profile, destination_bucket, destination_key) =
            self.remote_target(Some(destination)).await?;
        if source_profile.id != destination_profile.id || source_bucket != destination_bucket {
            return Err(AppError::UnsupportedProviderFeature(
                "move cleanup requires the original profile and bucket".to_string(),
            ));
        }
        let credentials =
            resolve_profile_credentials(self.credentials.as_ref(), &source_profile).await?;
        let client = self
            .clients
            .get_or_create(&source_profile, &credentials)
            .await?;
        let source_head = match client
            .head_object()
            .bucket(&source_bucket)
            .key(&source_key)
            .send()
            .await
        {
            Ok(head) => Some(head),
            Err(error) if is_not_found_provider_error(&error) => None,
            Err(error) => return Err(AppError::Provider(safe_provider_error(&error))),
        };
        let destination_head = client
            .head_object()
            .bucket(&destination_bucket)
            .key(&destination_key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        if let Some(source_head) = source_head.as_ref() {
            let source_size = source_head.content_length().unwrap_or_default().max(0) as u64;
            let destination_size =
                destination_head.content_length().unwrap_or_default().max(0) as u64;
            if source_size != destination_size || !checksums_match(source_head, &destination_head) {
                return Err(AppError::Provider(
                    "cleanup verification failed; destination no longer matches source".to_string(),
                ));
            }
        }
        if source_head.is_some() {
            client
                .delete_object()
                .bucket(&source_bucket)
                .key(&source_key)
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        }
        Ok(ExecutionOutcome::completed(
            destination_head.content_length().unwrap_or_default().max(0) as u64,
            1,
        ))
    }

    async fn move_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
    ) -> Result<ExecutionOutcome, AppError> {
        let result = self.copy_object(id, request).await?;
        if result.skipped {
            return Ok(result);
        }
        let (_, bucket, key) = self.remote_target(Some(&request.source)).await?;
        let credentials = resolve_profile_credentials(
            self.credentials.as_ref(),
            &self
                .profiles
                .get_connection_profile(match &request.source {
                    TransferEndpoint::Remote { profile_id, .. } => profile_id,
                    _ => {
                        return Err(AppError::Validation(
                            "move source must be remote".to_string(),
                        ))
                    }
                })
                .await?,
        )
        .await?;
        let profile = self
            .profiles
            .get_connection_profile(match &request.source {
                TransferEndpoint::Remote { profile_id, .. } => profile_id,
                _ => unreachable!(),
            })
            .await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        if self.manager.is_cancel_requested(id).await {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        if let Err(error) = client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))
        {
            let mut warning = result;
            warning.status = TransferStatus::CompletedWithWarnings;
            warning.failed_items = 1;
            warning.cleanup_required_items = 1;
            let _ = self.manager.set_error(id, error).await;
            return Ok(warning);
        }
        Ok(result)
    }

    async fn cleanup_move_prefix(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        _settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let (source_profile, bucket, source_prefix) =
            self.remote_target(Some(&request.source)).await?;
        let destination = request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("move cleanup destination is required".to_string())
        })?;
        let (destination_profile, destination_bucket, destination_prefix) =
            self.remote_target(Some(destination)).await?;
        if source_profile.id != destination_profile.id || bucket != destination_bucket {
            return Err(AppError::UnsupportedProviderFeature(
                "move cleanup requires the original profile and bucket".to_string(),
            ));
        }
        let credentials =
            resolve_profile_credentials(self.credentials.as_ref(), &source_profile).await?;
        let client = self
            .clients
            .get_or_create(&source_profile, &credentials)
            .await?;
        let objects = self
            .list_remote_objects(id, &client, &bucket, &source_prefix)
            .await?;
        let existing = self
            .list_remote_objects(id, &client, &bucket, &destination_prefix)
            .await?
            .into_iter()
            .map(|object| object.key)
            .collect::<HashSet<_>>();
        let plan = plan_remote_prefix(
            TransferOperation::MovePrefix,
            &source_profile.id.to_string(),
            &bucket,
            &source_prefix,
            &destination_prefix,
            &objects,
            CollisionPolicy::Replace,
            &existing,
        )?;
        self.manager
            .set_totals(
                id,
                Some(plan.items.len() as u64),
                Some(plan.items.iter().filter_map(|item| item.size_bytes).sum()),
            )
            .await?;
        let snapshots = plan
            .items
            .iter()
            .map(recursive_item_snapshot)
            .collect::<Vec<_>>();
        self.manager.replace_transfer_items(id, &snapshots).await?;
        let mut completed = 0_u64;
        let mut failed = 0_u64;
        let mut transferred = 0_u64;
        for item in &plan.items {
            self.manager.checkpoint(id).await?;
            let (
                TransferEndpoint::Remote {
                    key: source_key, ..
                },
                TransferEndpoint::Remote {
                    key: destination_key,
                    ..
                },
            ) = (&item.source, &item.destination)
            else {
                continue;
            };
            let source_head = match client
                .head_object()
                .bucket(&bucket)
                .key(source_key)
                .send()
                .await
            {
                Ok(head) => Some(head),
                Err(error) if is_not_found_provider_error(&error) => None,
                Err(error) => return Err(AppError::Provider(safe_provider_error(&error))),
            };
            let outcome = async {
                let destination_head = client
                    .head_object()
                    .bucket(&bucket)
                    .key(destination_key)
                    .send()
                    .await
                    .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                if let Some(source_head) = source_head.as_ref() {
                    let source_size =
                        source_head.content_length().unwrap_or_default().max(0) as u64;
                    let destination_size =
                        destination_head.content_length().unwrap_or_default().max(0) as u64;
                    if source_size != destination_size
                        || !checksums_match(source_head, &destination_head)
                    {
                        return Err(AppError::Provider(
                            "cleanup verification failed; destination no longer matches source"
                                .to_string(),
                        ));
                    }
                }
                if source_head.is_some() {
                    client
                        .delete_object()
                        .bucket(&bucket)
                        .key(source_key)
                        .send()
                        .await
                        .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                }
                Ok::<u64, AppError>(
                    destination_head.content_length().unwrap_or_default().max(0) as u64
                )
            }
            .await;
            match outcome {
                Ok(bytes) => {
                    completed += 1;
                    transferred = transferred.saturating_add(bytes);
                    self.manager
                        .update_transfer_item(
                            id,
                            &item.id,
                            TransferStatus::Completed,
                            bytes,
                            None,
                            false,
                        )
                        .await?;
                }
                Err(error) => {
                    failed += 1;
                    let public = PublicError::from(error);
                    self.manager
                        .update_transfer_item(
                            id,
                            &item.id,
                            TransferStatus::Failed,
                            0,
                            Some(&public),
                            true,
                        )
                        .await?;
                }
            }
            self.manager
                .update_progress(id, transferred, completed, None, None)
                .await?;
        }
        Ok(ExecutionOutcome {
            transferred_bytes: transferred,
            completed_items: completed,
            failed_items: failed,
            cleanup_required_items: failed,
            status: if failed == 0 {
                TransferStatus::Completed
            } else {
                TransferStatus::CompletedWithWarnings
            },
            skipped: false,
        })
    }

    async fn delete_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let (profile, bucket, key) = self.remote_target(Some(&request.source)).await?;
        self.manager.checkpoint(id).await?;
        let recursive = request.recursive || key.ends_with('/');
        if recursive && key.is_empty() {
            return Err(AppError::Validation(
                "recursive delete requires a non-empty prefix".to_string(),
            ));
        }
        if profile.root_prefix.as_deref() == Some(key.as_str()) {
            return Err(AppError::Validation(
                "deleting the profile root prefix requires selecting child objects explicitly"
                    .to_string(),
            ));
        }
        let confirmation = request.confirmation.as_deref().unwrap_or_default();
        // Confirmation is enforced at the Rust boundary as well as in the
        // renderer.  A caller that invokes the command directly must not be
        // able to bypass the destructive-operation prompt. Recursive jobs
        // use a non-empty acknowledgement here; large jobs are checked
        // against the exact prefix token after enumeration below.
        let direct_confirmation = if request.delete_keys.is_some() {
            None
        } else {
            (!recursive).then_some("DELETE")
        };
        require_delete_confirmation(confirmation, direct_confirmation)?;
        if request.delete_keys.is_some() {
            // Explicit multi-selection uses `DELETE` for ordinary batches and
            // an exact count token for large batches.  Either way, an empty
            // acknowledgement must never reach the provider.
            require_delete_confirmation(confirmation, None)?;
        }
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;

        if let Some(delete_keys) = request.delete_keys.as_ref() {
            if recursive {
                return Err(AppError::Validation(
                    "explicit delete selections cannot be recursive".to_string(),
                ));
            }
            if delete_keys.is_empty() || delete_keys.len() > 10_000 {
                return Err(AppError::Validation(
                    "delete selection must contain between 1 and 10000 objects".to_string(),
                ));
            }
            let mut objects = Vec::with_capacity(delete_keys.len());
            for selected_key in delete_keys {
                authorize_key(&profile, &bucket, selected_key)?;
                let size_bytes = match client
                    .head_object()
                    .bucket(&bucket)
                    .key(selected_key)
                    .send()
                    .await
                {
                    Ok(head) => head
                        .content_length()
                        .and_then(|value| u64::try_from(value).ok()),
                    Err(error) if is_not_found_provider_error(&error) => None,
                    Err(error) => return Err(AppError::Provider(safe_provider_error(&error))),
                };
                objects.push(RemoteObject {
                    key: selected_key.clone(),
                    size_bytes,
                    is_folder_marker: selected_key.ends_with('/'),
                });
            }
            let expected = format!("DELETE {} OBJECTS", objects.len());
            return self
                .delete_prefix(
                    id,
                    &client,
                    &bucket,
                    "",
                    confirmation,
                    settings,
                    Some(objects),
                    Some(expected),
                )
                .await;
        }

        if recursive {
            return self
                .delete_prefix(
                    id,
                    &client,
                    &bucket,
                    &key,
                    confirmation,
                    settings,
                    None,
                    None,
                )
                .await;
        }

        client
            .delete_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        Ok(ExecutionOutcome::completed(0, 1))
    }

    /// Delete all objects below a prefix using S3's multi-object delete API.
    /// Listing remains paginated and every page/batch observes the transfer
    /// checkpoint so pause/cancel requests are honored between provider calls.
    #[allow(clippy::too_many_arguments)]
    async fn delete_prefix(
        &self,
        id: Uuid,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        prefix: &str,
        confirmation: &str,
        settings: &SettingsSnapshot,
        objects_override: Option<Vec<RemoteObject>>,
        typed_confirmation_override: Option<String>,
    ) -> Result<ExecutionOutcome, AppError> {
        let objects = match objects_override {
            Some(objects) => objects,
            None => self.list_remote_objects(id, client, bucket, prefix).await?,
        };
        if objects.is_empty() {
            return Ok(ExecutionOutcome::completed(0, 0));
        }
        let known_bytes = objects
            .iter()
            .filter_map(|object| object.size_bytes)
            .sum::<u64>();
        if (objects.len() as u64) > settings.typed_confirm_object_threshold
            || known_bytes > settings.typed_confirm_bytes_threshold
        {
            let expected = typed_confirmation_override
                .clone()
                .unwrap_or_else(|| format!("DELETE {}", prefix.trim_end_matches('/')));
            require_delete_confirmation(confirmation, Some(&expected))?;
        }
        let total_bytes = objects
            .iter()
            .map(|object| object.size_bytes)
            .collect::<Option<Vec<_>>>()
            .map(|sizes| sizes.into_iter().fold(0_u64, u64::saturating_add));
        self.manager
            .set_totals(id, Some(objects.len() as u64), total_bytes)
            .await?;
        // Keep a durable item record for every planned delete. This makes
        // Object Lock/retention failures actionable per object instead of
        // reducing a batch response to one aggregate warning.
        let item_snapshots = objects
            .iter()
            .map(|object| TransferItem {
                schema_version: crate::dto::transfer::DTO_SCHEMA_VERSION,
                id: object.key.clone(),
                source_key: Some(object.key.clone()),
                destination_key: None,
                local_path: None,
                size_bytes: object.size_bytes,
                status: TransferStatus::Queued,
                retry_count: 0,
                error: None,
            })
            .collect::<Vec<_>>();
        self.manager
            .replace_transfer_items(id, &item_snapshots)
            .await?;

        let mut completed_items = 0_u64;
        let mut failed_items = 0_u64;
        let mut completed_bytes = 0_u64;
        let mut failure_codes = Vec::<String>::new();
        let mut first_batch_error = None::<String>;

        for chunk in objects.chunks(1_000) {
            self.manager.checkpoint(id).await?;
            let identifiers = chunk
                .iter()
                .map(|object| {
                    ObjectIdentifier::builder()
                        .key(object.key.clone())
                        .build()
                        .map_err(|error| {
                            AppError::Validation(format!(
                                "delete request contains an invalid object key: {error}"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let delete = Delete::builder()
                .set_objects(Some(identifiers))
                .quiet(true)
                .build()
                .map_err(|error| {
                    AppError::Validation(format!("delete request is invalid: {error}"))
                })?;

            let output = match client
                .delete_objects()
                .bucket(bucket)
                .delete(delete)
                .send()
                .await
            {
                Ok(output) => output,
                Err(error) => {
                    // A batch-level provider error gives no per-key result;
                    // count the complete batch as failed and continue with
                    // later batches so a transient failure cannot hide work
                    // that did succeed.
                    failed_items = failed_items.saturating_add(chunk.len() as u64);
                    if first_batch_error.is_none() {
                        first_batch_error = Some(safe_provider_error(&error));
                    }
                    let public_error =
                        PublicError::from(AppError::Provider(safe_provider_error(&error)));
                    for object in chunk {
                        self.manager
                            .update_transfer_item(
                                id,
                                &object.key,
                                TransferStatus::Failed,
                                0,
                                Some(&public_error),
                                false,
                            )
                            .await?;
                    }
                    self.manager
                        .update_progress(id, completed_bytes, completed_items, None, None)
                        .await?;
                    continue;
                }
            };

            // Own the response details before awaiting item persistence. The
            // SDK output borrows its error slice, while the manager writes may
            // yield between each item.
            let errors = output
                .errors()
                .iter()
                .map(|error| {
                    (
                        error.key().map(ToString::to_string),
                        error.code().map(ToString::to_string),
                        error.message().map(ToString::to_string),
                    )
                })
                .collect::<Vec<_>>();
            let mut unmatched_errors = errors
                .iter()
                .filter(|(key, _, _)| {
                    key.as_deref()
                        .map(|key| !chunk.iter().any(|object| object.key == key))
                        .unwrap_or(true)
                })
                .count();
            for object in chunk {
                let matched_error = errors
                    .iter()
                    .find(|(error_key, _, _)| error_key.as_deref() == Some(object.key.as_str()))
                    .map(|(_, code, message)| (code.as_deref(), message.as_deref()));
                let object_error = match matched_error {
                    Some(value) => Some(value),
                    None if unmatched_errors > 0 => {
                        unmatched_errors -= 1;
                        Some((None, None))
                    }
                    None => None,
                };
                if let Some((code, message)) = object_error {
                    failed_items = failed_items.saturating_add(1);
                    let description = match (code, message) {
                        (Some(code), Some(message)) => {
                            format!("{code}: {message}")
                        }
                        (Some(code), None) => code.to_string(),
                        (None, Some(message)) => message.to_string(),
                        (None, None) => "provider rejected deletion".to_string(),
                    };
                    if let Some(code) = code {
                        if failure_codes.len() < 3 && !failure_codes.iter().any(|item| item == code)
                        {
                            failure_codes.push(code.to_string());
                        }
                    }
                    let public_error = PublicError::from(AppError::Provider(description));
                    self.manager
                        .update_transfer_item(
                            id,
                            &object.key,
                            TransferStatus::Failed,
                            0,
                            Some(&public_error),
                            false,
                        )
                        .await?;
                } else {
                    completed_items = completed_items.saturating_add(1);
                    completed_bytes =
                        completed_bytes.saturating_add(object.size_bytes.unwrap_or_default());
                    self.manager
                        .update_transfer_item(
                            id,
                            &object.key,
                            TransferStatus::Completed,
                            object.size_bytes.unwrap_or_default(),
                            None,
                            false,
                        )
                        .await?;
                }
            }
            self.manager
                .update_progress(id, completed_bytes, completed_items, None, None)
                .await?;
        }

        if failed_items == 0 {
            return Ok(ExecutionOutcome::completed(
                completed_bytes,
                completed_items,
            ));
        }

        let mut summary =
            format!("recursive delete completed with {failed_items} object failure(s)");
        if !failure_codes.is_empty() {
            summary.push_str(&format!(" ({})", failure_codes.join(", ")));
        }
        if let Some(error) = first_batch_error {
            summary.push_str(&format!("; batch request failed: {error}"));
        }
        // Preserve a concise, redacted diagnostic on the job while exposing
        // the machine-readable completed/failed counts through the result.
        let _ = self
            .manager
            .set_error(id, AppError::Provider(summary))
            .await;
        Ok(ExecutionOutcome {
            transferred_bytes: completed_bytes,
            completed_items,
            failed_items,
            cleanup_required_items: 0,
            status: TransferStatus::CompletedWithWarnings,
            skipped: false,
        })
    }

    async fn upload_directory(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let source_root = local_path(&request.source)?;
        let (profile, bucket, destination_prefix) =
            self.remote_target(request.destination.as_ref()).await?;
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let existing = self
            .list_remote_objects(id, &client, &bucket, &destination_prefix)
            .await?
            .into_iter()
            .map(|object| object.key)
            .collect::<HashSet<_>>();
        let plan = plan_upload_directory_with_options(
            &source_root,
            &profile.id.to_string(),
            &bucket,
            &destination_prefix,
            request.collision_policy,
            &existing,
            settings.preserve_empty_folders,
            request.preserve_root,
        )?;
        let executor = S3RecursiveExecutor {
            client,
            manager: self.manager.clone(),
            transfer_id: id,
            profile_id: Some(profile.id.to_string()),
            operation: TransferOperation::UploadDirectory,
            settings: settings.clone(),
            metadata: request.metadata.clone(),
            replace_metadata: false,
        };
        self.execute_recursive_plan(id, plan, executor).await
    }

    async fn download_prefix(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let (profile, bucket, source_prefix) = self.remote_target(Some(&request.source)).await?;
        let destination_root = local_path(request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("download destination is required".to_string())
        })?)?;
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let objects = self
            .list_remote_objects(id, &client, &bucket, &source_prefix)
            .await?;
        let plan = plan_download_prefix(
            &profile.id.to_string(),
            &bucket,
            &source_prefix,
            &destination_root,
            &objects,
            request.collision_policy,
            &HashSet::new(),
            settings.preserve_empty_folders,
        )?;
        let manifest_path = write_mapping_manifest(
            &destination_root,
            &profile.id.to_string(),
            &bucket,
            &source_prefix,
            &plan,
        )?;
        self.manager
            .set_mapping_manifest_path(id, manifest_path.to_string_lossy().to_string())
            .await?;
        let executor = S3RecursiveExecutor {
            client,
            manager: self.manager.clone(),
            transfer_id: id,
            profile_id: Some(profile.id.to_string()),
            operation: TransferOperation::DownloadPrefix,
            settings: settings.clone(),
            metadata: None,
            replace_metadata: false,
        };
        self.execute_recursive_plan(id, plan, executor).await
    }

    async fn copy_prefix(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        self.remote_prefix_operation(id, request, TransferOperation::CopyPrefix, settings)
            .await
    }

    async fn move_prefix(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        self.remote_prefix_operation(id, request, TransferOperation::MovePrefix, settings)
            .await
    }

    async fn remote_prefix_operation(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        operation: TransferOperation,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        let (source_profile, bucket, source_prefix) =
            self.remote_target(Some(&request.source)).await?;
        let destination = request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("recursive copy/move destination is required".to_string())
        })?;
        let (destination_profile, destination_bucket, destination_prefix) =
            self.remote_target(Some(destination)).await?;
        if source_profile.id != destination_profile.id || bucket != destination_bucket {
            return Err(AppError::UnsupportedProviderFeature(
                "cross-profile or cross-bucket recursive copy is not supported".to_string(),
            ));
        }
        if source_prefix == destination_prefix {
            return Err(AppError::Validation(
                "recursive copy/move source and destination must differ".to_string(),
            ));
        }
        let credentials =
            resolve_profile_credentials(self.credentials.as_ref(), &source_profile).await?;
        let client = self
            .clients
            .get_or_create(&source_profile, &credentials)
            .await?;
        let objects = self
            .list_remote_objects(id, &client, &bucket, &source_prefix)
            .await?;
        let existing = self
            .list_remote_objects(id, &client, &bucket, &destination_prefix)
            .await?
            .into_iter()
            .map(|object| object.key)
            .collect::<HashSet<_>>();
        let plan = plan_remote_prefix(
            operation,
            &source_profile.id.to_string(),
            &bucket,
            &source_prefix,
            &destination_prefix,
            &objects,
            request.collision_policy,
            &existing,
        )?;
        let executor = S3RecursiveExecutor {
            client,
            manager: self.manager.clone(),
            transfer_id: id,
            profile_id: Some(source_profile.id.to_string()),
            operation,
            settings: settings.clone(),
            metadata: request.metadata.clone(),
            replace_metadata: request.replace_metadata,
        };
        self.execute_recursive_plan(id, plan, executor).await
    }

    async fn list_remote_objects(
        &self,
        id: Uuid,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<RemoteObject>, AppError> {
        let mut token = None;
        let mut objects = Vec::new();
        loop {
            self.manager.checkpoint(id).await?;
            let mut operation = client.list_objects_v2().bucket(bucket).prefix(prefix);
            if let Some(token) = &token {
                operation = operation.continuation_token(token);
            }
            let page = operation
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            for object in page.contents() {
                if let Some(key) = object.key() {
                    objects.push(RemoteObject {
                        key: key.to_string(),
                        size_bytes: object.size().map(|size| size.max(0) as u64),
                        is_folder_marker: key.ends_with('/'),
                    });
                }
            }
            token = page.next_continuation_token().map(ToString::to_string);
            if token.is_none() {
                break;
            }
            if objects.len() > 100_000 {
                return Err(AppError::Validation(
                    "recursive transfer contains more than 100000 items".to_string(),
                ));
            }
        }
        Ok(objects)
    }

    async fn execute_recursive_plan<E: RecursiveExecutor>(
        &self,
        id: Uuid,
        plan: RecursivePlan,
        executor: E,
    ) -> Result<ExecutionOutcome, AppError> {
        let item_snapshots = plan
            .items
            .iter()
            .map(recursive_item_snapshot)
            .collect::<Vec<_>>();
        if let Err(error) = self
            .manager
            .replace_transfer_items(id, &item_snapshots)
            .await
        {
            tracing::warn!(transfer_id = %id, error = %error, "unable to persist recursive transfer plan");
        }
        let cancellation = CancellationFlag::default();
        let monitor_cancellation = cancellation.clone();
        let monitor_manager = self.manager.clone();
        let cancellation_monitor = tokio::spawn(async move {
            loop {
                if monitor_manager.is_cancel_requested(id).await {
                    monitor_cancellation.cancel();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        });
        let (sender, mut receiver) =
            mpsc::channel::<crate::transfer::recursive::RecursiveProgress>(32);
        let manager = self.manager.clone();
        let progress_task = tokio::spawn(async move {
            while let Some(progress) = receiver.recv().await {
                let _ = manager
                    .update_progress(
                        id,
                        progress.transferred_bytes,
                        progress.completed_items,
                        None,
                        None,
                    )
                    .await;
            }
        });
        let result = execute_recursive(&plan, &executor, &cancellation, Some(sender)).await;
        cancellation_monitor.abort();
        let _ = progress_task.await;
        for item in &plan.items {
            let failure = result
                .failures
                .iter()
                .find(|failure| failure.item_id == item.id);
            let (status, bytes, error, cleanup_required) = match failure {
                Some(failure) => (
                    if failure.cleanup_required {
                        TransferStatus::CompletedWithWarnings
                    } else {
                        TransferStatus::Failed
                    },
                    0,
                    Some(&failure.error),
                    failure.cleanup_required,
                ),
                None if item.collision == crate::transfer::recursive::CollisionResolution::Skip => {
                    (TransferStatus::Completed, 0, None, false)
                }
                None => (
                    TransferStatus::Completed,
                    item.size_bytes.unwrap_or_default(),
                    None,
                    false,
                ),
            };
            if let Err(persist_error) = self
                .manager
                .update_transfer_item(id, &item.id, status, bytes, error, cleanup_required)
                .await
            {
                tracing::warn!(transfer_id = %id, item_id = %item.id, error = %persist_error, "unable to persist recursive transfer item result");
            }
        }
        Ok(ExecutionOutcome {
            transferred_bytes: result.transferred_bytes,
            completed_items: result.completed_items,
            failed_items: result.failed_items,
            cleanup_required_items: result.cleanup_required_items,
            status: result.status,
            skipped: false,
        })
    }

    async fn remote_target(
        &self,
        endpoint: Option<&TransferEndpoint>,
    ) -> Result<(ConnectionProfile, String, String), AppError> {
        let TransferEndpoint::Remote {
            profile_id,
            bucket,
            key,
        } = endpoint
            .ok_or_else(|| AppError::Validation("remote endpoint is required".to_string()))?
        else {
            return Err(AppError::Validation(
                "remote endpoint is required".to_string(),
            ));
        };
        let profile = self.profiles.get_connection_profile(profile_id).await?;
        authorize_key(&profile, bucket, key)?;
        Ok((profile, bucket.clone(), key.clone()))
    }
}

fn recursive_item_snapshot(item: &RecursiveItem) -> TransferItem {
    let source_key = match &item.source {
        TransferEndpoint::Remote { key, .. } => Some(key.clone()),
        TransferEndpoint::Local { path } => Some(path.clone()),
    };
    let destination_key = match &item.destination {
        TransferEndpoint::Remote { key, .. } => Some(key.clone()),
        TransferEndpoint::Local { .. } => None,
    };
    let local_path = match (&item.source, &item.destination) {
        (TransferEndpoint::Local { path }, _) => Some(path.clone()),
        (_, TransferEndpoint::Local { path }) => Some(path.clone()),
        _ => None,
    };
    TransferItem {
        schema_version: crate::dto::transfer::DTO_SCHEMA_VERSION,
        id: item.id.clone(),
        source_key,
        destination_key,
        local_path,
        size_bytes: item.size_bytes,
        status: TransferStatus::Queued,
        retry_count: 0,
        error: None,
    }
}

struct S3RecursiveExecutor {
    client: Arc<aws_sdk_s3::Client>,
    manager: Arc<TransferManager>,
    transfer_id: Uuid,
    profile_id: Option<String>,
    operation: TransferOperation,
    settings: SettingsSnapshot,
    metadata: Option<UploadMetadata>,
    replace_metadata: bool,
}

#[async_trait]
impl RecursiveExecutor for S3RecursiveExecutor {
    async fn execute_item(
        &self,
        item: &RecursiveItem,
        cancellation: &CancellationFlag,
    ) -> Result<u64, AppError> {
        if cancellation.is_cancelled() {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        self.manager.checkpoint(self.transfer_id).await?;
        match self.operation {
            TransferOperation::UploadDirectory => self.upload_item(item).await,
            TransferOperation::DownloadPrefix => self.download_item(item).await,
            TransferOperation::CopyPrefix | TransferOperation::MovePrefix => {
                self.copy_item(item).await
            }
            _ => Err(AppError::Validation(
                "invalid recursive operation".to_string(),
            )),
        }
    }

    async fn delete_source(
        &self,
        item: &RecursiveItem,
        cancellation: &CancellationFlag,
    ) -> Result<(), AppError> {
        if cancellation.is_cancelled() {
            return Err(AppError::TransferStateConflict(
                "transfer cancelled".to_string(),
            ));
        }
        let TransferEndpoint::Remote { bucket, key, .. } = &item.source else {
            return Err(AppError::Validation(
                "move source must be remote".to_string(),
            ));
        };
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        Ok(())
    }
}

impl S3RecursiveExecutor {
    async fn upload_item(&self, item: &RecursiveItem) -> Result<u64, AppError> {
        let TransferEndpoint::Local { path } = &item.source else {
            return Err(AppError::Validation(
                "recursive upload source must be local".to_string(),
            ));
        };
        let TransferEndpoint::Remote {
            profile_id,
            bucket,
            key,
        } = &item.destination
        else {
            return Err(AppError::Validation(
                "recursive upload destination must be remote".to_string(),
            ));
        };
        if item.is_directory {
            self.client
                .put_object()
                .bucket(bucket)
                .key(key)
                .content_length(0)
                .body(ByteStream::from(Vec::<u8>::new()))
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            return Ok(0);
        }
        let metadata = fs::metadata(path).await?;
        if !metadata.is_file() {
            return Err(AppError::Validation(
                "recursive upload source is not a regular file".to_string(),
            ));
        }
        let size = metadata.len();
        let modified = metadata.modified().ok();
        if size >= self.settings.multipart_threshold_bytes {
            self.upload_multipart_item(profile_id, path, bucket, key, size, self.metadata.as_ref())
                .await?;
            verify_local_upload_snapshot(Path::new(path), size, modified).await?;
            return Ok(size);
        }
        let body = ByteStream::from_path(path)
            .await
            .map_err(|error| AppError::Provider(format!("local upload stream: {error}")))?;
        let mut upload = self
            .client
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_length(size as i64);
        if let Some(value) = upload_content_type(Path::new(path), self.metadata.as_ref()) {
            upload = upload.content_type(value);
        }
        if let Some(metadata) = self.metadata.as_ref() {
            if let Some(value) = metadata.content_disposition.as_deref() {
                upload = upload.content_disposition(value);
            }
            if let Some(value) = metadata.cache_control.as_deref() {
                upload = upload.cache_control(value);
            }
            for (name, value) in &metadata.user_metadata {
                upload = upload.metadata(name, value);
            }
        }
        upload
            .body(body)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        verify_local_upload_snapshot(Path::new(path), size, modified).await?;
        Ok(size)
    }

    async fn upload_multipart_item(
        &self,
        profile_id: &str,
        path: &str,
        bucket: &str,
        key: &str,
        size: u64,
        metadata: Option<&UploadMetadata>,
    ) -> Result<(), AppError> {
        let part_size = effective_multipart_part_size(self.settings.initial_part_size_bytes, size)?;
        let mut upload_builder = self
            .client
            .create_multipart_upload()
            .bucket(bucket)
            .key(key);
        if let Some(value) = upload_content_type(Path::new(path), metadata) {
            upload_builder = upload_builder.content_type(value);
        }
        if let Some(metadata) = metadata {
            if let Some(value) = metadata.content_disposition.as_deref() {
                upload_builder = upload_builder.content_disposition(value);
            }
            if let Some(value) = metadata.cache_control.as_deref() {
                upload_builder = upload_builder.cache_control(value);
            }
            for (name, value) in &metadata.user_metadata {
                upload_builder = upload_builder.metadata(name, value);
            }
        }
        let upload = upload_builder
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();
        if let Err(error) = self
            .manager
            .persist_multipart_upload(
                self.transfer_id,
                Some(profile_id),
                bucket,
                key,
                &upload_id,
                part_size,
            )
            .await
        {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            return Err(error);
        }
        let result = async {
            let mut file = File::open(path).await?;
            let mut parts = Vec::new();
            let mut part_number = 1_i32;
            loop {
                self.manager.checkpoint(self.transfer_id).await?;
                let mut buffer = vec![0_u8; part_size as usize];
                let mut read = 0_usize;
                while read < buffer.len() {
                    let count = file.read(&mut buffer[read..]).await?;
                    if count == 0 {
                        break;
                    }
                    read += count;
                }
                if read == 0 {
                    break;
                }
                buffer.truncate(read);
                let output = self
                    .client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(part_number)
                    .body(ByteStream::from(buffer))
                    .send()
                    .await
                    .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                let etag = output.e_tag().ok_or_else(|| {
                    AppError::Provider("provider did not return a part ETag".to_string())
                })?;
                self.manager
                    .persist_multipart_part(
                        self.transfer_id,
                        u32::try_from(part_number).map_err(|_| {
                            AppError::Unknown("multipart part number overflow".to_string())
                        })?,
                        etag,
                        read as u64,
                    )
                    .await?;
                parts.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(etag)
                        .build(),
                );
                part_number += 1;
            }
            self.client
                .complete_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                )
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
            if let Err(error) = self.manager.clear_multipart_upload(self.transfer_id).await {
                tracing::warn!(transfer_id = %self.transfer_id, error = %error, "unable to clear aborted recursive multipart checkpoint");
            }
        } else if let Err(error) = self.manager.clear_multipart_upload(self.transfer_id).await {
            tracing::warn!(transfer_id = %self.transfer_id, error = %error, "unable to clear completed recursive multipart checkpoint");
        }
        result
    }

    async fn download_item(&self, item: &RecursiveItem) -> Result<u64, AppError> {
        let TransferEndpoint::Remote { bucket, key, .. } = &item.source else {
            return Err(AppError::Validation(
                "recursive download source must be remote".to_string(),
            ));
        };
        let TransferEndpoint::Local { path } = &item.destination else {
            return Err(AppError::Validation(
                "recursive download destination must be local".to_string(),
            ));
        };
        let destination = PathBuf::from(path);
        ensure_local_path_not_reparse(&destination)?;
        if item.is_directory {
            fs::create_dir_all(&destination).await?;
            return Ok(0);
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
            ensure_local_path_not_reparse(&destination)?;
        }
        let head = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let expected = head.content_length().unwrap_or(0).max(0) as u64;
        let remote_etag = head.e_tag().map(ToString::to_string);
        if item.size_bytes.is_some_and(|planned| planned != expected) {
            return Err(AppError::Provider(
                "download integrity check failed: object size changed".to_string(),
            ));
        }
        let partial = partial_path(&destination, self.transfer_id);
        let result = async {
            let mut transferred = fs::metadata(&partial)
                .await
                .map(|meta| meta.len())
                .unwrap_or(0);
            if transferred > expected {
                transferred = 0;
                let _ = fs::remove_file(&partial).await;
            }
            loop {
                let _ = self.manager.checkpoint(self.transfer_id).await?;
                if transferred == expected {
                    break;
                }
                let mut request = self.client.get_object().bucket(bucket).key(key);
                if transferred > 0 {
                    request = request.range(format!("bytes={transferred}-"));
                }
                let output = request
                    .send()
                    .await
                    .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                let response_size = output
                    .content_length()
                    .and_then(|value| u64::try_from(value).ok())
                    .unwrap_or_default();
                if remote_etag.as_deref() != output.e_tag() {
                    return Err(AppError::Provider(
                        "download integrity check failed: remote object identity changed"
                            .to_string(),
                    ));
                }
                if transferred.saturating_add(response_size) != expected {
                    return Err(AppError::Provider(
                        "download integrity check failed: range response changed".to_string(),
                    ));
                }
                let mut file = if transferred > 0 {
                    fs::OpenOptions::new().append(true).open(&partial).await?
                } else {
                    File::create(&partial).await?
                };
                let mut stream = output.body.into_async_read();
                let mut buffer = vec![0_u8; 1024 * 1024];
                let mut paused = false;
                loop {
                    if self.manager.checkpoint(self.transfer_id).await? {
                        paused = true;
                        break;
                    }
                    let count = stream.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    file.write_all(&buffer[..count]).await?;
                    transferred = transferred.saturating_add(count as u64);
                }
                file.flush().await?;
                drop(file);
                if paused {
                    // Resume from the validated local offset with a fresh
                    // Range request rather than continuing a stale stream.
                    continue;
                }
                if transferred != expected {
                    return Err(AppError::Provider(
                        "download integrity check failed: content length mismatch".to_string(),
                    ));
                }
            }
            ensure_local_path_not_reparse(&destination)?;
            if destination.exists() {
                fs::remove_file(&destination).await?;
            }
            fs::rename(&partial, &destination).await?;
            Ok::<u64, AppError>(transferred)
        }
        .await;
        if result.is_err() && !self.settings.keep_partial_downloads {
            let _ = fs::remove_file(&partial).await;
        }
        result
    }

    async fn copy_item(&self, item: &RecursiveItem) -> Result<u64, AppError> {
        let TransferEndpoint::Remote {
            bucket: source_bucket,
            key: source_key,
            ..
        } = &item.source
        else {
            return Err(AppError::Validation(
                "recursive copy source must be remote".to_string(),
            ));
        };
        let TransferEndpoint::Remote {
            bucket: destination_bucket,
            key: destination_key,
            ..
        } = &item.destination
        else {
            return Err(AppError::Validation(
                "recursive copy destination must be remote".to_string(),
            ));
        };
        if source_bucket == destination_bucket && source_key == destination_key {
            return Err(AppError::TransferStateConflict(
                "copy source and destination must differ".to_string(),
            ));
        }
        let source = self
            .client
            .head_object()
            .bucket(source_bucket)
            .key(source_key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let source_size = source.content_length().unwrap_or(0).max(0) as u64;
        let source_etag = source.e_tag().map(ToString::to_string);
        let multipart_copy = source_size > SINGLE_COPY_LIMIT_BYTES;
        if multipart_copy {
            self.copy_multipart_item(
                source_bucket,
                source_key,
                destination_bucket,
                destination_key,
                source_size,
                &source,
                self.replace_metadata,
                self.metadata.as_ref(),
            )
            .await?;
        } else {
            let mut copy = self
                .client
                .copy_object()
                .copy_source(encode_copy_source(source_bucket, source_key))
                .bucket(destination_bucket)
                .key(destination_key);
            if self.replace_metadata {
                copy = copy.metadata_directive(MetadataDirective::Replace);
                if let Some(metadata) = self.metadata.as_ref() {
                    if let Some(value) = metadata.content_type.as_deref() {
                        copy = copy.content_type(value);
                    }
                    if let Some(value) = metadata.content_disposition.as_deref() {
                        copy = copy.content_disposition(value);
                    }
                    if let Some(value) = metadata.cache_control.as_deref() {
                        copy = copy.cache_control(value);
                    }
                    for (name, value) in &metadata.user_metadata {
                        copy = copy.metadata(name, value);
                    }
                }
            }
            copy.send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        }
        self.manager.checkpoint(self.transfer_id).await?;
        let destination = self
            .client
            .head_object()
            .bucket(destination_bucket)
            .key(destination_key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let destination_size = destination.content_length().unwrap_or(0).max(0) as u64;
        if source_size != destination_size
            || (!multipart_copy
                && source_etag.is_some()
                && destination.e_tag().map(str::to_string) != source_etag)
            || if self.replace_metadata {
                !replacement_metadata_matches(&destination, self.metadata.as_ref())
            } else {
                !metadata_matches(&source, &destination)
            }
        {
            return Err(AppError::Provider("copy verification failed".to_string()));
        }
        Ok(source_size)
    }

    #[allow(clippy::too_many_arguments)]
    async fn copy_multipart_item(
        &self,
        source_bucket: &str,
        source_key: &str,
        destination_bucket: &str,
        destination_key: &str,
        source_size: u64,
        source_head: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
        replace_metadata: bool,
        replacement: Option<&UploadMetadata>,
    ) -> Result<(), AppError> {
        let part_size = effective_multipart_part_size(MULTIPART_COPY_PART_BYTES, source_size)?;
        let ranges = multipart_copy_ranges(source_size)?;
        let mut create = self
            .client
            .create_multipart_upload()
            .bucket(destination_bucket)
            .key(destination_key);
        if !replace_metadata {
            if let Some(value) = source_head.content_type() {
                create = create.content_type(value);
            }
            if let Some(value) = source_head.content_disposition() {
                create = create.content_disposition(value);
            }
            if let Some(value) = source_head.cache_control() {
                create = create.cache_control(value);
            }
            if let Some(value) = source_head.content_encoding() {
                create = create.content_encoding(value);
            }
            if let Some(value) = source_head.content_language() {
                create = create.content_language(value);
            }
            #[allow(deprecated)]
            if let Some(value) = source_head.expires().cloned() {
                create = create.expires(value);
            }
            if let Some(value) = source_head.website_redirect_location() {
                create = create.website_redirect_location(value);
            }
            if let Some(metadata) = source_head.metadata() {
                for (key, value) in metadata {
                    create = create.metadata(key, value);
                }
            }
        } else if let Some(metadata) = replacement {
            if let Some(value) = metadata.content_type.as_deref() {
                create = create.content_type(value);
            }
            if let Some(value) = metadata.content_disposition.as_deref() {
                create = create.content_disposition(value);
            }
            if let Some(value) = metadata.cache_control.as_deref() {
                create = create.cache_control(value);
            }
            for (key, value) in &metadata.user_metadata {
                create = create.metadata(key, value);
            }
        }
        let upload = create
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();
        if let Err(error) = self
            .manager
            .persist_multipart_upload(
                self.transfer_id,
                self.profile_id.as_deref(),
                destination_bucket,
                destination_key,
                &upload_id,
                part_size,
            )
            .await
        {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .send()
                .await;
            return Err(error);
        }
        let source = encode_copy_source(source_bucket, source_key);
        let result = async {
            let part_concurrency = usize::from(self.settings.per_job_part_concurrency.clamp(1, 16));
            let permits = Arc::new(Semaphore::new(part_concurrency));
            let mut in_flight = JoinSet::new();
            let mut parts = BTreeMap::<i32, (String, u64)>::new();
            let mut transferred = 0_u64;
            for (part_number, start, end) in ranges {
                self.manager.checkpoint(self.transfer_id).await?;
                let bytes = end.saturating_sub(start).saturating_add(1);
                let permit = permits.clone().acquire_owned().await.map_err(|_| {
                    AppError::TransferStateConflict(
                        "multipart copy scheduler is unavailable".to_string(),
                    )
                })?;
                let copy_client = self.client.clone();
                let copy_bucket = destination_bucket.to_string();
                let copy_key = destination_key.to_string();
                let copy_upload_id = upload_id.clone();
                let copy_source = source.clone();
                in_flight.spawn(async move {
                    let _permit = permit;
                    let output = copy_client
                        .upload_part_copy()
                        .bucket(copy_bucket)
                        .key(copy_key)
                        .upload_id(copy_upload_id)
                        .part_number(part_number)
                        .copy_source(copy_source)
                        .copy_source_range(format!("bytes={start}-{end}"))
                        .send()
                        .await
                        .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
                    let etag = output
                        .copy_part_result()
                        .and_then(|result| result.e_tag())
                        .map(ToString::to_string)
                        .ok_or_else(|| {
                            AppError::Provider(
                                "provider did not return a copy part ETag".to_string(),
                            )
                        })?;
                    Ok::<(i32, String, u64), AppError>((part_number, etag, bytes))
                });
                if in_flight.len() >= part_concurrency {
                    let joined = in_flight.join_next().await.ok_or_else(|| {
                        AppError::Unknown("multipart copy queue ended early".to_string())
                    })?;
                    let (part, etag, size_bytes) = joined.map_err(|error| {
                        AppError::Unknown(format!("multipart copy task failed: {error}"))
                    })??;
                    self.manager
                        .persist_multipart_part(
                            self.transfer_id,
                            u32::try_from(part).map_err(|_| {
                                AppError::Unknown("multipart part number overflow".to_string())
                            })?,
                            &etag,
                            size_bytes,
                        )
                        .await?;
                    parts.insert(part, (etag, size_bytes));
                    transferred = transferred.saturating_add(size_bytes);
                    self.manager
                        .update_progress(self.transfer_id, transferred, 0, None, None)
                        .await?;
                }
            }
            while !in_flight.is_empty() {
                let joined = in_flight.join_next().await.ok_or_else(|| {
                    AppError::Unknown("multipart copy queue ended early".to_string())
                })?;
                let (part, etag, size_bytes) = joined.map_err(|error| {
                    AppError::Unknown(format!("multipart copy task failed: {error}"))
                })??;
                self.manager
                    .persist_multipart_part(
                        self.transfer_id,
                        u32::try_from(part).map_err(|_| {
                            AppError::Unknown("multipart part number overflow".to_string())
                        })?,
                        &etag,
                        size_bytes,
                    )
                    .await?;
                parts.insert(part, (etag, size_bytes));
                transferred = transferred.saturating_add(size_bytes);
                self.manager
                    .update_progress(self.transfer_id, transferred, 0, None, None)
                    .await?;
            }
            self.client
                .complete_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(
                            parts
                                .iter()
                                .map(|(part, (etag, _))| {
                                    CompletedPart::builder()
                                        .part_number(*part)
                                        .e_tag(etag.clone())
                                        .build()
                                })
                                .collect(),
                        ))
                        .build(),
                )
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(destination_bucket)
                .key(destination_key)
                .upload_id(&upload_id)
                .send()
                .await;
            if let Err(error) = self.manager.clear_multipart_upload(self.transfer_id).await {
                tracing::warn!(transfer_id = %self.transfer_id, error = %error, "unable to clear aborted recursive multipart copy checkpoint");
            }
        } else if let Err(error) = self.manager.clear_multipart_upload(self.transfer_id).await {
            tracing::warn!(transfer_id = %self.transfer_id, error = %error, "unable to clear completed recursive multipart copy checkpoint");
        }
        result
    }
}

fn authorize_request(request: &StartTransferRequest) -> Result<(), AppError> {
    let requires_object_key = !matches!(
        request.operation,
        TransferOperation::UploadDirectory
            | TransferOperation::DownloadPrefix
            | TransferOperation::CopyPrefix
            | TransferOperation::MovePrefix
    );
    for endpoint in [
        &request.source,
        request.destination.as_ref().unwrap_or(&request.source),
    ] {
        match endpoint {
            TransferEndpoint::Local { path } => {
                if path.is_empty()
                    || path.contains('\0')
                    || !is_absolute_local_path(path)
                    || path.split(['/', '\\']).any(|part| part == "..")
                {
                    return Err(AppError::Validation("local path is unsafe".to_string()));
                }
                validate_local_path_length(Path::new(path))?;
            }
            TransferEndpoint::Remote { bucket, key, .. } => {
                if !valid_bucket_name(bucket)
                    || requires_object_key && key.trim().is_empty()
                    || key.len() > 1_024
                    || key.contains('\0')
                    || key.contains('\\')
                    || key.split('/').any(|part| part == "..")
                {
                    return Err(AppError::Validation(
                        "remote object reference is invalid".to_string(),
                    ));
                }
            }
        }
    }
    if request.operation == TransferOperation::DeleteObjects
        && !request
            .confirmation
            .as_deref()
            .is_some_and(|value| value == "DELETE" || value.starts_with("DELETE "))
    {
        return Err(AppError::Validation(
            "delete operations require confirmation".to_string(),
        ));
    }
    Ok(())
}

fn authorize_key(profile: &ConnectionProfile, bucket: &str, key: &str) -> Result<(), AppError> {
    if !valid_bucket_name(bucket)
        || key.contains('\0')
        || key.contains('\\')
        || key.split('/').any(|part| part == "..")
    {
        return Err(AppError::Validation(
            "remote object reference is invalid".to_string(),
        ));
    }
    if let Some(root) = &profile.root_prefix {
        if !key.starts_with(root) {
            return Err(AppError::RootPrefixViolation);
        }
    }
    Ok(())
}

fn valid_bucket_name(bucket: &str) -> bool {
    let length = bucket.len();
    (3..=255).contains(&length)
        && !bucket.chars().any(char::is_control)
        && !bucket.contains(['/', '\\'])
}

fn local_path(endpoint: &TransferEndpoint) -> Result<PathBuf, AppError> {
    let TransferEndpoint::Local { path } = endpoint else {
        return Err(AppError::Validation(
            "local endpoint is required".to_string(),
        ));
    };
    if path.is_empty()
        || path.contains('\0')
        || !is_absolute_local_path(path)
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(AppError::Validation("local path is invalid".to_string()));
    }
    validate_local_path_length(Path::new(path))?;
    Ok(PathBuf::from(path))
}

fn local_endpoint_has_directory_hint(endpoint: &TransferEndpoint) -> bool {
    matches!(
        endpoint,
        TransferEndpoint::Local { path }
            if path.ends_with('/') || path.ends_with('\\')
    )
}

fn ensure_local_path_not_reparse(path: &Path) -> Result<(), AppError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                return Err(AppError::Validation(
                    "download destination contains a symbolic link or reparse point".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn check_available_disk_space(directory: &Path, required_bytes: u64) -> Result<(), AppError> {
    let available = fs2::available_space(directory).map_err(AppError::Io)?;
    if available < required_bytes {
        return Err(AppError::LocalDiskFull);
    }
    Ok(())
}

fn is_absolute_local_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || value.starts_with("\\\\")
        || value.starts_with("//")
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\')
            && value.as_bytes()[0].is_ascii_alphabetic())
}

async fn verify_local_upload_snapshot(
    path: &Path,
    expected_size: u64,
    expected_modified: Option<SystemTime>,
) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file()
        || metadata.len() != expected_size
        || expected_modified.is_some_and(|expected| metadata.modified().ok() != Some(expected))
    {
        return Err(AppError::LocalFileChanged);
    }
    Ok(())
}

fn partial_path(destination: &Path, id: Uuid) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    destination.with_file_name(format!("{name}.s3fm-partial-{id}"))
}

fn unique_local_destination(destination: &Path) -> Result<PathBuf, AppError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let stem = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("download");
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for index in 1..=10_000_u32 {
        let candidate = parent.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Validation(
        "could not find a free renamed destination".to_string(),
    ))
}

async fn ensure_collision(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    policy: CollisionPolicy,
) -> Result<(), AppError> {
    if policy == CollisionPolicy::Replace {
        return Ok(());
    }
    let exists = match client.head_object().bucket(bucket).key(key).send().await {
        Ok(_) => true,
        Err(error) if is_not_found_provider_error(&error) => false,
        Err(error) => return Err(AppError::Provider(safe_provider_error(&error))),
    };
    if !exists {
        return Ok(());
    }
    match policy {
        CollisionPolicy::Skip => Err(AppError::TransferStateConflict(
            "destination skipped".to_string(),
        )),
        CollisionPolicy::Ask | CollisionPolicy::Fail | CollisionPolicy::Rename => Err(
            AppError::Validation("destination already exists".to_string()),
        ),
        CollisionPolicy::Replace => Ok(()),
    }
}

async fn unique_remote_key(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<String, AppError> {
    if !object_exists(client, bucket, key).await? {
        return Ok(key.to_string());
    }
    let (parent, filename) = key.rsplit_once('/').unwrap_or(("", key));
    let (stem, extension) = filename
        .rsplit_once('.')
        .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
        .map(|(stem, extension)| (stem, format!(".{extension}")))
        .unwrap_or((filename, String::new()));
    for index in 1..=10_000_u32 {
        let candidate_name = format!("{stem} ({index}){extension}");
        let candidate = if parent.is_empty() {
            candidate_name
        } else {
            format!("{parent}/{candidate_name}")
        };
        if !object_exists(client, bucket, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(AppError::Validation(
        "could not find a free renamed object key".to_string(),
    ))
}

async fn prefix_has_objects(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<bool, AppError> {
    let output = client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .max_keys(1)
        .send()
        .await
        .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
    Ok(!output.contents().is_empty() || !output.common_prefixes().is_empty())
}

async fn object_exists(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<bool, AppError> {
    match client.head_object().bucket(bucket).key(key).send().await {
        Ok(_) => Ok(true),
        Err(error) if is_not_found_provider_error(&error) => Ok(false),
        Err(error) => Err(AppError::Provider(safe_provider_error(&error))),
    }
}

fn metadata_matches(
    source: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
    destination: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
) -> bool {
    optional_metadata_value_matches(source.content_type(), destination.content_type())
        && optional_metadata_value_matches(
            source.content_disposition(),
            destination.content_disposition(),
        )
        && optional_metadata_value_matches(source.cache_control(), destination.cache_control())
        && optional_metadata_value_matches(
            source.content_encoding(),
            destination.content_encoding(),
        )
        && optional_metadata_value_matches(
            source.content_language(),
            destination.content_language(),
        )
        && optional_metadata_value_matches(source.expires_string(), destination.expires_string())
        && optional_metadata_value_matches(
            source.website_redirect_location(),
            destination.website_redirect_location(),
        )
        && user_metadata_matches(source.metadata(), destination.metadata())
}

fn replacement_metadata_matches(
    destination: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
    replacement: Option<&UploadMetadata>,
) -> bool {
    let Some(replacement) = replacement else {
        return destination
            .metadata()
            .map(|values| values.is_empty())
            .unwrap_or(true);
    };
    replacement
        .content_type
        .as_deref()
        .map(|value| destination.content_type() == Some(value))
        .unwrap_or(true)
        && replacement
            .content_disposition
            .as_deref()
            .map(|value| destination.content_disposition() == Some(value))
            .unwrap_or(true)
        && replacement
            .cache_control
            .as_deref()
            .map(|value| destination.cache_control() == Some(value))
            .unwrap_or(true)
        && destination
            .metadata()
            .map(|values| {
                values.len() == replacement.user_metadata.len()
                    && replacement
                        .user_metadata
                        .iter()
                        .all(|(key, value)| values.get(key) == Some(value))
            })
            .unwrap_or(replacement.user_metadata.is_empty())
}

fn checksums_match(
    source: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
    destination: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
) -> bool {
    optional_checksum_matches(source.checksum_crc32(), destination.checksum_crc32())
        && optional_checksum_matches(source.checksum_sha1(), destination.checksum_sha1())
        && optional_checksum_matches(source.checksum_sha256(), destination.checksum_sha256())
}

fn optional_checksum_matches(source: Option<&str>, destination: Option<&str>) -> bool {
    source.is_none() || destination.is_none() || source == destination
}

fn optional_metadata_value_matches(source: Option<&str>, destination: Option<&str>) -> bool {
    source.is_none() || source == destination
}

fn user_metadata_matches(
    source: Option<&std::collections::HashMap<String, String>>,
    destination: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    match source {
        None => true,
        Some(values) if values.is_empty() => {
            destination.map(|values| values.is_empty()).unwrap_or(true)
        }
        Some(values) => destination == Some(values),
    }
}

/// Infer a conservative HTTP Content-Type when the caller did not provide
/// one explicitly. Explicit metadata always wins; unknown extensions are left
/// to the provider's default rather than guessing an active type.
fn upload_content_type(path: &Path, metadata: Option<&UploadMetadata>) -> Option<String> {
    metadata
        .and_then(|value| value.content_type.clone())
        .or_else(|| infer_upload_content_type(path).map(ToString::to_string))
}

fn infer_upload_content_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "log" | "conf" | "cfg" | "properties" | "env" => Some("text/plain"),
        "json" | "jsonl" => Some("application/json"),
        "xml" => Some("application/xml"),
        "md" | "markdown" => Some("text/markdown"),
        "csv" => Some("text/csv"),
        "yaml" | "yml" => Some("application/yaml"),
        "toml" => Some("application/toml"),
        "html" | "htm" => Some("text/html"),
        "css" => Some("text/css"),
        "js" | "jsx" | "ts" | "tsx" => Some("text/javascript"),
        "svg" => Some("image/svg+xml"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" => Some("audio/ogg"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

fn safe_provider_error(error: &impl std::fmt::Display) -> String {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    if is_credential_expired_message(&message) {
        return "credential expired".to_string();
    }
    if [
        "authorization",
        "access key",
        "secret",
        "security token",
        "signature",
        "presign",
        "x-amz-credential",
        "x-amz-security-token",
        "x-amz-algorithm",
        "x-amz-date",
        "http://",
        "https://",
        "akia",
        "asia",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "provider request failed (sensitive details redacted)".to_string();
    }
    message.chars().take(240).collect::<String>()
}

fn is_not_found_provider_error(error: &impl std::fmt::Display) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("nosuchkey")
        || message.contains("notfound")
        || message.contains("not found")
        || message.contains("status code: 404")
        || message.contains("statuscode: 404")
}

fn encode_copy_source(bucket: &str, key: &str) -> String {
    format!(
        "{}/{}",
        percent_encode_path(bucket),
        percent_encode_path(key)
    )
}

fn percent_encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_copy_ranges_are_bounded_and_contiguous() {
        let part_bytes = MULTIPART_COPY_PART_BYTES;
        let ranges = multipart_copy_ranges(part_bytes * 2 + 7).unwrap();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], (1, 0, part_bytes - 1));
        assert_eq!(ranges[1], (2, part_bytes, part_bytes * 2 - 1));
        assert_eq!(ranges[2], (3, part_bytes * 2, part_bytes * 2 + 6));
        let expanded = multipart_copy_ranges(part_bytes * 10_001).unwrap();
        assert_eq!(expanded.len(), 10_000);
        assert!(expanded.windows(2).all(|pair| pair[0].2 + 1 == pair[1].1));
    }

    #[test]
    fn multipart_part_size_grows_before_part_limit() {
        let size = 5 * 1024 * 1024 * 1024 * 1024_u64;
        let part_size = effective_multipart_part_size(16 * 1024 * 1024, size).unwrap();
        assert!(part_size >= size.div_ceil(MAX_MULTIPART_PARTS));
        assert!(size.div_ceil(part_size) <= MAX_MULTIPART_PARTS);
        assert!(part_size <= MAX_IN_MEMORY_PART_BYTES);
    }

    #[test]
    fn upload_content_type_infers_safe_common_extensions() {
        assert_eq!(
            upload_content_type(Path::new("photo.PNG"), None).as_deref(),
            Some("image/png")
        );
        assert_eq!(
            upload_content_type(Path::new("notes.md"), None).as_deref(),
            Some("text/markdown")
        );
        assert_eq!(upload_content_type(Path::new("archive.bin"), None), None);
    }

    #[test]
    fn explicit_upload_content_type_overrides_extension_inference() {
        let metadata = UploadMetadata {
            content_type: Some("application/x-custom".to_string()),
            content_disposition: None,
            cache_control: None,
            user_metadata: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            upload_content_type(Path::new("photo.png"), Some(&metadata)).as_deref(),
            Some("application/x-custom")
        );
    }

    #[test]
    fn delete_confirmation_is_required_at_the_command_boundary() {
        assert!(require_delete_confirmation("", Some("DELETE")).is_err());
        assert!(require_delete_confirmation("NO", Some("DELETE")).is_err());
        assert!(require_delete_confirmation("DELETE", Some("DELETE")).is_ok());
        // Small recursive deletes still need an acknowledgement even though
        // they do not require the stronger typed-prefix token.
        assert!(require_delete_confirmation("", None).is_err());
        assert!(require_delete_confirmation("confirm", None).is_ok());
    }

    #[test]
    fn recursive_delete_confirmation_uses_the_exact_prefix_for_large_jobs() {
        assert!(require_delete_confirmation("DELETE folder", Some("DELETE folder")).is_ok());
        assert!(require_delete_confirmation("DELETE folder/", Some("DELETE folder")).is_err());
    }

    #[test]
    fn download_directory_hint_accepts_trailing_separators() {
        assert!(local_endpoint_has_directory_hint(
            &TransferEndpoint::Local {
                path: "C:\\Downloads\\".to_string(),
            }
        ));
        assert!(local_endpoint_has_directory_hint(
            &TransferEndpoint::Local {
                path: "C:/Downloads/".to_string(),
            }
        ));
        assert!(!local_endpoint_has_directory_hint(
            &TransferEndpoint::Local {
                path: "C:\\Downloads\\file.txt".to_string(),
            }
        ));
    }
}
