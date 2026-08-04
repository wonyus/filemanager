use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use aws_sdk_s3::{
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier},
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use uuid::Uuid;

const MAX_IN_MEMORY_PART_BYTES: u64 = 64 * 1024 * 1024;

use crate::{
    application::profile_service::ProfileService,
    domain::{error::AppError, profile::ConnectionProfile},
    dto::{
        settings::SettingsSnapshot,
        transfer::{
            CollisionPolicy, StartTransferRequest, TransferEndpoint, TransferJob,
            TransferOperation, TransferStatus,
        },
    },
    infrastructure::{
        credentials::{resolve_profile_credentials, CredentialStore},
        s3::S3ClientManager,
    },
    transfer::{
        recursive::{
            execute_recursive, plan_download_prefix, plan_remote_prefix, plan_upload_directory,
            write_mapping_manifest, CancellationFlag, RecursiveExecutor, RecursiveItem,
            RecursivePlan, RemoteObject,
        },
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
}

impl ExecutionOutcome {
    fn completed(transferred_bytes: u64, completed_items: u64) -> Self {
        Self {
            transferred_bytes,
            completed_items,
            failed_items: 0,
            cleanup_required_items: 0,
            status: TransferStatus::Completed,
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
        request.settings_snapshot = Some(self.settings.get().await);
        let job = self.manager.create(request.clone()).await?;
        let worker = self.clone();
        tokio::spawn(async move {
            worker.run(job.id, request).await;
        });
        Ok(job)
    }

    pub async fn retry(&self, id: Uuid) -> Result<TransferJob, AppError> {
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
            let settings = request.settings_snapshot.clone().unwrap_or_default();
            let outcome = self.execute(id, &request, &settings).await?;
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

    async fn execute(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
        settings: &SettingsSnapshot,
    ) -> Result<ExecutionOutcome, AppError> {
        match request.operation {
            TransferOperation::UploadFile => self.upload_file(id, request, settings).await,
            TransferOperation::DownloadFile => self.download_file(id, request, settings).await,
            TransferOperation::CopyObject => self.copy_object(id, request).await,
            TransferOperation::MoveObject => self.move_object(id, request).await,
            TransferOperation::DeleteObjects => self.delete_object(id, request).await,
            TransferOperation::UploadDirectory => {
                self.upload_directory(id, request, settings).await
            }
            TransferOperation::DownloadPrefix => self.download_prefix(id, request, settings).await,
            TransferOperation::CopyPrefix => self.copy_prefix(id, request, settings).await,
            TransferOperation::MovePrefix => self.move_prefix(id, request, settings).await,
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
        let (profile, bucket, key) = self.remote_target(request.destination.as_ref()).await?;
        let metadata = fs::metadata(&source).await?;
        if !metadata.is_file() {
            return Err(AppError::Validation(
                "upload source must be a file".to_string(),
            ));
        }
        let size = metadata.len();
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        if let Err(error) = ensure_collision(&client, &bucket, &key, request.collision_policy).await
        {
            if request.collision_policy == CollisionPolicy::Skip
                && matches!(error, AppError::TransferStateConflict(_))
            {
                return Ok(ExecutionOutcome::completed(0, 1));
            }
            return Err(error);
        }
        if size >= settings.multipart_threshold_bytes {
            self.upload_multipart(id, &client, &bucket, &key, &source, size, settings)
                .await?;
        } else {
            let body = ByteStream::from_path(&source)
                .await
                .map_err(|error| AppError::Provider(format!("local upload stream: {error}")))?;
            client
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .content_length(size as i64)
                .body(body)
                .send()
                .await
                .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
            self.manager
                .update_progress(id, size, 1, None, None)
                .await?;
        }
        Ok(ExecutionOutcome::completed(size, 1))
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_multipart(
        &self,
        id: Uuid,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
        source: &Path,
        size: u64,
        settings: &SettingsSnapshot,
    ) -> Result<(), AppError> {
        let part_size = settings
            .initial_part_size_bytes
            .clamp(5 * 1024 * 1024, 5 * 1024 * 1024 * 1024)
            .min(MAX_IN_MEMORY_PART_BYTES);
        let part_count = size.div_ceil(part_size);
        if part_count > 10_000 {
            return Err(AppError::Validation(
                "object requires too many multipart parts".to_string(),
            ));
        }
        let upload = client
            .create_multipart_upload()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();
        let mut file = File::open(source).await?;
        let mut completed = Vec::with_capacity(part_count as usize);
        let mut transferred = 0_u64;
        let mut part_number = 1_i32;
        let result = async {
            loop {
                if self.manager.is_cancel_requested(id).await {
                    return Err(AppError::TransferStateConflict(
                        "transfer cancelled".to_string(),
                    ));
                }
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
                let output = client
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
                completed.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(etag)
                        .build(),
                );
                transferred += read as u64;
                self.manager
                    .update_progress(id, transferred, 0, None, None)
                    .await?;
                part_number += 1;
            }
            let multipart = CompletedMultipartUpload::builder()
                .set_parts(Some(completed))
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
        let destination = local_path(request.destination.as_ref().ok_or_else(|| {
            AppError::Validation("download destination is required".to_string())
        })?)?;
        if destination.exists() && request.collision_policy != CollisionPolicy::Replace {
            if request.collision_policy == CollisionPolicy::Skip {
                return Ok(ExecutionOutcome::completed(0, 1));
            }
            return Err(AppError::Validation(
                "destination already exists".to_string(),
            ));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let output = client
            .get_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let total = output.content_length().unwrap_or(0).max(0) as u64;
        let partial = partial_path(&destination, id);
        let result: Result<u64, AppError> = async {
            let mut file = File::create(&partial).await?;
            let mut stream = output.body.into_async_read();
            let mut buffer = vec![0_u8; 1024 * 1024];
            let mut transferred = 0_u64;
            loop {
                if self.manager.is_cancel_requested(id).await {
                    return Err(AppError::TransferStateConflict(
                        "transfer cancelled".to_string(),
                    ));
                }
                let count = stream.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                file.write_all(&buffer[..count]).await?;
                transferred += count as u64;
                self.manager
                    .update_progress(id, transferred, 0, None, None)
                    .await?;
            }
            file.flush().await?;
            if total > 0 && transferred != total {
                return Err(AppError::Provider(format!(
                    "download size mismatch: expected {total} bytes, received {transferred}"
                )));
            }
            Ok(transferred)
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
                Ok(ExecutionOutcome::completed(
                    if total == 0 { transferred } else { total },
                    1,
                ))
            }
            Err(error) => {
                if !settings.keep_partial_downloads {
                    let _ = fs::remove_file(&partial).await;
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
        let destination = match request.destination.as_ref() {
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
                return Ok(ExecutionOutcome::completed(0, 1));
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
        client
            .copy_object()
            .copy_source(encode_copy_source(&source_bucket, &source_key))
            .bucket(&destination.0)
            .key(&destination.1)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
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
            || source_etag.is_some()
                && verified_head.e_tag().map(ToString::to_string) != source_etag
        {
            return Err(AppError::Provider("copy verification failed".to_string()));
        }
        Ok(ExecutionOutcome::completed(source_size, 1))
    }

    async fn move_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
    ) -> Result<ExecutionOutcome, AppError> {
        let result = self.copy_object(id, request).await?;
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
        if let Err(_error) = client
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
            return Ok(warning);
        }
        Ok(result)
    }

    async fn delete_object(
        &self,
        id: Uuid,
        request: &StartTransferRequest,
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
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;

        if recursive {
            return self
                .delete_prefix(
                    id,
                    &client,
                    &bucket,
                    &key,
                    request.confirmation.as_deref().unwrap_or_default(),
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
    async fn delete_prefix(
        &self,
        id: Uuid,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        prefix: &str,
        confirmation: &str,
    ) -> Result<ExecutionOutcome, AppError> {
        let objects = self.list_remote_objects(id, client, bucket, prefix).await?;
        if objects.is_empty() {
            return Ok(ExecutionOutcome::completed(0, 0));
        }
        let known_bytes = objects
            .iter()
            .filter_map(|object| object.size_bytes)
            .sum::<u64>();
        if objects.len() > 100 || known_bytes > 10 * 1024 * 1024 * 1024 {
            let expected = format!("DELETE {}", prefix.trim_end_matches('/'));
            if confirmation != expected {
                return Err(AppError::Validation(format!(
                    "typed confirmation required; enter `{expected}` to delete this prefix"
                )));
            }
        }

        let mut completed_items = 0_u64;
        let mut failed_items = 0_u64;
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
                    self.manager
                        .update_progress(id, 0, completed_items, None, None)
                        .await?;
                    continue;
                }
            };

            let errors = output.errors();
            let batch_failed = errors.len().min(chunk.len()) as u64;
            failed_items = failed_items.saturating_add(batch_failed);
            completed_items =
                completed_items.saturating_add((chunk.len() as u64).saturating_sub(batch_failed));
            for error in errors {
                if let Some(code) = error.code() {
                    if failure_codes.len() < 3 && !failure_codes.iter().any(|item| item == code) {
                        failure_codes.push(code.to_string());
                    }
                }
            }
            self.manager
                .update_progress(id, 0, completed_items, None, None)
                .await?;
        }

        if failed_items == 0 {
            return Ok(ExecutionOutcome::completed(0, completed_items));
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
            transferred_bytes: 0,
            completed_items,
            failed_items,
            cleanup_required_items: 0,
            status: TransferStatus::CompletedWithWarnings,
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
        let plan = plan_upload_directory(
            &source_root,
            &profile.id.to_string(),
            &bucket,
            &destination_prefix,
            request.collision_policy,
            &existing,
            settings.preserve_empty_folders,
        )?;
        let executor = S3RecursiveExecutor {
            client,
            manager: self.manager.clone(),
            transfer_id: id,
            operation: TransferOperation::UploadDirectory,
            settings: settings.clone(),
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
        let _ = write_mapping_manifest(
            &destination_root,
            &profile.id.to_string(),
            &bucket,
            &source_prefix,
            &plan,
        )?;
        let executor = S3RecursiveExecutor {
            client,
            manager: self.manager.clone(),
            transfer_id: id,
            operation: TransferOperation::DownloadPrefix,
            settings: settings.clone(),
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
            operation,
            settings: settings.clone(),
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
        Ok(ExecutionOutcome {
            transferred_bytes: result.transferred_bytes,
            completed_items: result.completed_items,
            failed_items: result.failed_items,
            cleanup_required_items: result.cleanup_required_items,
            status: result.status,
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

struct S3RecursiveExecutor {
    client: Arc<aws_sdk_s3::Client>,
    manager: Arc<TransferManager>,
    transfer_id: Uuid,
    operation: TransferOperation,
    settings: SettingsSnapshot,
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
        let TransferEndpoint::Remote { bucket, key, .. } = &item.destination else {
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
        if size >= self.settings.multipart_threshold_bytes {
            self.upload_multipart_item(path, bucket, key, size).await?;
            return Ok(size);
        }
        let body = ByteStream::from_path(path)
            .await
            .map_err(|error| AppError::Provider(format!("local upload stream: {error}")))?;
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_length(size as i64)
            .body(body)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        Ok(size)
    }

    async fn upload_multipart_item(
        &self,
        path: &str,
        bucket: &str,
        key: &str,
        size: u64,
    ) -> Result<(), AppError> {
        let part_size = self
            .settings
            .initial_part_size_bytes
            .clamp(5 * 1024 * 1024, 5 * 1024 * 1024 * 1024)
            .min(MAX_IN_MEMORY_PART_BYTES);
        if size.div_ceil(part_size) > 10_000 {
            return Err(AppError::Validation(
                "object requires too many multipart parts".to_string(),
            ));
        }
        let upload = self
            .client
            .create_multipart_upload()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| AppError::Provider("provider did not return an upload ID".to_string()))?
            .to_string();
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
        if item.is_directory {
            fs::create_dir_all(&destination).await?;
            return Ok(0);
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let output = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
        let expected = output.content_length().unwrap_or(0).max(0) as u64;
        if item.size_bytes.is_some_and(|planned| planned != expected) {
            return Err(AppError::Provider(
                "download integrity check failed: object size changed".to_string(),
            ));
        }
        let partial = partial_path(&destination, Uuid::new_v4());
        let result = async {
            let mut file = File::create(&partial).await?;
            let mut stream = output.body.into_async_read();
            let mut buffer = vec![0_u8; 1024 * 1024];
            let mut transferred = 0_u64;
            loop {
                self.manager.checkpoint(self.transfer_id).await?;
                let count = stream.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                file.write_all(&buffer[..count]).await?;
                transferred = transferred.saturating_add(count as u64);
            }
            file.flush().await?;
            drop(file);
            if expected != transferred {
                return Err(AppError::Provider(
                    "download integrity check failed: content length mismatch".to_string(),
                ));
            }
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
        self.client
            .copy_object()
            .copy_source(encode_copy_source(source_bucket, source_key))
            .bucket(destination_bucket)
            .key(destination_key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_error(&error)))?;
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
            || source_etag.is_some() && destination.e_tag().map(str::to_string) != source_etag
        {
            return Err(AppError::Provider("copy verification failed".to_string()));
        }
        Ok(source_size)
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
                    || path.split(['/', '\\']).any(|part| part == "..")
                {
                    return Err(AppError::Validation("local path is unsafe".to_string()));
                }
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
    if path.is_empty() || path.contains('\0') || path.split(['/', '\\']).any(|part| part == "..") {
        return Err(AppError::Validation("local path is invalid".to_string()));
    }
    Ok(PathBuf::from(path))
}

fn partial_path(destination: &Path, id: Uuid) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    destination.with_file_name(format!("{name}.s3fm-partial-{id}"))
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
        CollisionPolicy::Ask | CollisionPolicy::Fail => Err(AppError::Validation(
            "destination already exists".to_string(),
        )),
        CollisionPolicy::Replace => Ok(()),
    }
}

fn safe_provider_error(error: &impl std::fmt::Display) -> String {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
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
