use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use aws_sdk_s3::presigning::PresigningConfig;
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::SecretString;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::{
    domain::{
        error::AppError,
        profile::{ConnectionProfile, SecretReference},
    },
    dto::{
        explorer::{
            EntryKind, EntrySummary, ExplorerLocation, ListEntriesPage, ListEntriesRequest,
        },
        metadata::{
            ObjectMetadata, ObjectRequest, PreviewKind, PreviewRequest, PreviewResult, ShareLink,
            ShareLinkRequest, DEFAULT_PREVIEW_LIMIT_BYTES, MAX_SHARE_SECONDS,
            METADATA_SCHEMA_VERSION,
        },
        profile::{
            BucketSummary, ConnectionTestResult, ProfileDetail, ProfileDraft, ProfileSummary,
            SecretInput,
        },
    },
    infrastructure::{
        credentials::{resolve_profile_credentials, CredentialStore, ResolvedCredentials},
        database::Database,
        s3::S3ClientManager,
    },
    transfer::TransferManager,
};

pub struct ProfileService {
    database: Database,
    credentials: Arc<dyn CredentialStore>,
    clients: Arc<S3ClientManager>,
    lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
    transfers: Arc<TransferManager>,
}

impl ProfileService {
    pub fn new(
        database: Database,
        credentials: Arc<dyn CredentialStore>,
        clients: Arc<S3ClientManager>,
        transfers: Arc<TransferManager>,
    ) -> Self {
        Self {
            database,
            credentials,
            clients,
            lifecycle_lock: Arc::new(tokio::sync::Mutex::new(())),
            transfers,
        }
    }

    pub async fn list_profiles(&self) -> Result<Vec<ProfileSummary>, AppError> {
        self.database.list_profiles().await
    }

    pub async fn get_profile(&self, id: &str) -> Result<ProfileDetail, AppError> {
        let profile = self.load_profile(id).await?;
        Ok(to_detail(&profile))
    }

    pub async fn get_connection_profile(&self, id: &str) -> Result<ConnectionProfile, AppError> {
        self.load_profile(id).await
    }

