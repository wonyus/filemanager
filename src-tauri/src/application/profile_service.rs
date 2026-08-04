use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use aws_sdk_s3::presigning::PresigningConfig;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::SecretString;
use tokio::{fs, io::AsyncReadExt, sync::Mutex};
use uuid::Uuid;

use crate::{
    domain::{
        error::{is_credential_expired_message, AppError},
        profile::{ConnectionProfile, SecretReference},
        provider::ProviderType,
    },
    dto::{
        explorer::{
            EntryKind, EntrySummary, ExplorerLocation, ListEntriesPage, ListEntriesRequest,
        },
        metadata::{
            MetadataEditRequest, MetadataEditResult, ObjectMetadata, ObjectRequest, PreviewKind,
            PreviewRequest, PreviewResult, ShareLink, ShareLinkRequest,
            DEFAULT_PREVIEW_LIMIT_BYTES, MAX_SHARE_SECONDS, METADATA_SCHEMA_VERSION,
        },
        profile::{
            BucketSummary, ConnectionTestResult, ProfileDetail, ProfileDraft,
            ProfileExportDocument, ProfileExportEntry, ProfileExportRequest, ProfileExportResult,
            ProfileImportRejection, ProfileImportRequest, ProfileImportResult, ProfileSummary,
            SecretInput, PROFILE_EXPORT_SCHEMA_VERSION,
        },
    },
    infrastructure::{
        credentials::{resolve_profile_credentials, CredentialStore, ResolvedCredentials},
        database::Database,
        s3::S3ClientManager,
    },
    transfer::{settings::SettingsService, TransferManager},
};

const MAX_SAFE_IMAGE_PIXELS: u64 = 100_000_000;
const MAX_IMAGE_HEADER_BYTES: u64 = 1024 * 1024;
/// A single binary fallback is deliberately much smaller than the aggregate
/// cache quota so a preview request cannot consume all renderer/app memory.
const MAX_BINARY_FALLBACK_BYTES: u64 = 32 * 1024 * 1024;
const PREVIEW_HANDLE_SECONDS: u32 = 900;

struct BinaryPreviewCacheEntry {
    bytes: Arc<Vec<u8>>,
    content_type: String,
    created_at: Instant,
    last_access_at: Instant,
}

#[derive(Default)]
struct BinaryPreviewCache {
    entries: HashMap<String, BinaryPreviewCacheEntry>,
    total_bytes: u64,
}

struct BinaryPreviewRequest<'a> {
    profile: &'a ConnectionProfile,
    client: &'a aws_sdk_s3::Client,
    bucket: &'a str,
    key: &'a str,
    preview_kind: PreviewKind,
    content_type: String,
    etag: Option<&'a str>,
    version_id: Option<&'a str>,
    total_size: Option<u64>,
}

impl BinaryPreviewCache {
    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(u64::try_from(entry.bytes.len()).unwrap_or(u64::MAX));
        }
    }

    fn prune(&mut self, quota_bytes: u64, max_age: Duration, now: Instant) {
        let expired = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                (now.duration_since(entry.created_at) > max_age).then_some(key.clone())
            })
            .collect::<Vec<_>>();
        for key in expired {
            self.remove(&key);
        }
        while self.total_bytes > quota_bytes {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access_at)
                .map(|(key, _)| key.clone());
            let Some(key) = oldest else { break };
            self.remove(&key);
        }
    }

    fn get(
        &mut self,
        key: &str,
        quota_bytes: u64,
        max_age: Duration,
        now: Instant,
    ) -> Option<(Arc<Vec<u8>>, String)> {
        self.prune(quota_bytes, max_age, now);
        let entry = self.entries.get_mut(key)?;
        entry.last_access_at = now;
        Some((entry.bytes.clone(), entry.content_type.clone()))
    }

    fn insert(
        &mut self,
        key: String,
        bytes: Vec<u8>,
        content_type: String,
        quota_bytes: u64,
        max_age: Duration,
        now: Instant,
    ) {
        let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if quota_bytes == 0 || bytes_len > quota_bytes {
            return;
        }
        self.remove(&key);
        self.total_bytes = self.total_bytes.saturating_add(bytes_len);
        self.entries.insert(
            key,
            BinaryPreviewCacheEntry {
                bytes: Arc::new(bytes),
                content_type,
                created_at: now,
                last_access_at: now,
            },
        );
        self.prune(quota_bytes, max_age, now);
    }
}