    pub async fn create_profile(&self, draft: ProfileDraft) -> Result<ProfileDetail, AppError> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let now = Utc::now();
        let id = Uuid::new_v4();
        let secret_reference = SecretReference::new(id, "secret-access-key");
        let session_reference = SecretReference::new(id, "session-token");
        let has_secret = matches!(&draft.secret_access_key, SecretInput::Replace(_));
        let has_session = matches!(&draft.session_token, SecretInput::Replace(_));
        let profile = profile_from_draft(
            &draft,
            id,
            now,
            now,
            has_secret.then(|| secret_reference.clone()),
            has_session.then(|| session_reference.clone()),
        )?;
        let secret = required_secret(&draft)?;
        if let Err(error) = self.credentials.put(&secret_reference, secret).await {
            self.cleanup_written_credential(
                &secret_reference,
                "create-secret-write-failed",
                &error.to_string(),
            )
            .await;
            return Err(error);
        }
        let session_written = if let SecretInput::Replace(token) = &draft.session_token {
            if let Err(error) = self
                .credentials
                .put(&session_reference, SecretString::new(token.clone().into()))
                .await
            {
                self.cleanup_written_credential(
                    &secret_reference,
                    "create-session-write-failed",
                    &error.to_string(),
                )
                .await;
                // A vault adapter may report an error after writing.  Remove
                // both targets so a retry cannot retain an orphaned session.
                self.cleanup_written_credential(
                    &session_reference,
                    "create-session-write-failed",
                    &error.to_string(),
                )
                .await;
                return Err(error);
            }
            true
        } else {
            false
        };
        if let Err(error) = self
            .database
            .increment_credential_ref(&secret_reference)
            .await
        {
            self.cleanup_written_credential(
                &secret_reference,
                "create-secret-reference-failed",
                &error.to_string(),
            )
            .await;
            if session_written {
                self.cleanup_written_credential(
                    &session_reference,
                    "create-session-reference-failed",
                    &error.to_string(),
                )
                .await;
            }
            return Err(error);
        }
        if session_written {
            if let Err(error) = self
                .database
                .increment_credential_ref(&session_reference)
                .await
            {
                let _ = self
                    .database
                    .decrement_credential_ref(&secret_reference)
                    .await;
                self.cleanup_written_credential(
                    &secret_reference,
                    "create-session-reference-failed",
                    &error.to_string(),
                )
                .await;
                self.cleanup_written_credential(
                    &session_reference,
                    "create-session-reference-failed",
                    &error.to_string(),
                )
                .await;
                return Err(error);
            }
        }
        if let Err(error) = self.database.insert_profile(&profile).await {
            let _ = self
                .database
                .decrement_credential_ref(&secret_reference)
                .await;
            self.cleanup_written_credential(
                &secret_reference,
                "create-profile-insert-failed",
                &error.to_string(),
            )
            .await;
            if session_written {
                let _ = self
                    .database
                    .decrement_credential_ref(&session_reference)
                    .await;
                self.cleanup_written_credential(
                    &session_reference,
                    "create-profile-insert-failed",
                    &error.to_string(),
                )
                .await;
            }
            return Err(error);
        }
        Ok(to_detail(&profile))
    }

    pub async fn update_profile(
        &self,
        id: &str,
        expected_revision: i64,
        draft: ProfileDraft,
    ) -> Result<ProfileDetail, AppError> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let current = self.load_profile(id).await?;
        if self.transfers.has_active_for_profile(id).await {
            return Err(AppError::TransferStateConflict(
                "profile cannot be changed while transfers are active".to_string(),
            ));
        }
        if current.revision != expected_revision {
            return Err(AppError::ProfileRevisionConflict);
        }
        let secret_changed = !draft.secret_access_key.is_unchanged();
        let session_changed = !draft.session_token.is_unchanged();
        let clearing_session_for_static = current.credential_mode
            == crate::domain::provider::CredentialMode::TemporarySession
            && draft.credential_mode == crate::domain::provider::CredentialMode::Static
            && draft.session_token.is_unchanged();
        let next_secret_reference = match &draft.secret_access_key {
            SecretInput::Unchanged => current.secret_reference.clone(),
            SecretInput::Replace(_) => Some(SecretReference::new(
                current.id,
                &format!("secret-access-key-{}", Uuid::new_v4()),
            )),
            SecretInput::Clear => None,
        };
        let next_session_reference = match &draft.session_token {
            SecretInput::Unchanged
                if draft.credential_mode == crate::domain::provider::CredentialMode::Static =>
            {
                None
            }
            SecretInput::Unchanged => current.session_reference.clone(),
            SecretInput::Replace(_) => Some(SecretReference::new(
                current.id,
                &format!("session-token-{}", Uuid::new_v4()),
            )),
            SecretInput::Clear => None,
        };
        let now = Utc::now();
        let mut profile_draft = draft.clone();
        if profile_draft
            .access_key_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            profile_draft.access_key_id = current.access_key_id.clone();
        }
        let mut next = profile_from_draft(
            &profile_draft,
            current.id,
            current.created_at,
            now,
            next_secret_reference.clone(),
            next_session_reference.clone(),
        )?;
        if draft.secret_access_key.is_unchanged() {
            next.secret_reference = current.secret_reference.clone();
        }
        if draft.session_token.is_unchanged() && !clearing_session_for_static {
            next.session_reference = current.session_reference.clone();
        }
        next.revision = expected_revision;

        if let SecretInput::Replace(_) = &draft.secret_access_key {
            let reference = next
                .secret_reference
                .as_ref()
                .expect("new secret reference");
            if let Err(error) = self
                .credentials
                .put(reference, required_secret(&draft)?)
                .await
            {
                self.cleanup_written_credential(
                    reference,
                    "update-secret-write-failed",
                    &error.to_string(),
                )
                .await;
                return Err(error);
            }
            if let Err(error) = self.database.increment_credential_ref(reference).await {
                self.cleanup_written_credential(
                    reference,
                    "update-secret-reference-failed",
                    &error.to_string(),
                )
                .await;
                return Err(error);
            }
        }
        if let SecretInput::Replace(token) = &draft.session_token {
            let reference = next
                .session_reference
                .as_ref()
                .expect("new session reference");
            if let Err(error) = self
                .credentials
                .put(reference, SecretString::new(token.clone().into()))
                .await
            {
                self.cleanup_written_credential(
                    reference,
                    "update-session-write-failed",
                    &error.to_string(),
                )
                .await;
                if matches!(&draft.secret_access_key, SecretInput::Replace(_)) {
                    let secret_reference = next
                        .secret_reference
                        .as_ref()
                        .expect("new secret reference");
                    let _ = self
                        .database
                        .decrement_credential_ref(secret_reference)
                        .await;
                    self.cleanup_written_credential(
                        secret_reference,
                        "update-session-write-failed",
                        &error.to_string(),
                    )
                    .await;
                }
                return Err(error);
            }
            if let Err(error) = self.database.increment_credential_ref(reference).await {
                self.cleanup_written_credential(
                    reference,
                    "update-session-reference-failed",
                    &error.to_string(),
                )
                .await;
                if matches!(&draft.secret_access_key, SecretInput::Replace(_)) {
                    let secret_reference = next
                        .secret_reference
                        .as_ref()
                        .expect("new secret reference");
                    let _ = self
                        .database
                        .decrement_credential_ref(secret_reference)
                        .await;
                    self.cleanup_written_credential(
                        secret_reference,
                        "update-session-reference-failed",
                        &error.to_string(),
                    )
                    .await;
                }
                return Err(error);
            }
        }
        if let Err(error) = self.database.update_profile(&next, expected_revision).await {
            if matches!(&draft.secret_access_key, SecretInput::Replace(_)) {
                let reference = next
                    .secret_reference
                    .as_ref()
                    .expect("new secret reference");
                let _ = self.database.decrement_credential_ref(reference).await;
                self.cleanup_written_credential(
                    reference,
                    "update-profile-write-failed",
                    &error.to_string(),
                )
                .await;
            }
            if matches!(&draft.session_token, SecretInput::Replace(_)) {
                let reference = next
                    .session_reference
                    .as_ref()
                    .expect("new session reference");
                let _ = self.database.decrement_credential_ref(reference).await;
                self.cleanup_written_credential(
                    reference,
                    "update-profile-write-failed",
                    &error.to_string(),
                )
                .await;
            }
            return Err(error);
        }
        if secret_changed {
            self.release_reference_best_effort(current.secret_reference.as_ref())
                .await;
        }
        if session_changed || clearing_session_for_static {
            self.release_reference_best_effort(current.session_reference.as_ref())
                .await;
        }
        self.clients.invalidate(current.id).await;
        next.revision = expected_revision + 1;
        Ok(to_detail(&next))
    }

    pub async fn duplicate_profile(&self, id: &str) -> Result<ProfileDetail, AppError> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let current = self.load_profile(id).await?;
        let now = Utc::now();
        let mut duplicate = current.clone();
        duplicate.id = Uuid::new_v4();
        let suffix = " Copy";
        let max_base_chars = 80usize.saturating_sub(suffix.chars().count());
        let base = current
            .name
            .chars()
            .take(max_base_chars)
            .collect::<String>();
        duplicate.name = format!("{base}{suffix}");
        duplicate.created_at = now;
        duplicate.updated_at = now;
        duplicate.revision = 1;
        duplicate.favorite = false;
        duplicate.favorite_order = 0;
        let mut secret_ref_incremented = false;
        if let Some(reference) = duplicate.secret_reference.as_ref() {
            self.database.increment_credential_ref(reference).await?;
            secret_ref_incremented = true;
        }
        let mut session_ref_incremented = false;
        if let Some(reference) = duplicate.session_reference.as_ref() {
            if let Err(error) = self.database.increment_credential_ref(reference).await {
                if secret_ref_incremented {
                    if let Some(secret_reference) = duplicate.secret_reference.as_ref() {
                        if let Err(cleanup_error) = self
                            .database
                            .decrement_credential_ref(secret_reference)
                            .await
                        {
                            let _ = self
                                .database
                                .record_credential_cleanup(
                                    secret_reference,
                                    "duplicate-profile-reference-rollback",
                                    &cleanup_error.to_string(),
                                )
                                .await;
                        }
                    }
                }
                return Err(error);
            }
            session_ref_incremented = true;
        }
        if let Err(error) = self.database.insert_profile(&duplicate).await {
            if secret_ref_incremented {
                if let Some(reference) = duplicate.secret_reference.as_ref() {
                    if let Err(cleanup_error) =
                        self.database.decrement_credential_ref(reference).await
                    {
                        let _ = self
                            .database
                            .record_credential_cleanup(
                                reference,
                                "duplicate-profile-insert-rollback",
                                &cleanup_error.to_string(),
                            )
                            .await;
                    }
                }
            }
            if session_ref_incremented {
                if let Some(reference) = duplicate.session_reference.as_ref() {
                    if let Err(cleanup_error) =
                        self.database.decrement_credential_ref(reference).await
                    {
                        let _ = self
                            .database
                            .record_credential_cleanup(
                                reference,
                                "duplicate-profile-insert-rollback",
                                &cleanup_error.to_string(),
                            )
                            .await;
                    }
                }
            }
            return Err(error);
        }
        Ok(to_detail(&duplicate))
    }

    pub async fn delete_profile(&self, id: &str) -> Result<(), AppError> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        if self.transfers.has_active_for_profile(id).await {
            return Err(AppError::TransferStateConflict(
                "profile cannot be deleted while transfers are active".to_string(),
            ));
        }
        let Some((candidate_profile, candidate_refs)) = self
            .database
            .profile_and_credential_cleanup_candidates(id)
            .await?
        else {
            return Err(AppError::ProfileNotFound(id.to_string()));
        };

        // Delete vault entries first.  If any provider rejects the deletion,
        // restore entries already removed and leave SQLite metadata intact so
        // the user can retry or force the operation explicitly.
        let mut deleted_credentials = Vec::new();
        for reference in &candidate_refs {
            let previous = match self.credentials.get(reference).await {
                Ok(value) => value,
                Err(error) => {
                    self.restore_deleted_credentials(&mut deleted_credentials)
                        .await;
                    return Err(error);
                }
            };
            if let Err(error) = self.credentials.delete(reference).await {
                self.restore_deleted_credentials(&mut deleted_credentials)
                    .await;
                return Err(error);
            }
            deleted_credentials.push((reference.clone(), previous));
        }

        let (profile, _cleanup_refs) = match self
            .database
            .delete_profile_and_release_credentials(id)
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => {
                self.restore_deleted_credentials(&mut deleted_credentials)
                    .await;
                return Err(AppError::ProfileNotFound(id.to_string()));
            }
            Err(error) => {
                self.restore_deleted_credentials(&mut deleted_credentials)
                    .await;
                return Err(error);
            }
        };
        if profile.id != candidate_profile.id {
            self.restore_deleted_credentials(&mut deleted_credentials)
                .await;
            return Err(AppError::Unknown(
                "profile changed during deletion; retry the operation".to_string(),
            ));
        }
        self.clients.invalidate(profile.id).await;
        Ok(())
    }

    async fn restore_deleted_credentials(
        &self,
        deleted: &mut Vec<(SecretReference, Option<SecretString>)>,
    ) {
        while let Some((reference, secret)) = deleted.pop() {
            if let Some(secret) = secret {
                if let Err(error) = self.credentials.put(&reference, secret).await {
                    let _ = self
                        .database
                        .record_credential_cleanup(
                            &reference,
                            "delete-profile-rollback",
                            &error.to_string(),
                        )
                        .await;
                }
            }
        }
    }

    pub async fn test_profile(
        &self,
        draft: ProfileDraft,
    ) -> Result<ConnectionTestResult, AppError> {
        let now = Utc::now();
        let current = if let Some(id) = draft.id.as_deref() {
            Some(self.load_profile(id).await?)
        } else {
            None
        };
        let test_id = current
            .as_ref()
            .map(|profile| profile.id)
            .unwrap_or_else(Uuid::new_v4);
        let test_secret_reference = match &draft.secret_access_key {
            SecretInput::Replace(_) => {
                Some(SecretReference::new(test_id, "test-secret-access-key"))
            }
            SecretInput::Unchanged => current
                .as_ref()
                .and_then(|profile| profile.secret_reference.clone()),
            SecretInput::Clear => None,
        };
        let test_session_reference = match &draft.session_token {
            SecretInput::Replace(_) => Some(SecretReference::new(test_id, "test-session-token")),
            SecretInput::Unchanged => current
                .as_ref()
                .and_then(|profile| profile.session_reference.clone()),
            SecretInput::Clear => None,
        };
        let mut profile_draft = draft.clone();
        if profile_draft
            .access_key_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            profile_draft.access_key_id = current
                .as_ref()
                .and_then(|profile| profile.access_key_id.clone());
        }
        let profile = profile_from_draft(
            &profile_draft,
            test_id,
            now,
            now,
            test_secret_reference,
            test_session_reference,
        )?;
        let existing_secret = if draft.secret_access_key.is_unchanged() {
            let reference = current
                .as_ref()
                .and_then(|profile| profile.secret_reference.as_ref())
                .ok_or_else(|| {
                    AppError::CredentialMissing("secret access key is required".to_string())
                })?;
            Some(self.credentials.get(reference).await?.ok_or_else(|| {
                AppError::CredentialMissing("secret access key is unavailable".to_string())
            })?)
        } else {
            None
        };
        let existing_session = if draft.session_token.is_unchanged() {
            if let Some(reference) = current
                .as_ref()
                .and_then(|profile| profile.session_reference.as_ref())
            {
                self.credentials.get(reference).await?
            } else {
                None
            }
        } else {
            None
        };
        if profile.credential_mode == crate::domain::provider::CredentialMode::TemporarySession
            && draft.session_token.is_unchanged()
            && existing_session.is_none()
        {
            return Err(AppError::CredentialMissing(
                "temporary session token is unavailable".to_string(),
            ));
        }
        let access_key_id = draft
            .access_key_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                current
                    .as_ref()
                    .and_then(|profile| profile.access_key_id.clone())
            })
            .ok_or_else(|| AppError::CredentialMissing("access key ID is required".to_string()))?;
        let secret_access_key = match &draft.secret_access_key {
            SecretInput::Replace(_) => required_secret(&draft)?,
            SecretInput::Unchanged => existing_secret.ok_or_else(|| {
                AppError::CredentialMissing("secret access key is required".to_string())
            })?,
            SecretInput::Clear => {
                return Err(AppError::CredentialMissing(
                    "secret access key cannot be cleared for a connection test".to_string(),
                ))
            }
        };
        let session_token = match &draft.session_token {
            SecretInput::Replace(value) => Some(SecretString::new(value.clone().into())),
            SecretInput::Unchanged => existing_session,
            SecretInput::Clear => None,
        };
        let credentials = ResolvedCredentials {
            access_key_id,
            secret_access_key,
            session_token,
        };
        let started = Instant::now();
        let client = self.clients.build_temporary(&profile, &credentials).await?;
        let mut result = ConnectionTestResult {
            schema_version: 1,
            success: false,
            latency_ms: 0,
            bucket_access: false,
            message: "Connection test failed".to_string(),
            provider_request_id: None,
            can_list_buckets: None,
            can_head_bucket: None,
            // Keep capability claims conservative until a provider-specific probe
            // proves support.  An authenticated list/head request alone does not
            // guarantee multipart or presigning support on S3-compatible services.
            supports_multipart_upload: None,
            supports_presigned_get: None,
        };
        match client.list_buckets().send().await {
            Ok(_) => {
                result.can_list_buckets = Some(true);
                if let Some(bucket) = profile.default_bucket.as_deref() {
                    match client.head_bucket().bucket(bucket).send().await {
                        Ok(_) => {
                            result.success = true;
                            result.bucket_access = true;
                            result.can_head_bucket = Some(true);
                            result.message =
                                "Connection successful; bucket access verified".to_string();
                        }
                        Err(error) => {
                            result.can_head_bucket = Some(false);
                            result.message =
                                format!("Bucket access failed: {}", safe_provider_message(&error));
                        }
                    }
                } else {
                    result.success = true;
                    result.message = "Connection successful".to_string();
                }
            }
            Err(error) => {
                result.can_list_buckets = Some(false);
                if let Some(bucket) = profile.default_bucket.as_deref() {
                    match client.head_bucket().bucket(bucket).send().await {
                        Ok(_) => {
                            result.success = true;
                            result.bucket_access = true;
                            result.can_head_bucket = Some(true);
                            result.message =
                                "Connection successful; bucket access verified".to_string();
                        }
                        Err(head_error) => {
                            result.can_head_bucket = Some(false);
                            result.message = format!(
                                "Bucket access failed: {}",
                                safe_provider_message(&head_error)
                            );
                        }
                    }
                } else {
                    result.message =
                        format!("Bucket listing failed: {}", safe_provider_message(&error));
                }
            }
        }
        result.latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        if result.success && draft.id.is_some() {
            let _ = self
                .database
                .save_profile_capabilities(profile.id, &result)
                .await;
        }
        Ok(result)
    }

    pub async fn list_buckets(&self, id: &str) -> Result<Vec<BucketSummary>, AppError> {
        let profile = self.load_profile(id).await?;
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let output = match client.list_buckets().send().await {
            Ok(output) => output,
            Err(error) => {
                if let Some(bucket) = profile.default_bucket.as_deref() {
                    client
                        .head_bucket()
                        .bucket(bucket)
                        .send()
                        .await
                        .map_err(|head_error| {
                            AppError::Provider(safe_provider_message(&head_error))
                        })?;
                    return Ok(vec![BucketSummary {
                        schema_version: 1,
                        name: bucket.to_string(),
                        creation_date: None,
                    }]);
                }
                return Err(AppError::Provider(safe_provider_message(&error)));
            }
        };
        let mut buckets = output
            .buckets()
            .iter()
            .filter_map(|bucket| {
                bucket.name().map(|name| BucketSummary {
                    schema_version: 1,
                    name: name.to_string(),
                    creation_date: bucket.creation_date().map(ToString::to_string),
                })
            })
            .collect::<Vec<_>>();
        if let Some(default_bucket) = profile.default_bucket.as_deref() {
            if !buckets.iter().any(|bucket| bucket.name == default_bucket)
                && client
                    .head_bucket()
                    .bucket(default_bucket)
                    .send()
                    .await
                    .is_ok()
            {
                buckets.push(BucketSummary {
                    schema_version: 1,
                    name: default_bucket.to_string(),
                    creation_date: None,
                });
            }
        }
        buckets.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(buckets)
    }

    pub async fn list_entries(
        &self,
        request: ListEntriesRequest,
    ) -> Result<ListEntriesPage, AppError> {
        if request.schema_version != 1 {
            return Err(AppError::Validation(
                "unsupported listing schema version".to_string(),
            ));
        }
        validate_explorer_location(&request.location, request.continuation_token.as_deref())?;
        if request.page_size == 0 || request.page_size > 1_000 {
            return Err(AppError::Validation(
                "pageSize must be between 1 and 1000".to_string(),
            ));
        }
        let profile = self.load_profile(&request.location.profile_id).await?;
        let prefix = normalize_location_prefix(&profile, &request.location.prefix)?;
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        let mut operation = client
            .list_objects_v2()
            .bucket(&request.location.bucket)
            .prefix(&prefix)
            .delimiter("/")
            .max_keys(i32::from(request.page_size.clamp(1, 1_000)));
        if let Some(token) = &request.continuation_token {
            operation = operation.continuation_token(token);
        }
        let output = operation
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))?;
        let mut entries = Vec::new();
        let mut seen_prefixes = HashSet::new();
        for common_prefix in output.common_prefixes() {
            if let Some(key) = common_prefix.prefix() {
                seen_prefixes.insert(key.to_string());
                entries.push(entry_for_prefix(&request.location, key));
            }
        }
        for object in output.contents() {
            let Some(key) = object.key() else { continue };
            let is_marker = object.size() == Some(0) && key.ends_with('/');
            if is_marker && seen_prefixes.contains(key) {
                continue;
            }
            entries.push(EntrySummary {
                schema_version: 1,
                id: format!(
                    "{}:{}:{}:{}",
                    request.location.profile_id,
                    request.location.bucket,
                    key,
                    if is_marker { "marker" } else { "file" }
                ),
                kind: if is_marker {
                    EntryKind::FolderMarker
                } else {
                    EntryKind::File
                },
                display_name: display_name(&prefix, key),
                key: key.to_string(),
                size: object.size().map(|value| value as u64),
                last_modified: object.last_modified().map(ToString::to_string),
                storage_class: object
                    .storage_class()
                    .map(|value| value.as_str().to_string()),
                content_type_hint: None,
                is_folder_marker: is_marker,
            });
        }
        Ok(ListEntriesPage {
            schema_version: 1,
            request_generation: request.request_generation,
            location: request.location,
            entries,
            next_token: output.next_continuation_token().map(ToString::to_string),
            is_complete: !output.is_truncated().unwrap_or(false),
            provider_request_id: None,
        })
    }

    /// Return provider metadata for one object without exposing credentials or
    /// the raw SDK response over IPC.
    pub async fn head_object(&self, request: ObjectRequest) -> Result<ObjectMetadata, AppError> {
        let (profile, client, key) = self.object_context(&request).await?;
        let head = self.fetch_head(&client, &request.bucket, &key).await?;
        Ok(metadata_from_head(&profile, &request, &key, &head))
    }

    /// Read a bounded UTF-8 preview for an allow-listed text object.
    pub async fn preview_object(&self, request: PreviewRequest) -> Result<PreviewResult, AppError> {
        let object_request = ObjectRequest {
            schema_version: request.schema_version,
            profile_id: request.profile_id.clone(),
            bucket: request.bucket.clone(),
            key: request.key.clone(),
        };
        let (profile, client, key) = self.object_context(&object_request).await?;
        let head = self.fetch_head(&client, &request.bucket, &key).await?;
        let content_type = head.content_type().unwrap_or("application/octet-stream");
        let Some(preview_kind) = preview_kind_for_content_type(content_type) else {
            return Err(AppError::UnsupportedProviderFeature(
                "preview is unavailable for this content type; use Properties or Download"
                    .to_string(),
            ));
        };
        let total_size = head
            .content_length()
            .and_then(|size| u64::try_from(size).ok());

        // Binary media and PDF previews use a short-lived in-memory bearer
        // URL.  The UI clears this handle when the selected object changes;
        // it is never persisted or sent to diagnostics/history.
        if !matches!(preview_kind, PreviewKind::Text) {
            let expires_in_seconds = 900_u32;
            let (url, expires_at) = self
                .presigned_get(&client, &request.bucket, &key, expires_in_seconds)
                .await?;
            return Ok(PreviewResult {
                schema_version: METADATA_SCHEMA_VERSION,
                profile_id: profile.id.to_string(),
                bucket: request.bucket,
                key,
                preview_kind,
                content_type: content_type.to_string(),
                text: String::new(),
                url: Some(url),
                expires_at: Some(expires_at),
                bytes_read: 0,
                total_size,
                truncated: false,
            });
        }
        let max_bytes = request
            .max_bytes
            .unwrap_or(DEFAULT_PREVIEW_LIMIT_BYTES)
            .clamp(1, DEFAULT_PREVIEW_LIMIT_BYTES);
        let range = format!("bytes=0-{}", max_bytes.saturating_sub(1));
        let output = client
            .get_object()
            .bucket(&request.bucket)
            .key(&key)
            .range(range)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))?;

        // Read one byte beyond the cap so a provider that ignores the range
        // request cannot cause an unbounded preview response.
        let mut stream = output.body.into_async_read().take(u64::from(max_bytes) + 1);
        let mut bytes = Vec::with_capacity(max_bytes as usize + 1);
        stream.read_to_end(&mut bytes).await.map_err(AppError::Io)?;
        let provider_returned_more = bytes.len() > max_bytes as usize;
        if provider_returned_more {
            bytes.truncate(max_bytes as usize);
            // Do not return a partial UTF-8 sequence at the byte limit.
            while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
                bytes.pop();
            }
        }
        let text = String::from_utf8(bytes.clone()).map_err(|_| {
            AppError::UnsupportedProviderFeature("the object is not valid UTF-8 text".to_string())
        })?;
        let truncated =
            provider_returned_more || total_size.is_some_and(|size| size > u64::from(max_bytes));
        Ok(PreviewResult {
            schema_version: METADATA_SCHEMA_VERSION,
            profile_id: profile.id.to_string(),
            bucket: request.bucket,
            key,
            preview_kind,
            content_type: content_type.to_string(),
            text,
            url: None,
            expires_at: None,
            bytes_read: u32::try_from(bytes.len()).unwrap_or(max_bytes),
            total_size,
            truncated,
        })
    }

    /// Create a short-lived GET URL.  The object is headed first so callers
    /// receive a deterministic object-not-found/provider error instead of a
    /// link that is known to be invalid at creation time.
    pub async fn create_share_link(
        &self,
        request: ShareLinkRequest,
    ) -> Result<ShareLink, AppError> {
        let object_request = ObjectRequest {
            schema_version: request.schema_version,
            profile_id: request.profile_id.clone(),
            bucket: request.bucket.clone(),
            key: request.key.clone(),
        };
        let (profile, client, key) = self.object_context(&object_request).await?;
        self.fetch_head(&client, &request.bucket, &key).await?;
        let expires_in_seconds = normalize_share_expiry(request.expires_in_seconds);
        let (url, expires_at) = self
            .presigned_get(&client, &request.bucket, &key, expires_in_seconds)
            .await?;
        Ok(ShareLink {
            schema_version: METADATA_SCHEMA_VERSION,
            profile_id: profile.id.to_string(),
            bucket: request.bucket,
            key,
            url,
            expires_at,
            expires_in_seconds,
        })
    }

    async fn object_context(
        &self,
        request: &ObjectRequest,
    ) -> Result<(ConnectionProfile, Arc<aws_sdk_s3::Client>, String), AppError> {
        if request.schema_version != METADATA_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported metadata schema version".to_string(),
            ));
        }
        validate_object_bucket(&request.bucket)?;
        let profile = self.load_profile(&request.profile_id).await?;
        let key = normalize_object_key(&profile, &request.key)?;
        let credentials = resolve_profile_credentials(self.credentials.as_ref(), &profile).await?;
        let client = self.clients.get_or_create(&profile, &credentials).await?;
        Ok((profile, client, key))
    }

    async fn fetch_head(
        &self,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
    ) -> Result<aws_sdk_s3::operation::head_object::HeadObjectOutput, AppError> {
        client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))
    }

    async fn presigned_get(
        &self,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
        expires_in_seconds: u32,
    ) -> Result<(String, String), AppError> {
        let config =
            PresigningConfig::expires_in(Duration::from_secs(u64::from(expires_in_seconds)))
                .map_err(|error| {
                    AppError::Validation(format!("invalid preview expiry: {error}"))
                })?;
        let presigned = client
            .get_object()
            .bucket(bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))?;
        Ok((
            presigned.uri().to_string(),
            (Utc::now() + ChronoDuration::seconds(i64::from(expires_in_seconds))).to_rfc3339(),
        ))
    }

    async fn load_profile(&self, id: &str) -> Result<ConnectionProfile, AppError> {
        let profile = self
            .database
            .get_profile(id)
            .await?
            .ok_or_else(|| AppError::ProfileNotFound(id.to_string()))?;
        profile.validate()?;
        Ok(profile)
    }

    async fn release_reference(&self, reference: Option<&SecretReference>) -> Result<(), AppError> {
        let Some(reference) = reference else {
            return Ok(());
        };
        if self.database.decrement_credential_ref(reference).await? {
            if let Err(error) = self.credentials.delete(reference).await {
                self.database
                    .record_credential_cleanup(
                        reference,
                        "delete-after-last-reference",
                        &error.to_string(),
                    )
                    .await?;
                return Err(error);
            }
        }
        Ok(())
    }

    async fn release_reference_best_effort(&self, reference: Option<&SecretReference>) {
        if let Err(error) = self.release_reference(reference).await {
            // The profile/database mutation has already committed at this point.
            // Keep the operation successful and leave a retryable cleanup record.
            tracing::warn!(error = %error, "credential cleanup deferred");
        }
    }

    async fn cleanup_written_credential(
        &self,
        reference: &SecretReference,
        reason: &str,
        cause: &str,
    ) {
        if let Err(error) = self.credentials.delete(reference).await {
            let detail = format!("{cause}; credential deletion failed: {error}");
            let _ = self
                .database
                .record_credential_cleanup(reference, reason, &detail)
                .await;
        }
    }
}

fn validate_object_bucket(bucket: &str) -> Result<(), AppError> {
    if !(3..=255).contains(&bucket.chars().count())
        || bucket.len() > 255
        || bucket.contains('/')
        || bucket.contains('\\')
        || bucket.chars().any(char::is_control)
    {
        return Err(AppError::Validation("bucket is invalid".to_string()));
    }
    Ok(())
}

fn normalize_object_key(profile: &ConnectionProfile, requested: &str) -> Result<String, AppError> {
    if requested.is_empty()
        || requested.len() > 1_024
        || requested.starts_with('/')
        || requested.contains('\\')
        || requested.chars().any(char::is_control)
        || requested.split('/').any(|segment| segment == "..")
    {
        return Err(AppError::RootPrefixViolation);
    }
    if let Some(root) = &profile.root_prefix {
        if !requested.starts_with(root) {
            return Err(AppError::RootPrefixViolation);
        }
    }
    Ok(requested.to_string())
}

fn preview_kind_for_content_type(content_type: &str) -> Option<PreviewKind> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if media_type.starts_with("text/")
        || matches!(
            media_type.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/css"
                | "application/graphql"
                | "application/yaml"
                | "application/x-yaml"
                | "image/svg+xml"
        )
    {
        return Some(PreviewKind::Text);
    }
    if matches!(
        media_type.as_str(),
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/bmp"
    ) {
        return Some(PreviewKind::Image);
    }
    if matches!(
        media_type.as_str(),
        "audio/mpeg" | "audio/wav" | "audio/ogg"
    ) {
        return Some(PreviewKind::Audio);
    }
    if matches!(media_type.as_str(), "video/mp4" | "video/webm") {
        return Some(PreviewKind::Video);
    }
    if media_type == "application/pdf" {
        return Some(PreviewKind::Pdf);
    }
    None
}