pub struct ProfileService {
    database: Database,
    credentials: Arc<dyn CredentialStore>,
    clients: Arc<S3ClientManager>,
    lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
    transfers: Arc<TransferManager>,
    settings: Arc<SettingsService>,
    binary_preview_cache: Arc<Mutex<BinaryPreviewCache>>,
}

impl ProfileService {
    pub fn new(
        database: Database,
        credentials: Arc<dyn CredentialStore>,
        clients: Arc<S3ClientManager>,
        transfers: Arc<TransferManager>,
        settings: Arc<SettingsService>,
    ) -> Self {
        Self {
            database,
            credentials,
            clients,
            lifecycle_lock: Arc::new(tokio::sync::Mutex::new(())),
            transfers,
            settings,
            binary_preview_cache: Arc::new(Mutex::new(BinaryPreviewCache::default())),
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

    /// Export provider configuration without any OS-vault reference or
    /// credential value. The file is written atomically and is bounded to a
    /// small, explicit profile count so it cannot become an unbounded IPC
    /// or filesystem operation.
    pub async fn export_profiles(
        &self,
        request: ProfileExportRequest,
    ) -> Result<ProfileExportResult, AppError> {
        if request.schema_version != PROFILE_EXPORT_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported profile export schema version".to_string(),
            ));
        }
        if request.profile_ids.is_empty() || request.profile_ids.len() > 100 {
            return Err(AppError::Validation(
                "profile export must contain between 1 and 100 profiles".to_string(),
            ));
        }
        let destination = profile_file_path(&request.destination_path)?;
        let mut profiles = Vec::with_capacity(request.profile_ids.len());
        for id in &request.profile_ids {
            let profile = self.load_profile(id).await?;
            profiles.push(ProfileExportEntry {
                id: profile.id.to_string(),
                name: profile.name,
                provider: profile.provider,
                account_id: None,
                endpoint: profile.endpoint,
                region: profile.region,
                credential_mode: profile.credential_mode,
                access_key_id: profile.access_key_id,
                default_bucket: profile.default_bucket,
                root_prefix: profile.root_prefix,
                addressing_style: profile.addressing_style,
                allow_insecure_http: profile.allow_insecure_http,
                favorite: profile.favorite,
            });
        }
        let document = ProfileExportDocument {
            schema_version: PROFILE_EXPORT_SCHEMA_VERSION,
            exported_at: Utc::now().to_rfc3339(),
            profiles,
        };
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| {
            AppError::Unknown(format!("profile export encoding failed: {error}"))
        })?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let temporary = destination.with_extension("tmp");
        fs::write(&temporary, &bytes).await?;
        fs::rename(&temporary, &destination).await?;
        Ok(ProfileExportResult {
            schema_version: PROFILE_EXPORT_SCHEMA_VERSION,
            path: destination.to_string_lossy().into_owned(),
            profile_count: document.profiles.len(),
            redacted: true,
        })
    }

    /// Import portable configuration into new local UUIDs. Imported
    /// credentials are intentionally absent; the user must enter them in the
    /// profile editor before testing or using the connection. Provider
    /// endpoints and roots are validated by the same Rust rules as normal
    /// profile creation, so exported capability claims cannot bypass checks.
    pub async fn import_profiles(
        &self,
        request: ProfileImportRequest,
    ) -> Result<ProfileImportResult, AppError> {
        if request.schema_version != PROFILE_EXPORT_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported profile import schema version".to_string(),
            ));
        }
        let source = profile_file_path(&request.source_path)?;
        let bytes = fs::read(&source).await?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(AppError::Validation(
                "profile import file exceeds the 2 MiB safety limit".to_string(),
            ));
        }
        let document: ProfileExportDocument = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::Validation(format!("invalid profile export: {error}")))?;
        if document.schema_version != PROFILE_EXPORT_SCHEMA_VERSION {
            return Err(AppError::Validation(
                "unsupported profile export schema version".to_string(),
            ));
        }
        if document.profiles.is_empty() || document.profiles.len() > 100 {
            return Err(AppError::Validation(
                "profile import must contain between 1 and 100 profiles".to_string(),
            ));
        }
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let mut imported = Vec::new();
        let mut rejected = Vec::new();
        for entry in document.profiles {
            let name = entry.name.clone();
            let draft = ProfileDraft {
                schema_version: 1,
                id: None,
                name: entry.name,
                provider: entry.provider,
                account_id: entry.account_id,
                endpoint: entry.endpoint,
                region: entry.region,
                credential_mode: entry.credential_mode,
                access_key_id: entry.access_key_id,
                secret_access_key: SecretInput::Unchanged,
                session_token: SecretInput::Unchanged,
                default_bucket: entry.default_bucket,
                root_prefix: entry.root_prefix,
                addressing_style: Some(entry.addressing_style),
                // Imported insecure HTTP is never enabled implicitly.
                allow_insecure_http: false,
                favorite: entry.favorite,
            };
            let now = Utc::now();
            let candidate_id = Uuid::new_v4();
            let placeholder_secret = SecretReference::new(candidate_id, "import-placeholder");
            let placeholder_session =
                SecretReference::new(candidate_id, "import-session-placeholder");
            let candidate = match profile_from_draft(
                &draft,
                candidate_id,
                now,
                now,
                Some(placeholder_secret),
                (entry.credential_mode
                    == crate::domain::provider::CredentialMode::TemporarySession)
                    .then_some(placeholder_session),
            ) {
                Ok(profile) => profile,
                Err(AppError::Validation(reason)) => {
                    rejected.push(ProfileImportRejection { name, reason });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut candidate = candidate;
            candidate.secret_reference = None;
            candidate.session_reference = None;
            if let Err(error) = candidate.validate_configuration() {
                rejected.push(ProfileImportRejection {
                    name,
                    reason: error.to_string(),
                });
                continue;
            }
            if let Err(error) = self.database.insert_profile(&candidate).await {
                rejected.push(ProfileImportRejection {
                    name,
                    reason: error.to_string(),
                });
                continue;
            }
            imported.push(to_detail(&candidate));
        }
        Ok(ProfileImportResult {
            schema_version: PROFILE_EXPORT_SCHEMA_VERSION,
            imported,
            rejected,
            credentials_required: true,
        })
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
        let mut metadata = metadata_from_head(&profile, &request, &key, &head);
        self.apply_share_capability(&profile, &mut metadata).await?;
        Ok(metadata)
    }

    /// Replace editable HTTP/user metadata by copying an object onto itself.
    /// S3-compatible providers implement this as a non-atomic operation, so
    /// callers receive an explicit warning and a fresh HeadObject snapshot.
    pub async fn edit_metadata(
        &self,
        request: MetadataEditRequest,
    ) -> Result<MetadataEditResult, AppError> {
        if request.content_type.is_none()
            && request.content_disposition.is_none()
            && request.cache_control.is_none()
            && request.user_metadata.is_none()
        {
            return Err(AppError::Validation(
                "metadata edit must change at least one field".to_string(),
            ));
        }
        validate_metadata_patch(&request)?;
        let object_request = ObjectRequest {
            schema_version: request.schema_version,
            profile_id: request.profile_id.clone(),
            bucket: request.bucket.clone(),
            key: request.key.clone(),
        };
        let (profile, client, key) = self.object_context(&object_request).await?;
        let head = self.fetch_head(&client, &request.bucket, &key).await?;

        let mut user_metadata = head.metadata().cloned().unwrap_or_default();
        if let Some(replacement) = request.user_metadata {
            user_metadata = replacement.into_iter().collect::<HashMap<_, _>>();
        }
        let mut operation = client
            .copy_object()
            .copy_source(encode_copy_source(&request.bucket, &key))
            .bucket(&request.bucket)
            .key(&key)
            .metadata_directive(aws_sdk_s3::types::MetadataDirective::Replace)
            .set_metadata(Some(user_metadata));
        if let Some(value) = request
            .content_type
            .or_else(|| head.content_type().map(ToString::to_string))
        {
            operation = operation.content_type(value);
        }
        if let Some(value) = request
            .content_disposition
            .or_else(|| head.content_disposition().map(ToString::to_string))
        {
            operation = operation.content_disposition(value);
        }
        if let Some(value) = request
            .cache_control
            .or_else(|| head.cache_control().map(ToString::to_string))
        {
            operation = operation.cache_control(value);
        }
        if let Some(value) = head.content_encoding() {
            operation = operation.content_encoding(value);
        }
        if let Some(value) = head.content_language() {
            operation = operation.content_language(value);
        }
        #[allow(deprecated)]
        let expires = head.expires().cloned();
        if let Some(value) = expires {
            operation = operation.expires(value);
        }
        if let Some(value) = head.website_redirect_location() {
            operation = operation.website_redirect_location(value);
        }
        operation
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))?;

        let updated = self.fetch_head(&client, &request.bucket, &key).await?;
        let mut metadata = metadata_from_head(&profile, &object_request, &key, &updated);
        self.apply_share_capability(&profile, &mut metadata).await?;
        Ok(MetadataEditResult {
            schema_version: METADATA_SCHEMA_VERSION,
            metadata,
            warning: "Metadata replacement uses a non-atomic copy-on-self operation; the object may briefly be unavailable.".to_string(),
        })
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
        let declared_content_type = head.content_type().unwrap_or("application/octet-stream");
        let Some(preview_kind) = preview_kind_for_object(declared_content_type, &key) else {
            return Err(AppError::UnsupportedProviderFeature(
                "preview is unavailable for this content type; use Properties or Download"
                    .to_string(),
            ));
        };
        // Keep provider metadata separate from inferred display metadata.  A
        // number of S3-compatible gateways return octet-stream for every
        // object, so extension inference is used only for preview policy and
        // the ephemeral result content type.
        let content_type = effective_preview_content_type(declared_content_type, &key);
        let total_size = head
            .content_length()
            .and_then(|size| u64::try_from(size).ok());

        // Binary media and PDF previews prefer a short-lived in-memory bearer
        // URL.  Image dimensions are checked before handing a remote URL to
        // the renderer so a decompression bomb cannot make the browser decode
        // more than the 100 megapixel safety budget.
        if !matches!(preview_kind, PreviewKind::Text) {
            if matches!(preview_kind, PreviewKind::Image) {
                self.validate_image_dimensions(&client, &request.bucket, &key, &content_type)
                    .await?;
            }
            let (presign_supported, _) = self.share_capability(&profile).await?;
            if presign_supported {
                let (url, expires_at) = self
                    .presigned_get(&client, &request.bucket, &key, PREVIEW_HANDLE_SECONDS)
                    .await?;
                return Ok(PreviewResult {
                    schema_version: METADATA_SCHEMA_VERSION,
                    profile_id: profile.id.to_string(),
                    bucket: request.bucket,
                    key,
                    preview_kind,
                    content_type,
                    text: String::new(),
                    url: Some(url),
                    data_url: None,
                    expires_at: Some(expires_at),
                    bytes_read: 0,
                    total_size,
                    truncated: false,
                });
            }
            return self
                .binary_preview_fallback(BinaryPreviewRequest {
                    profile: &profile,
                    client: &client,
                    bucket: &request.bucket,
                    key: &key,
                    preview_kind,
                    content_type,
                    etag: head.e_tag(),
                    version_id: head.version_id(),
                    total_size,
                })
                .await;
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
            content_type,
            text,
            url: None,
            data_url: None,
            expires_at: None,
            bytes_read: u32::try_from(bytes.len()).unwrap_or(max_bytes),
            total_size,
            truncated,
        })
    }

    async fn validate_image_dimensions(
        &self,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
        content_type: &str,
    ) -> Result<(), AppError> {
        let bytes = self
            .read_bounded_object(
                client,
                bucket,
                key,
                MAX_IMAGE_HEADER_BYTES,
                Some(MAX_IMAGE_HEADER_BYTES),
            )
            .await?;
        let Some((width, height)) = image_dimensions(&bytes, content_type) else {
            return Err(AppError::UnsupportedProviderFeature(
                "image dimensions could not be verified safely; use Download instead".to_string(),
            ));
        };
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| {
                AppError::UnsupportedProviderFeature(
                    "image dimensions exceed the safe preview limit".to_string(),
                )
            })?;
        if pixels > MAX_SAFE_IMAGE_PIXELS {
            return Err(AppError::UnsupportedProviderFeature(format!(
                "image is too large to preview safely ({width}×{height}; maximum is 100 megapixels)"
            )));
        }
        Ok(())
    }

    async fn binary_preview_fallback(
        &self,
        request: BinaryPreviewRequest<'_>,
    ) -> Result<PreviewResult, AppError> {
        let BinaryPreviewRequest {
            profile,
            client,
            bucket,
            key,
            preview_kind,
            content_type,
            etag,
            version_id,
            total_size,
        } = request;
        let settings = self.settings.get().await;
        let quota_bytes = settings.preview_cache_bytes;
        let per_object_limit = quota_bytes.min(MAX_BINARY_FALLBACK_BYTES);
        if per_object_limit == 0 {
            return Err(AppError::UnsupportedProviderFeature(
                "binary preview cache is disabled; use Download instead".to_string(),
            ));
        }
        if total_size.is_some_and(|size| size > per_object_limit) {
            return Err(AppError::UnsupportedProviderFeature(format!(
                "binary object exceeds the bounded preview fallback ({per_object_limit} bytes); use Download instead"
            )));
        }
        let max_age = Duration::from_secs(
            u64::from(settings.preview_cache_max_age_hours).saturating_mul(60 * 60),
        );
        let cache_key = format!(
            "{}\u{1f}{bucket}\u{1f}{key}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{content_type}",
            profile.id,
            etag.unwrap_or_default(),
            version_id.unwrap_or_default(),
            total_size.unwrap_or_default(),
        );
        let now = Instant::now();
        let cached =
            self.binary_preview_cache
                .lock()
                .await
                .get(&cache_key, quota_bytes, max_age, now);
        let bytes = if let Some((bytes, _cached_content_type)) = cached {
            bytes
        } else {
            let bytes = self
                .read_bounded_object(client, bucket, key, per_object_limit, None)
                .await?;
            self.binary_preview_cache.lock().await.insert(
                cache_key,
                bytes.clone(),
                content_type.clone(),
                quota_bytes,
                max_age,
                now,
            );
            Arc::new(bytes)
        };
        let data_url = format!(
            "data:{};base64,{}",
            safe_data_url_content_type(&content_type),
            BASE64.encode(bytes.as_ref()),
        );
        let expires_at =
            (Utc::now() + ChronoDuration::seconds(i64::from(PREVIEW_HANDLE_SECONDS))).to_rfc3339();
        Ok(PreviewResult {
            schema_version: METADATA_SCHEMA_VERSION,
            profile_id: profile.id.to_string(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            preview_kind,
            content_type,
            text: String::new(),
            url: None,
            data_url: Some(data_url),
            expires_at: Some(expires_at),
            bytes_read: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            total_size: total_size.or_else(|| u64::try_from(bytes.len()).ok()),
            truncated: false,
        })
    }

    async fn read_bounded_object(
        &self,
        client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
        max_bytes: u64,
        range_limit: Option<u64>,
    ) -> Result<Vec<u8>, AppError> {
        let mut operation = client.get_object().bucket(bucket).key(key);
        if let Some(limit) = range_limit {
            operation = operation.range(format!("bytes=0-{}", limit.saturating_sub(1)));
        }
        let output = operation
            .send()
            .await
            .map_err(|error| AppError::Provider(safe_provider_message(&error)))?;
        let mut stream = output
            .body
            .into_async_read()
            .take(max_bytes.saturating_add(1));
        let mut bytes =
            Vec::with_capacity(usize::try_from(max_bytes.min(1024 * 1024)).unwrap_or(0));
        stream.read_to_end(&mut bytes).await.map_err(AppError::Io)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(AppError::UnsupportedProviderFeature(format!(
                "binary preview exceeds the bounded {max_bytes}-byte cache limit; use Download instead"
            )));
        }
        Ok(bytes)
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
        self.ensure_presigned_get_supported(&profile).await?;
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

    async fn apply_share_capability(
        &self,
        profile: &ConnectionProfile,
        metadata: &mut ObjectMetadata,
    ) -> Result<(), AppError> {
        let (supported, reason) = self.share_capability(profile).await?;
        metadata.share_supported = supported;
        metadata.share_reason = reason.clone();
        // Binary previews and PDF use the same presigned GET capability as a
        // share link. Text previews are bounded authenticated reads and remain
        // available even when the provider cannot mint bearer URLs.
        if !supported && !matches!(metadata.preview_kind, Some(PreviewKind::Text) | None) {
            metadata.preview_supported = false;
            metadata.preview_reason = reason;
        }
        Ok(())
    }

    async fn ensure_presigned_get_supported(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<(), AppError> {
        let (supported, reason) = self.share_capability(profile).await?;
        if supported {
            return Ok(());
        }
        Err(AppError::UnsupportedProviderFeature(reason.unwrap_or_else(
            || "temporary share links are unavailable for this provider".to_string(),
        )))
    }

    async fn share_capability(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<(bool, Option<String>), AppError> {
        let profile_id = profile.id.to_string();
        let observed = self
            .database
            .get_profile_capabilities(&profile_id)
            .await?
            .and_then(|value| value.supports_presigned_get);
        if observed == Some(false) {
            return Ok((
                false,
                Some(
                    "the provider capability check reported that presigned GET is unavailable"
                        .to_string(),
                ),
            ));
        }
        if profile.provider == ProviderType::CustomS3 && observed != Some(true) {
            return Ok((
                false,
                Some("share links are disabled for unknown/custom providers until a capability check confirms presigned GET support".to_string()),
            ));
        }
        Ok((true, None))
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

fn validate_metadata_patch(request: &MetadataEditRequest) -> Result<(), AppError> {
    for (field, value, max_len) in [
        ("contentType", request.content_type.as_deref(), 256_usize),
        (
            "contentDisposition",
            request.content_disposition.as_deref(),
            1_024_usize,
        ),
        (
            "cacheControl",
            request.cache_control.as_deref(),
            1_024_usize,
        ),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty()
                || value.len() > max_len
                || value.chars().any(char::is_control)
            {
                return Err(AppError::Validation(format!("{field} is invalid")));
            }
        }
    }
    if let Some(metadata) = &request.user_metadata {
        if metadata.len() > 100 {
            return Err(AppError::Validation(
                "userMetadata cannot contain more than 100 entries".to_string(),
            ));
        }
        let total_bytes = metadata
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()))
            .sum::<usize>();
        if total_bytes > 8_192
            || metadata.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 128
                    || value.len() > 2_048
                    || key.chars().any(char::is_control)
                    || value.chars().any(char::is_control)
            })
        {
            return Err(AppError::Validation(
                "userMetadata contains an invalid key or value".to_string(),
            ));
        }
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

/// S3-compatible gateways frequently return `application/octet-stream` for
/// text files. A conservative extension fallback keeps common UTF-8 logs and
/// config files previewable without ever treating active content as HTML.
fn preview_kind_for_object(content_type: &str, key: &str) -> Option<PreviewKind> {
    preview_kind_for_content_type(content_type).or_else(|| {
        let media_type = content_type
            .split(';')
            .next()
            .unwrap_or(content_type)
            .trim()
            .to_ascii_lowercase();
        if !media_type.is_empty() && media_type != "application/octet-stream" {
            return None;
        }
        preview_kind_for_content_type(infer_content_type_for_key(key).unwrap_or_default())
    })
}

fn infer_content_type_for_key(key: &str) -> Option<&'static str> {
    let extension = key
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref()? {
        "txt" | "log" | "conf" | "cfg" | "properties" | "env" => Some("text/plain"),
        "json" | "jsonl" => Some("application/json"),
        "xml" => Some("application/xml"),
        "md" | "markdown" => Some("text/markdown"),
        "csv" => Some("text/csv"),
        "yaml" | "yml" => Some("application/yaml"),
        "toml" => Some("application/toml"),
        "rs" | "ts" | "tsx" | "js" | "jsx" => Some("text/plain"),
        "css" => Some("text/css"),
        "html" | "htm" => Some("text/html"),
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

fn effective_preview_content_type(content_type: &str, key: &str) -> String {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    if !media_type.is_empty() && !media_type.eq_ignore_ascii_case("application/octet-stream") {
        return media_type.to_ascii_lowercase();
    }
    infer_content_type_for_key(key)
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn safe_data_url_content_type(content_type: &str) -> String {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if matches!(
        media_type.as_str(),
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/ogg"
            | "video/mp4"
            | "video/webm"
            | "application/pdf"
    ) {
        media_type
    } else {
        "application/octet-stream".to_string()
    }
}

fn image_dimensions(bytes: &[u8], content_type: &str) -> Option<(u32, u32)> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "image/png" => png_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        "image/bmp" => bmp_dimensions(bytes),
        _ => None,
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || (!bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a")) {
        return None;
    }
    let width = u32::from(u16::from_le_bytes(bytes[6..8].try_into().ok()?));
    let height = u32::from(u16::from_le_bytes(bytes[8..10].try_into().ok()?));
    (width > 0 && height > 0).then_some((width, height))
}

fn bmp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 26 || &bytes[0..2] != b"BM" {
        return None;
    }
    let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?);
    let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?);
    let width = width.unsigned_abs();
    let height = height.unsigned_abs();
    (width > 0 && height > 0).then_some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || &bytes[0..2] != b"\xff\xd8" {
        return None;
    }
    let mut offset = 2_usize;
    while offset + 1 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(offset)?,
            *bytes.get(offset + 1)?,
        ]));
        if length < 2 || offset + length > bytes.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            if length < 7 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]));
            return (width > 0 && height > 0).then_some((width, height));
        }
        offset += length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 16 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut offset = 12_usize;
    while offset + 8 <= bytes.len() {
        let chunk = &bytes[offset..offset + 4];
        let size = usize::try_from(u32::from_le_bytes(
            bytes[offset + 4..offset + 8].try_into().ok()?,
        ))
        .ok()?;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(size)?;
        if data_end > bytes.len() {
            return None;
        }
        let data = &bytes[data_start..data_end];
        if chunk == b"VP8X" && data.len() >= 10 {
            let width =
                1 + u32::from(data[4]) + (u32::from(data[5]) << 8) + (u32::from(data[6]) << 16);
            let height =
                1 + u32::from(data[7]) + (u32::from(data[8]) << 8) + (u32::from(data[9]) << 16);
            return (width > 0 && height > 0).then_some((width, height));
        }
        if chunk == b"VP8 " && data.len() >= 10 && data[3..6] == [0x9d, 0x01, 0x2a] {
            let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
            let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
            return (width > 0 && height > 0).then_some((width, height));
        }
        if chunk == b"VP8L" && data.len() >= 5 && data[0] == 0x2f {
            let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
            let width = 1 + (bits & 0x3fff);
            let height = 1 + ((bits >> 14) & 0x3fff);
            return (width > 0 && height > 0).then_some((width, height));
        }
        offset = data_end + (size & 1);
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
    let preview_kind = preview_kind_for_object(
        content_type
            .as_deref()
            .unwrap_or("application/octet-stream"),
        &request.key,
    );
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
        // A metadata snapshot is refined by `apply_share_capability` before
        // it crosses IPC. Keep conservative defaults for internal callers.
        share_supported: false,
        share_reason: Some("checking provider capability".to_string()),
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

fn profile_file_path(value: &str) -> Result<PathBuf, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "profile file path is invalid".to_string(),
        ));
    }
    let path = Path::new(trimmed);
    if path.file_name().is_none() {
        return Err(AppError::Validation(
            "profile file path must name a file".to_string(),
        ));
    }
    Ok(path.to_path_buf())
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
    if is_credential_expired_message(&message) {
        return "credential expired".to_string();
    }
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
    use crate::domain::{
        profile::ConnectionProfile,
        provider::{AddressingStyle, CredentialMode, ProviderType},
    };
    use std::collections::BTreeMap;
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
        assert!(matches!(
            preview_kind_for_object("application/octet-stream", "logs/app.log"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_object("application/octet-stream", "web/index.html"),
            Some(PreviewKind::Text)
        ));
        assert!(matches!(
            preview_kind_for_object("application/octet-stream", "images/photo.png"),
            Some(PreviewKind::Image)
        ));
        assert_eq!(
            effective_preview_content_type("application/octet-stream", "images/photo.png"),
            "image/png"
        );
    }

    #[test]
    fn image_dimension_guard_parses_allowlisted_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&4_u32.to_be_bytes());
        png.extend_from_slice(&3_u32.to_be_bytes());
        assert_eq!(image_dimensions(&png, "image/png"), Some((4, 3)));

        let gif = b"GIF89a\x10\0\x08\0";
        assert_eq!(image_dimensions(gif, "image/gif"), Some((16, 8)));

        let mut bmp = b"BM".to_vec();
        bmp.resize(26, 0);
        bmp[18..22].copy_from_slice(&1_i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2_i32.to_le_bytes());
        assert_eq!(image_dimensions(&bmp, "image/bmp"), Some((1, 2)));
    }

    #[test]
    fn image_dimension_guard_rejects_unknown_or_oversized_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&10_001_u32.to_be_bytes());
        png.extend_from_slice(&10_001_u32.to_be_bytes());
        let (width, height) = image_dimensions(&png, "image/png").unwrap();
        assert!(u64::from(width) * u64::from(height) > MAX_SAFE_IMAGE_PIXELS);
        assert!(image_dimensions(b"not an image", "image/png").is_none());
    }

    #[test]
    fn data_url_content_type_is_header_safe() {
        assert_eq!(safe_data_url_content_type("image/png"), "image/png");
        assert_eq!(
            safe_data_url_content_type("image/png; charset=utf-8"),
            "image/png"
        );
        assert_eq!(
            safe_data_url_content_type("text/plain\r\ndata:evil"),
            "application/octet-stream"
        );
        assert_eq!(
            safe_data_url_content_type("application/javascript"),
            "application/octet-stream"
        );
    }

    #[test]
    fn metadata_patch_validates_editable_fields_and_user_metadata() {
        let request = MetadataEditRequest {
            schema_version: METADATA_SCHEMA_VERSION,
            profile_id: Uuid::new_v4().to_string(),
            bucket: "bucket".to_string(),
            key: "folder/file.txt".to_string(),
            content_type: Some("text/plain".to_string()),
            content_disposition: None,
            cache_control: Some("max-age=60".to_string()),
            user_metadata: Some(BTreeMap::from([(
                "x-owner".to_string(),
                "desktop".to_string(),
            )])),
        };
        assert!(validate_metadata_patch(&request).is_ok());
        let invalid = MetadataEditRequest {
            content_type: Some("text/\nplain".to_string()),
            ..request
        };
        assert!(validate_metadata_patch(&invalid).is_err());
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