fn normalize_share_expiry(value: Option<u32>) -> u32 {
    value.unwrap_or(3_600).clamp(300, MAX_SHARE_SECONDS)
}

fn metadata_from_head(
    profile: &ConnectionProfile,
    request: &ObjectRequest,
    key: &str,
    head: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
) -> ObjectMetadata {
    let content_type = head.content_type().map(ToString::to_string);
    let preview_kind = content_type
        .as_deref()
        .and_then(preview_kind_for_content_type);
    let (preview_supported, preview_reason) = if preview_kind.is_some() {
        (true, None)
    } else if content_type.is_some() {
        (
            false,
            Some(
                "preview is unavailable for this content type; use Properties or Download"
                    .to_string(),
            ),
        )
    } else {
        (
            false,
            Some("the object does not declare a supported content type".to_string()),
        )
    };
    ObjectMetadata {
        schema_version: METADATA_SCHEMA_VERSION,
        profile_id: profile.id.to_string(),
        bucket: request.bucket.clone(),
        key: key.to_string(),
        size: head
            .content_length()
            .and_then(|size| u64::try_from(size).ok()),
        etag: head.e_tag().map(ToString::to_string),
        version_id: head.version_id().map(ToString::to_string),
        last_modified: head.last_modified().map(ToString::to_string),
        storage_class: head.storage_class().map(|value| value.as_str().to_string()),
        content_type,
        content_disposition: head.content_disposition().map(ToString::to_string),
        cache_control: head.cache_control().map(ToString::to_string),
        content_encoding: head.content_encoding().map(ToString::to_string),
        content_language: head.content_language().map(ToString::to_string),
        expires: head.expires_string().map(ToString::to_string),
        checksum_sha256: head.checksum_sha256().map(ToString::to_string),
        checksum_sha1: head.checksum_sha1().map(ToString::to_string),
        checksum_crc32: head.checksum_crc32().map(ToString::to_string),
        checksum_crc32c: head.checksum_crc32_c().map(ToString::to_string),
        encryption: head
            .server_side_encryption()
            .map(|value| value.as_str().to_string()),
        user_metadata: head
            .metadata()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        preview_supported,
        preview_kind,
        preview_reason,
    }
}

fn profile_from_draft(
    draft: &ProfileDraft,
    id: Uuid,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    secret_reference: Option<SecretReference>,
    session_reference: Option<SecretReference>,
) -> Result<ConnectionProfile, AppError> {
    if draft.schema_version != 1 {
        return Err(AppError::Validation(
            "unsupported profile schema version".to_string(),
        ));
    }
    if draft.credential_mode == crate::domain::provider::CredentialMode::Static
        && matches!(&draft.session_token, SecretInput::Replace(_))
    {
        return Err(AppError::Validation(
            "session token is only valid for temporary session credentials".to_string(),
        ));
    }
    if let SecretInput::Replace(value) = &draft.session_token {
        if value.is_empty() || value.chars().count() > 16_384 {
            return Err(AppError::Validation(
                "Session token must be 1–16,384 characters".to_string(),
            ));
        }
    }
    if let SecretInput::Replace(value) = &draft.secret_access_key {
        if value.is_empty() || value.chars().count() > 16_384 {
            return Err(AppError::Validation(
                "Secret access key must be 1–16,384 characters".to_string(),
            ));
        }
    }
    let provider = draft.provider;
    let region = if draft.region.trim().is_empty() {
        provider.default_region().to_string()
    } else {
        draft.region.trim().to_string()
    };
    let account_id = draft
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if provider == crate::domain::provider::ProviderType::CloudflareR2 && region != "auto" {
        return Err(AppError::Validation(
            "Cloudflare R2 region must be `auto`".to_string(),
        ));
    }
    if provider == crate::domain::provider::ProviderType::CloudflareR2
        && account_id.is_some_and(|value| {
            !(3..=64).contains(&value.chars().count())
                || !value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
    {
        return Err(AppError::Validation(
            "Cloudflare R2 account ID is invalid".to_string(),
        ));
    }
    let explicit_endpoint = draft
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let inferred_r2_account = if provider == crate::domain::provider::ProviderType::CloudflareR2
        && account_id.is_none()
    {
        explicit_endpoint.as_deref().and_then(|endpoint| {
            url::Url::parse(endpoint)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string))
                .and_then(|host| {
                    host.strip_suffix(".r2.cloudflarestorage.com")
                        .map(ToString::to_string)
                })
        })
    } else {
        None
    };
    let provider_account_id = account_id.or(inferred_r2_account.as_deref());
    if provider == crate::domain::provider::ProviderType::CloudflareR2 {
        let Some(account_id) = provider_account_id else {
            return Err(AppError::Validation(
                "Cloudflare R2 requires an account ID or account endpoint".to_string(),
            ));
        };
        if !(3..=64).contains(&account_id.chars().count())
            || !account_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(AppError::Validation(
                "Cloudflare R2 account ID is invalid".to_string(),
            ));
        }
    }
    let derived_endpoint = provider.endpoint_for(&region, provider_account_id);
    let endpoint = explicit_endpoint
        .clone()
        .or_else(|| derived_endpoint.clone());
    if provider == crate::domain::provider::ProviderType::AwsS3 && endpoint.is_some() {
        return Err(AppError::Validation(
            "AWS S3 uses its managed endpoint; choose Custom S3 for a custom endpoint".to_string(),
        ));
    }
    if matches!(
        provider,
        crate::domain::provider::ProviderType::CloudflareR2
            | crate::domain::provider::ProviderType::Wasabi
    ) && explicit_endpoint.is_some()
        && explicit_endpoint != derived_endpoint
    {
        return Err(AppError::Validation(
            "The selected provider controls its endpoint; choose Custom S3 for a different gateway"
                .to_string(),
        ));
    }
    if provider == crate::domain::provider::ProviderType::Wasabi {
        let Some(expected_endpoint) = provider.endpoint_for(&region, None) else {
            return Err(AppError::Validation(
                "Wasabi region is not supported by the preset".to_string(),
            ));
        };
        if endpoint.as_deref() != Some(expected_endpoint.as_str()) {
            return Err(AppError::Validation(
                "Wasabi endpoint must match the selected region preset".to_string(),
            ));
        }
    }
    if endpoint.is_none()
        && matches!(
            provider,
            crate::domain::provider::ProviderType::CloudflareR2
                | crate::domain::provider::ProviderType::Minio
                | crate::domain::provider::ProviderType::CustomS3
        )
    {
        return Err(AppError::Validation(
            "An endpoint or provider account identifier is required".to_string(),
        ));
    }
    let access_key_id = draft
        .access_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let default_bucket = draft
        .default_bucket
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let profile = ConnectionProfile {
        id,
        name: draft.name.trim().to_string(),
        provider,
        endpoint,
        region,
        credential_mode: draft.credential_mode,
        access_key_id,
        secret_reference,
        session_reference,
        default_bucket,
        root_prefix: normalize_root_prefix(draft.root_prefix.as_deref())?,
        addressing_style: draft
            .addressing_style
            .unwrap_or(provider.default_addressing_style()),
        allow_insecure_http: draft.allow_insecure_http,
        favorite: draft.favorite,
        favorite_order: 0,
        revision: 1,
        created_at,
        updated_at,
    };
    if draft.credential_mode == crate::domain::provider::CredentialMode::Static
        && draft.secret_access_key.is_clear()
    {
        return Err(AppError::Validation(
            "Static credentials cannot be cleared; enter a replacement secret".to_string(),
        ));
    }
    profile.validate()?;
    Ok(profile)
}

fn normalize_root_prefix(value: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.starts_with('/')
        || value.contains('\\')
        || value.contains("..")
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Root prefix is invalid".to_string()));
    }
    Ok(Some(if value.ends_with('/') {
        value.to_string()
    } else {
        format!("{value}/")
    }))
}

fn normalize_location_prefix(
    profile: &ConnectionProfile,
    requested: &str,
) -> Result<String, AppError> {
    if requested.starts_with('/') || requested.contains('\\') {
        return Err(AppError::RootPrefixViolation);
    }
    if requested.split('/').any(|segment| segment == "..") {
        return Err(AppError::RootPrefixViolation);
    }
    if let Some(root) = &profile.root_prefix {
        let root_without_separator = root.trim_end_matches('/');
        if requested.is_empty() {
            return Ok(root.clone());
        }
        // Treat the root as a path segment, not a string prefix.  Without this
        // exact-segment check, a root of `foo/` would also expose `foobar/...`.
        if requested == root_without_separator {
            return Ok(root.clone());
        }
        if !requested.starts_with(root) {
            return Err(AppError::Validation(
                "location is outside the profile root prefix".to_string(),
            ));
        }
    }
    Ok(requested.to_string())
}

fn validate_explorer_location(
    location: &ExplorerLocation,
    continuation_token: Option<&str>,
) -> Result<(), AppError> {
    if Uuid::parse_str(&location.profile_id).is_err() {
        return Err(AppError::Validation("profileId is invalid".to_string()));
    }
    if !(3..=255).contains(&location.bucket.chars().count())
        || location.bucket.len() > 255
        || location.bucket.contains('/')
        || location.bucket.contains('\\')
        || location.bucket.chars().any(char::is_control)
    {
        return Err(AppError::Validation("bucket is invalid".to_string()));
    }
    if location.prefix.len() > 1_024
        || location.prefix.contains('\\')
        || location.prefix.chars().any(char::is_control)
    {
        return Err(AppError::Validation("prefix is invalid".to_string()));
    }
    if continuation_token.is_some_and(|token| token.len() > 8_192) {
        return Err(AppError::Validation(
            "continuationToken is too long".to_string(),
        ));
    }
    Ok(())
}

fn required_secret(draft: &ProfileDraft) -> Result<SecretString, AppError> {
    let value = match &draft.secret_access_key {
        SecretInput::Replace(value) if !value.is_empty() => value,
        _ => {
            return Err(AppError::CredentialMissing(
                "secret access key is required".to_string(),
            ))
        }
    };
    Ok(SecretString::new(value.clone().into()))
}

fn to_detail(profile: &ConnectionProfile) -> ProfileDetail {
    ProfileDetail {
        schema_version: 1,
        id: profile.id.to_string(),
        name: profile.name.clone(),
        provider: profile.provider,
        endpoint: profile.endpoint.clone(),
        region: profile.region.clone(),
        credential_mode: profile.credential_mode,
        access_key_preview: profile.access_key_id.as_ref().map(|value| {
            let character_count = value.chars().count();
            if character_count <= 8 {
                "••••".to_string()
            } else {
                let prefix = value.chars().take(4).collect::<String>();
                let suffix = value
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>();
                format!("{prefix}…{suffix}")
            }
        }),
        has_secret_access_key: profile.secret_reference.is_some(),
        has_session_token: profile.session_reference.is_some(),
        default_bucket: profile.default_bucket.clone(),
        root_prefix: profile.root_prefix.clone(),
        addressing_style: profile.addressing_style,
        allow_insecure_http: profile.allow_insecure_http,
        favorite: profile.favorite,
        favorite_order: profile.favorite_order,
        revision: profile.revision,
    }
}

fn display_name(prefix: &str, key: &str) -> String {
    key.strip_prefix(prefix)
        .unwrap_or(key)
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_string()
}

fn entry_for_prefix(location: &ExplorerLocation, key: &str) -> EntrySummary {
    EntrySummary {
        schema_version: 1,
        id: format!("{}:{}:{}:prefix", location.profile_id, location.bucket, key),
        kind: EntryKind::Prefix,
        display_name: display_name(&location.prefix, key),
        key: key.to_string(),
        size: None,
        last_modified: None,
        storage_class: None,
        content_type_hint: None,
        is_folder_marker: false,
    }
}

fn safe_provider_message(error: &impl std::fmt::Display) -> String {
    let message = error.to_string().replace(['\r', '\n'], " ");
    let lower = message.to_ascii_lowercase();
    // SDK display strings can include request URLs or authorization material.
    // Do not surface those values through the public IPC error envelope.
    if [
        "authorization",
        "accesskey",
        "access key",
        "secretaccesskey",
        "secret key",
        "x-amz-signature",
        "x-amz-credential",
        "x-amz-security-token",
        "x-amz-algorithm",
        "x-amz-date",
        "presign",
        "presigned",
        "http://",
        "https://",
        "akia",
        "asia",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "The provider rejected the request.".to_string();
    }
    message.chars().take(240).collect::<String>()
        + if message.chars().count() > 240 {
            "…"
        } else {
            ""
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        profile::ConnectionProfile,
        provider::{AddressingStyle, CredentialMode, ProviderType},
    };
    use uuid::Uuid;

    fn profile(root_prefix: Option<&str>) -> ConnectionProfile {
        let now = Utc::now();
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "root test".to_string(),
            provider: ProviderType::CustomS3,
            endpoint: Some("https://s3.example.test".to_string()),
            region: "us-east-1".to_string(),
            credential_mode: CredentialMode::Static,
            access_key_id: Some("access".to_string()),
            secret_reference: None,
            session_reference: None,
            default_bucket: None,
            root_prefix: root_prefix.map(ToString::to_string),
            addressing_style: AddressingStyle::Path,
            allow_insecure_http: false,
            favorite: false,
            favorite_order: 0,
            revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn root_prefix_is_an_exact_path_segment() {
        let profile = profile(Some("foo/"));
        assert_eq!(normalize_location_prefix(&profile, "foo").unwrap(), "foo/");
        assert_eq!(
            normalize_location_prefix(&profile, "foo/bar").unwrap(),
            "foo/bar"
        );
        assert!(normalize_location_prefix(&profile, "foobar").is_err());
    }

    #[test]
    fn object_key_must_stay_inside_profile_root() {
        let profile = profile(Some("foo/"));
        assert_eq!(
            normalize_object_key(&profile, "foo/file.txt").unwrap(),
            "foo/file.txt"
        );
        assert!(matches!(
            normalize_object_key(&profile, "foo"),
            Err(AppError::RootPrefixViolation)
        ));
        assert!(matches!(
            normalize_object_key(&profile, "foobar/file.txt"),
            Err(AppError::RootPrefixViolation)
        ));
        assert!(matches!(
            normalize_object_key(&profile, "foo/../secret"),
            Err(AppError::RootPrefixViolation)
        ));
    }

    #[test]
    fn preview_content_types_are_allow_listed() {
        assert!(matches!(
            preview_kind_for_content_type("text/plain; charset=utf-8"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_content_type("application/json"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_content_type("text/html"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_content_type("image/svg+xml"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_content_type("image/png"),
            Some(PreviewKind::Image)
        ));
        assert!(matches!(
            preview_kind_for_content_type("video/webm"),
            Some(PreviewKind::Video)
        ));
        assert!(preview_kind_for_content_type("application/octet-stream").is_none());
    }

    #[test]
    fn share_link_expiry_is_bounded() {
        assert_eq!(normalize_share_expiry(None), 3_600);
        assert_eq!(normalize_share_expiry(Some(1)), 300);
        assert_eq!(
            normalize_share_expiry(Some(MAX_SHARE_SECONDS + 1)),
            MAX_SHARE_SECONDS
        );
        assert_eq!(normalize_share_expiry(Some(3_600)), 3_600);
    }
}
