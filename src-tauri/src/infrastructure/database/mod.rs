use std::path::Path;

use chrono::{DateTime, Utc};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    Row, SqlitePool,
};
use uuid::Uuid;

use crate::{
    domain::{
        error::AppError,
        profile::{ConnectionProfile, SecretReference},
        provider::{AddressingStyle, CredentialMode, ProviderType},
    },
    dto::{
        explorer_state::{
            AddBookmarkRequest, Bookmark, ListBookmarksRequest, ListRecentLocationsRequest,
            RecentLocation, RecordRecentLocationRequest,
        },
        profile::{ConnectionState, CredentialState, ProfileSummary},
        settings::SettingsSnapshot,
        transfer::{
            CollisionPolicy, StartTransferRequest, TransferEndpoint, TransferJob,
            TransferOperation, TransferStatus,
        },
    },
};

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

/// A transfer snapshot loaded from SQLite.  The request is retained so a
/// completed/failed/interrupted job can be retried without asking the
/// renderer to reconstruct provider paths or local destinations.
#[derive(Debug, Clone)]
pub struct PersistedTransfer {
    pub job: TransferJob,
    pub request: StartTransferRequest,
}

impl Database {
    pub async fn connect(path: &Path) -> Result<Self, AppError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn list_profiles(&self) -> Result<Vec<ProfileSummary>, AppError> {
        let rows = sqlx::query(
            "SELECT id, name, provider, endpoint, region, default_bucket, root_prefix,
                    secret_reference, favorite, last_connected_at
             FROM connection_profiles
             ORDER BY favorite DESC, favorite_order ASC, name COLLATE NOCASE ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let provider_value: String = row.try_get("provider")?;
                let provider = ProviderType::parse_known(&provider_value).ok_or_else(|| {
                    AppError::Unknown("unknown provider in profile database".to_string())
                })?;
                Ok(ProfileSummary {
                    schema_version: 1,
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    provider,
                    endpoint_display: redacted_endpoint_display(row.try_get("endpoint")?),
                    region: row.try_get("region")?,
                    default_bucket: row.try_get("default_bucket")?,
                    root_prefix: row
                        .try_get::<Option<String>, _>("root_prefix")?
                        .unwrap_or_default(),
                    favorite: row.try_get::<i64, _>("favorite")? != 0,
                    last_connected_at: row.try_get("last_connected_at")?,
                    credential_state: if row
                        .try_get::<Option<String>, _>("secret_reference")
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        CredentialState::Configured
                    } else {
                        CredentialState::Missing
                    },
                    connection_state: ConnectionState::Unknown,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()
    }

    pub async fn add_bookmark(&self, request: AddBookmarkRequest) -> Result<Bookmark, AppError> {
        validate_schema_version(request.schema_version)?;
        validate_location(self, &request.profile_id, &request.bucket, &request.prefix).await?;
        let name = validate_bookmark_name(&request.name)?;
        let sort_order = request.sort_order.max(0);
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO bookmarks
                (profile_id, bucket, prefix, name, created_at, sort_order)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(profile_id, bucket, prefix) DO UPDATE SET
                name = excluded.name,
                sort_order = excluded.sort_order",
        )
        .bind(&request.profile_id)
        .bind(&request.bucket)
        .bind(&request.prefix)
        .bind(name)
        .bind(&created_at)
        .bind(sort_order)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query(
            "SELECT id, profile_id, bucket, prefix, name, sort_order, created_at
             FROM bookmarks WHERE profile_id = ? AND bucket = ? AND prefix = ?",
        )
        .bind(&request.profile_id)
        .bind(&request.bucket)
        .bind(&request.prefix)
        .fetch_one(&self.pool)
        .await?;
        row_to_bookmark(row)
    }

    pub async fn list_bookmarks(
        &self,
        request: ListBookmarksRequest,
    ) -> Result<Vec<Bookmark>, AppError> {
        validate_schema_version(request.schema_version)?;
        ensure_profile_exists(self, &request.profile_id).await?;
        let rows = sqlx::query(
            "SELECT id, profile_id, bucket, prefix, name, sort_order, created_at
             FROM bookmarks
             WHERE profile_id = ?
             ORDER BY sort_order ASC, name COLLATE NOCASE ASC, id ASC",
        )
        .bind(&request.profile_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_bookmark).collect()
    }

    pub async fn remove_bookmark(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("DELETE FROM bookmarks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_recent_location(
        &self,
        request: RecordRecentLocationRequest,
    ) -> Result<RecentLocation, AppError> {
        validate_schema_version(request.schema_version)?;
        let location = request.location;
        validate_location(
            self,
            &location.profile_id,
            &location.bucket,
            &location.prefix,
        )
        .await?;
        let opened_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO recent_locations (profile_id, bucket, prefix, opened_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(profile_id, bucket, prefix) DO UPDATE SET
                opened_at = excluded.opened_at",
        )
        .bind(&location.profile_id)
        .bind(&location.bucket)
        .bind(&location.prefix)
        .bind(&opened_at)
        .execute(&self.pool)
        .await?;
        // Keep only the latest 30 unique locations per profile. This is a
        // durable bound, not merely a UI paging limit, so long-running use
        // cannot grow the local database without limit.
        sqlx::query(
            "DELETE FROM recent_locations
             WHERE profile_id = ?
               AND id NOT IN (
                   SELECT id FROM recent_locations
                   WHERE profile_id = ?
                   ORDER BY opened_at DESC, id DESC
                   LIMIT 30
               )",
        )
        .bind(&location.profile_id)
        .bind(&location.profile_id)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(
            "SELECT id, profile_id, bucket, prefix, opened_at
             FROM recent_locations
             WHERE profile_id = ? AND bucket = ? AND prefix = ?",
        )
        .bind(&location.profile_id)
        .bind(&location.bucket)
        .bind(&location.prefix)
        .fetch_one(&self.pool)
        .await?;
        row_to_recent_location(row)
    }

    pub async fn list_recent_locations(
        &self,
        request: ListRecentLocationsRequest,
    ) -> Result<Vec<RecentLocation>, AppError> {
        validate_schema_version(request.schema_version)?;
        ensure_profile_exists(self, &request.profile_id).await?;
        let limit = i64::from(request.limit.unwrap_or(30).clamp(1, 100));
        let rows = sqlx::query(
            "SELECT id, profile_id, bucket, prefix, opened_at
             FROM recent_locations
             WHERE profile_id = ?
             ORDER BY opened_at DESC, id DESC
             LIMIT ?",
        )
        .bind(&request.profile_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recent_location).collect()
    }

    pub async fn load_settings(&self) -> Result<Option<SettingsSnapshot>, AppError> {
        let row = sqlx::query("SELECT value_json FROM settings WHERE key = 'app'")
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(None) };
        let value: String = row.try_get("value_json")?;
        let snapshot = serde_json::from_str(&value)
            .map_err(|error| AppError::Unknown(format!("invalid settings JSON: {error}")))?;
        Ok(Some(snapshot))
    }

    pub async fn save_settings(&self, snapshot: &SettingsSnapshot) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO settings (key, value_json, updated_at) VALUES ('app', ?, ?)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                 updated_at = excluded.updated_at",
        )
        .bind(
            serde_json::to_string(snapshot)
                .map_err(|error| AppError::Unknown(error.to_string()))?,
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_profile_capabilities(
        &self,
        profile_id: Uuid,
        capabilities: &crate::dto::profile::ConnectionTestResult,
    ) -> Result<(), AppError> {
        let value = serde_json::to_string(capabilities)
            .map_err(|error| AppError::Unknown(format!("serialize capabilities: {error}")))?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO profile_capabilities (profile_id, capabilities_json, observed_at)
             VALUES (?, ?, ?)
             ON CONFLICT(profile_id) DO UPDATE SET capabilities_json = excluded.capabilities_json,
                 observed_at = excluded.observed_at",
        )
        .bind(profile_id.to_string())
        .bind(value)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE connection_profiles SET last_connected_at = ? WHERE id = ?")
            .bind(now)
            .bind(profile_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_profile(&self, id: &str) -> Result<Option<ConnectionProfile>, AppError> {
        let row = sqlx::query(
            "SELECT id, name, provider, endpoint, region, credential_mode, access_key_id,
                    secret_reference, session_reference, default_bucket, root_prefix,
                    addressing_style, allow_insecure_http, favorite, favorite_order, revision,
                    created_at, updated_at
             FROM connection_profiles WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_profile).transpose()
    }

    pub async fn insert_profile(&self, profile: &ConnectionProfile) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO connection_profiles
             (id, name, provider, endpoint, region, credential_mode, access_key_id,
              secret_reference, session_reference, default_bucket, root_prefix, addressing_style,
              allow_insecure_http, favorite, favorite_order, revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(profile.id.to_string())
        .bind(&profile.name)
        .bind(profile.provider.as_str())
        .bind(&profile.endpoint)
        .bind(&profile.region)
        .bind(profile.credential_mode.as_str())
        .bind(&profile.access_key_id)
        .bind(
            profile
                .secret_reference
                .as_ref()
                .map(|value| value.0.clone()),
        )
        .bind(
            profile
                .session_reference
                .as_ref()
                .map(|value| value.0.clone()),
        )
        .bind(&profile.default_bucket)
        .bind(&profile.root_prefix)
        .bind(profile.addressing_style.as_str())
        .bind(if profile.allow_insecure_http {
            1_i64
        } else {
            0_i64
        })
        .bind(if profile.favorite { 1_i64 } else { 0_i64 })
        .bind(profile.favorite_order)
        .bind(profile.revision)
        .bind(profile.created_at.to_rfc3339())
        .bind(profile.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_profile(
        &self,
        profile: &ConnectionProfile,
        expected_revision: i64,
    ) -> Result<(), AppError> {
        let next_revision = expected_revision + 1;
        let result = sqlx::query(
            "UPDATE connection_profiles SET name = ?, provider = ?, endpoint = ?, region = ?,
             credential_mode = ?, access_key_id = ?, secret_reference = ?, session_reference = ?,
             default_bucket = ?, root_prefix = ?, addressing_style = ?, allow_insecure_http = ?,
             favorite = ?, favorite_order = ?, revision = ?, updated_at = ?
             WHERE id = ? AND revision = ?",
        )
        .bind(&profile.name)
        .bind(profile.provider.as_str())
        .bind(&profile.endpoint)
        .bind(&profile.region)
        .bind(profile.credential_mode.as_str())
        .bind(&profile.access_key_id)
        .bind(
            profile
                .secret_reference
                .as_ref()
                .map(|value| value.0.clone()),
        )
        .bind(
            profile
                .session_reference
                .as_ref()
                .map(|value| value.0.clone()),
        )
        .bind(&profile.default_bucket)
        .bind(&profile.root_prefix)
        .bind(profile.addressing_style.as_str())
        .bind(if profile.allow_insecure_http {
            1_i64
        } else {
            0_i64
        })
        .bind(if profile.favorite { 1_i64 } else { 0_i64 })
        .bind(profile.favorite_order)
        .bind(next_revision)
        .bind(profile.updated_at.to_rfc3339())
        .bind(profile.id.to_string())
        .bind(expected_revision)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::ProfileRevisionConflict);
        }
        Ok(())
    }

    pub async fn delete_profile(&self, id: &str) -> Result<Option<ConnectionProfile>, AppError> {
        let profile = self.get_profile(id).await?;
        if profile.is_some() {
            sqlx::query("DELETE FROM connection_profiles WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(profile)
    }

    /// Atomically removes profile metadata and decrements every logical
    /// credential reference.  The returned references are the ones whose
    /// vault entries may now be deleted after the SQLite transaction commits.
    pub async fn delete_profile_and_release_credentials(
        &self,
        id: &str,
    ) -> Result<Option<(ConnectionProfile, Vec<SecretReference>)>, AppError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, provider, endpoint, region, credential_mode, access_key_id,
                    secret_reference, session_reference, default_bucket, root_prefix,
                    addressing_style, allow_insecure_http, favorite, favorite_order, revision,
                    created_at, updated_at
             FROM connection_profiles WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let profile = row_to_profile(row)?;
        let references = [
            profile.secret_reference.clone(),
            profile.session_reference.clone(),
        ];
        let mut cleanup = Vec::new();
        for reference in references.iter().flatten() {
            let count =
                sqlx::query("SELECT profile_count FROM credential_refs WHERE secret_reference = ?")
                    .bind(reference.as_str())
                    .fetch_optional(&mut *transaction)
                    .await?
                    .map(|row| row.get::<i64, _>("profile_count"))
                    .ok_or_else(|| {
                        AppError::Unknown(
                            "credential reference is missing from database".to_string(),
                        )
                    })?;
            if count <= 1 {
                sqlx::query("DELETE FROM credential_refs WHERE secret_reference = ?")
                    .bind(reference.as_str())
                    .execute(&mut *transaction)
                    .await?;
                cleanup.push(reference.clone());
            } else {
                sqlx::query(
                    "UPDATE credential_refs SET profile_count = profile_count - 1, updated_at = ?
                     WHERE secret_reference = ?",
                )
                .bind(Utc::now().to_rfc3339())
                .bind(reference.as_str())
                .execute(&mut *transaction)
                .await?;
            }
        }
        sqlx::query("DELETE FROM connection_profiles WHERE id = ?")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some((profile, cleanup)))
    }

    /// Returns the profile and the credential references that would become
    /// unreferenced if it were deleted.  This read-only preflight lets the
    /// service remove vault entries before committing destructive metadata.
    pub async fn profile_and_credential_cleanup_candidates(
        &self,
        id: &str,
    ) -> Result<Option<(ConnectionProfile, Vec<SecretReference>)>, AppError> {
        let Some(profile) = self.get_profile(id).await? else {
            return Ok(None);
        };
        let mut candidates = Vec::new();
        for reference in [
            profile.secret_reference.clone(),
            profile.session_reference.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if candidates
                .iter()
                .any(|candidate: &SecretReference| candidate.as_str() == reference.as_str())
            {
                continue;
            }
            if self.credential_ref_count(&reference).await? <= 1 {
                candidates.push(reference);
            }
        }
        Ok(Some((profile, candidates)))
    }

    pub async fn increment_credential_ref(
        &self,
        reference: &SecretReference,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO credential_refs (secret_reference, profile_count, updated_at)
             VALUES (?, 1, ?)
             ON CONFLICT(secret_reference) DO UPDATE SET
                profile_count = profile_count + 1,
                updated_at = excluded.updated_at",
        )
        .bind(reference.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn decrement_credential_ref(
        &self,
        reference: &SecretReference,
    ) -> Result<bool, AppError> {
        let mut transaction = self.pool.begin().await?;
        let count =
            sqlx::query("SELECT profile_count FROM credential_refs WHERE secret_reference = ?")
                .bind(reference.as_str())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| row.get::<i64, _>("profile_count"))
                .unwrap_or(0);
        if count <= 1 {
            sqlx::query("DELETE FROM credential_refs WHERE secret_reference = ?")
                .bind(reference.as_str())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            Ok(true)
        } else {
            sqlx::query("UPDATE credential_refs SET profile_count = profile_count - 1, updated_at = ? WHERE secret_reference = ?")
                .bind(Utc::now().to_rfc3339())
                .bind(reference.as_str())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            Ok(false)
        }
    }

    pub async fn credential_ref_count(&self, reference: &SecretReference) -> Result<i64, AppError> {
        Ok(
            sqlx::query("SELECT profile_count FROM credential_refs WHERE secret_reference = ?")
                .bind(reference.as_str())
                .fetch_optional(&self.pool)
                .await?
                .map(|row| row.get::<i64, _>("profile_count"))
                .unwrap_or(0),
        )
    }

    pub async fn record_credential_cleanup(
        &self,
        reference: &SecretReference,
        reason: &str,
        error: &str,
    ) -> Result<(), AppError> {
        sqlx::query("INSERT INTO credential_cleanup (secret_reference, reason, created_at, last_error, attempt_count) VALUES (?, ?, ?, ?, 1)")
            .bind(reference.as_str())
            .bind(reason)
            .bind(Utc::now().to_rfc3339())
            .bind(error)
            .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist the current transfer state and its replayable request.  The
    /// request is redacted before serialization: the confirmation token is a
    /// one-shot UI value and must never be written to disk.  Credentials are
    /// represented only by the profile id in transfer endpoints.
    pub async fn save_transfer(
        &self,
        job: &TransferJob,
        request: &StartTransferRequest,
    ) -> Result<(), AppError> {
        let mut durable_request = request.clone();
        durable_request.confirmation = None;
        let settings_snapshot_json = durable_request
            .settings_snapshot
            .take()
            .map(|snapshot| serde_json::to_string(&snapshot))
            .transpose()
            .map_err(|error| AppError::Unknown(format!("serialize transfer settings: {error}")))?;
        let request_json = serde_json::to_string(&durable_request)
            .map_err(|error| AppError::Unknown(format!("serialize transfer request: {error}")))?;
        let job_json = serde_json::to_string(job)
            .map_err(|error| AppError::Unknown(format!("serialize transfer job: {error}")))?;
        let source_json = serde_json::to_string(&job.source)
            .map_err(|error| AppError::Unknown(format!("serialize transfer source: {error}")))?;
        let destination_json = job
            .destination
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                AppError::Unknown(format!("serialize transfer destination: {error}"))
            })?;
        let error_json = job
            .error
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| AppError::Unknown(format!("serialize transfer error: {error}")))?;
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO transfers
                (id, profile_id, operation, state, created_at, updated_at,
                 source_json, destination_json, status, collision_policy,
                 total_bytes, transferred_bytes, total_items, completed_items,
                 failed_items, retry_count, speed_bps, eta_seconds,
                 public_error_json, settings_snapshot_json, started_at,
                 finished_at, request_json, job_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 profile_id = excluded.profile_id,
                 operation = excluded.operation,
                 state = excluded.state,
                 updated_at = excluded.updated_at,
                 source_json = excluded.source_json,
                 destination_json = excluded.destination_json,
                 status = excluded.status,
                 collision_policy = excluded.collision_policy,
                 total_bytes = excluded.total_bytes,
                 transferred_bytes = excluded.transferred_bytes,
                 total_items = excluded.total_items,
                 completed_items = excluded.completed_items,
                 failed_items = excluded.failed_items,
                 retry_count = excluded.retry_count,
                 speed_bps = excluded.speed_bps,
                 eta_seconds = excluded.eta_seconds,
                 public_error_json = excluded.public_error_json,
                 settings_snapshot_json = excluded.settings_snapshot_json,
                 started_at = excluded.started_at,
                 finished_at = excluded.finished_at,
                 request_json = excluded.request_json,
                 job_json = excluded.job_json",
        )
        .bind(job.id.to_string())
        .bind(&job.profile_id)
        .bind(job.operation.as_str())
        .bind(job.status.as_str())
        .bind(&job.created_at)
        .bind(&now)
        .bind(source_json)
        .bind(destination_json)
        .bind(job.status.as_str())
        .bind(job.collision_policy.as_str())
        .bind(optional_i64(job.total_bytes, "total bytes")?)
        .bind(i64_value(job.transferred_bytes, "transferred bytes")?)
        .bind(optional_i64(job.total_items, "total items")?)
        .bind(i64_value(job.completed_items, "completed items")?)
        .bind(i64_value(job.failed_items, "failed items")?)
        .bind(i64::from(job.retry_count))
        .bind(optional_i64(job.speed_bps, "speed")?)
        .bind(optional_i64(job.eta_seconds, "ETA")?)
        .bind(error_json)
        .bind(settings_snapshot_json)
        .bind(&job.started_at)
        .bind(&job.finished_at)
        .bind(request_json)
        .bind(job_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark jobs that could have been running when the process stopped.  A
    /// paused job is deliberately left paused so a user pause survives a
    /// normal restart; the UI can still retry an interrupted job manually.
    pub async fn mark_active_transfers_interrupted(&self) -> Result<u64, AppError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE transfers
             SET status = 'interrupted', state = 'interrupted',
                 updated_at = ?, finished_at = ?
             WHERE status IN (
                 'planning', 'waitingForUser', 'running', 'pausing',
                 'retrying', 'cancelling'
             )",
        )
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Load all durable transfer snapshots.  Older databases may have rows
    /// created before JSON snapshots were introduced; those are reconstructed
    /// from the additive columns where possible.
    pub async fn list_transfers(&self) -> Result<Vec<PersistedTransfer>, AppError> {
        let rows = sqlx::query(
            "SELECT id, profile_id, operation, state, created_at, updated_at,
                    source_json, destination_json, status, collision_policy,
                    total_bytes, transferred_bytes, total_items, completed_items,
                    failed_items, retry_count, speed_bps, eta_seconds,
                    public_error_json, settings_snapshot_json, started_at,
                    finished_at, request_json, job_json
             FROM transfers
             ORDER BY created_at DESC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut transfers = Vec::with_capacity(rows.len());
        for row in rows {
            match persisted_transfer_from_row(&row) {
                Ok(transfer) => transfers.push(transfer),
                Err(error) => {
                    // A legacy row without endpoint snapshots cannot be
                    // retried, but it must not prevent the application from
                    // opening or hide the rest of transfer history.
                    tracing::warn!(error = %error, "skipping invalid persisted transfer row");
                }
            }
        }
        Ok(transfers)
    }

    /// Remove transfer rows and their cascaded item/checkpoint records.
    pub async fn delete_transfers(&self, ids: &[Uuid]) -> Result<usize, AppError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.pool.begin().await?;
        let mut count = 0usize;
        for id in ids {
            let result = sqlx::query("DELETE FROM transfers WHERE id = ?")
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
            count += result.rows_affected() as usize;
        }
        transaction.commit().await?;
        Ok(count)
    }

    /// Enforce the default history retention contract while never touching
    /// active jobs. The day and count bounds are supplied by SettingsService
    /// in a future configurable cleanup path; defaults here protect a fresh
    /// installation even before the first settings refresh.
    pub async fn prune_transfer_history(
        &self,
        retention_days: u32,
        max_jobs: u32,
    ) -> Result<usize, AppError> {
        let retention_days = retention_days.clamp(1, 90);
        let max_jobs = max_jobs.clamp(1, 10_000);
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
        let mut removed = 0usize;
        let old = sqlx::query(
            "DELETE FROM transfers
             WHERE status IN ('completed', 'completedWithWarnings', 'failed', 'cancelled', 'interrupted')
               AND finished_at IS NOT NULL
               AND finished_at < ?",
        )
        .bind(cutoff.to_rfc3339())
        .execute(&self.pool)
        .await?;
        removed += old.rows_affected() as usize;
        let overflow = sqlx::query(
            "DELETE FROM transfers
             WHERE status IN ('completed', 'completedWithWarnings', 'failed', 'cancelled', 'interrupted')
               AND id NOT IN (
                   SELECT id FROM transfers
                   WHERE status IN ('completed', 'completedWithWarnings', 'failed', 'cancelled', 'interrupted')
                   ORDER BY COALESCE(finished_at, created_at) DESC, id DESC
                   LIMIT ?
               )",
        )
        .bind(i64::from(max_jobs))
        .execute(&self.pool)
        .await?;
        removed += overflow.rows_affected() as usize;
        Ok(removed)
    }
}

fn i64_value(value: u64, label: &str) -> Result<i64, AppError> {
    i64::try_from(value)
        .map_err(|_| AppError::Validation(format!("transfer {label} exceeds SQLite limits")))
}

fn optional_i64(value: Option<u64>, label: &str) -> Result<Option<i64>, AppError> {
    value.map(|value| i64_value(value, label)).transpose()
}

fn persisted_transfer_from_row(row: &SqliteRow) -> Result<PersistedTransfer, AppError> {
    if let Some(job_json) = row.try_get::<Option<String>, _>("job_json")? {
        let mut job: TransferJob = serde_json::from_str(&job_json)
            .map_err(|error| AppError::Unknown(format!("invalid transfer job JSON: {error}")))?;
        let mut request = row
            .try_get::<Option<String>, _>("request_json")?
            .map(|value| {
                serde_json::from_str::<StartTransferRequest>(&value).map_err(|error| {
                    AppError::Unknown(format!("invalid transfer request JSON: {error}"))
                })
            })
            .transpose()?
            .unwrap_or_else(|| request_from_job(&job));
        request.settings_snapshot = row
            .try_get::<Option<String>, _>("settings_snapshot_json")?
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| {
                    AppError::Unknown(format!("invalid transfer settings JSON: {error}"))
                })
            })
            .transpose()?;
        // The persisted status is authoritative when recovering after a
        // crash.  Keep old job_json useful even if it predates the startup
        // interruption update.
        let status = parse_transfer_status(
            row.try_get::<String, _>("status")?.as_str(),
            row.try_get::<String, _>("state")?.as_str(),
        )?;
        job.status = status;
        job.finished_at = if status.is_terminal() {
            job.finished_at.or_else(|| {
                row.try_get::<Option<String>, _>("finished_at")
                    .ok()
                    .flatten()
            })
        } else {
            None
        };
        return Ok(PersistedTransfer { job, request });
    }

    let id = Uuid::parse_str(row.try_get::<String, _>("id")?.as_str())
        .map_err(|error| AppError::Unknown(format!("invalid transfer id in database: {error}")))?;
    let operation = parse_transfer_operation(row.try_get::<String, _>("operation")?.as_str())?;
    let status = parse_transfer_status(
        row.try_get::<String, _>("status")?.as_str(),
        row.try_get::<String, _>("state")?.as_str(),
    )?;
    let source = row
        .try_get::<Option<String>, _>("source_json")?
        .ok_or_else(|| AppError::Unknown(format!("transfer {id} has no source snapshot")))
        .and_then(|value| parse_endpoint(&value, "source"))?;
    let destination = row
        .try_get::<Option<String>, _>("destination_json")?
        .map(|value| parse_endpoint(&value, "destination"))
        .transpose()?;
    let collision_policy =
        parse_collision_policy(row.try_get::<String, _>("collision_policy")?.as_str())?;
    let created_at: String = row.try_get("created_at")?;
    let error = row
        .try_get::<Option<String>, _>("public_error_json")?
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|error| AppError::Unknown(format!("invalid transfer error JSON: {error}")))
        })
        .transpose()?;
    let job = TransferJob {
        schema_version: crate::dto::transfer::DTO_SCHEMA_VERSION,
        id,
        operation,
        profile_id: row.try_get("profile_id")?,
        source,
        destination,
        status,
        collision_policy,
        total_bytes: unsigned_option(row.try_get("total_bytes")?, "total bytes")?,
        transferred_bytes: unsigned_value(row.try_get("transferred_bytes")?, "transferred bytes")?,
        total_items: unsigned_option(row.try_get("total_items")?, "total items")?,
        completed_items: unsigned_value(row.try_get("completed_items")?, "completed items")?,
        failed_items: unsigned_value(row.try_get("failed_items")?, "failed items")?,
        speed_bps: unsigned_option(row.try_get("speed_bps")?, "speed")?,
        eta_seconds: unsigned_option(row.try_get("eta_seconds")?, "ETA")?,
        retry_count: u32::try_from(row.try_get::<i64, _>("retry_count")?)
            .map_err(|_| AppError::Unknown("invalid transfer retry count".to_string()))?,
        created_at,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        error,
    };
    let mut request = request_from_job(&job);
    request.settings_snapshot = row
        .try_get::<Option<String>, _>("settings_snapshot_json")?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                AppError::Unknown(format!("invalid transfer settings JSON: {error}"))
            })
        })
        .transpose()?;
    Ok(PersistedTransfer { job, request })
}

fn request_from_job(job: &TransferJob) -> StartTransferRequest {
    StartTransferRequest {
        schema_version: crate::dto::transfer::DTO_SCHEMA_VERSION,
        operation: job.operation,
        profile_id: job.profile_id.clone(),
        source: job.source.clone(),
        destination: job.destination.clone(),
        collision_policy: job.collision_policy,
        total_bytes: job.total_bytes,
        total_items: job.total_items,
        confirmation: None,
        recursive: matches!(
            job.operation,
            TransferOperation::UploadDirectory
                | TransferOperation::DownloadPrefix
                | TransferOperation::CopyPrefix
                | TransferOperation::MovePrefix
        ) || matches!(
            (&job.operation, &job.source),
            (
                TransferOperation::DeleteObjects,
                TransferEndpoint::Remote { key, .. }
            ) if key.ends_with('/')
        ),
        metadata: None,
        preserve_root: false,
        replace_metadata: false,
        settings_snapshot: None,
    }
}

fn parse_endpoint(value: &str, field: &str) -> Result<TransferEndpoint, AppError> {
    serde_json::from_str(value)
        .map_err(|error| AppError::Unknown(format!("invalid transfer {field} JSON: {error}")))
}

fn parse_transfer_operation(value: &str) -> Result<TransferOperation, AppError> {
    match value {
        "createFolder" => Ok(TransferOperation::CreateFolder),
        "uploadFile" => Ok(TransferOperation::UploadFile),
        "uploadDirectory" => Ok(TransferOperation::UploadDirectory),
        "downloadFile" => Ok(TransferOperation::DownloadFile),
        "downloadPrefix" => Ok(TransferOperation::DownloadPrefix),
        "copyObject" => Ok(TransferOperation::CopyObject),
        "copyPrefix" => Ok(TransferOperation::CopyPrefix),
        "moveObject" => Ok(TransferOperation::MoveObject),
        "movePrefix" => Ok(TransferOperation::MovePrefix),
        "deleteObjects" => Ok(TransferOperation::DeleteObjects),
        _ => Err(AppError::Unknown(format!(
            "unknown transfer operation: {value}"
        ))),
    }
}

fn parse_transfer_status(value: &str, fallback: &str) -> Result<TransferStatus, AppError> {
    let value = if value.is_empty() { fallback } else { value };
    match value {
        "queued" => Ok(TransferStatus::Queued),
        "planning" => Ok(TransferStatus::Planning),
        "waitingForUser" => Ok(TransferStatus::WaitingForUser),
        "running" => Ok(TransferStatus::Running),
        "pausing" => Ok(TransferStatus::Pausing),
        "paused" => Ok(TransferStatus::Paused),
        "retrying" => Ok(TransferStatus::Retrying),
        "cancelling" | "canceling" => Ok(TransferStatus::Cancelling),
        "completed" => Ok(TransferStatus::Completed),
        "completedWithWarnings" => Ok(TransferStatus::CompletedWithWarnings),
        "failed" => Ok(TransferStatus::Failed),
        "cancelled" | "canceled" => Ok(TransferStatus::Cancelled),
        "interrupted" => Ok(TransferStatus::Interrupted),
        _ => Err(AppError::Unknown(format!(
            "unknown transfer status: {value}"
        ))),
    }
}

fn parse_collision_policy(value: &str) -> Result<CollisionPolicy, AppError> {
    match value {
        "ask" => Ok(CollisionPolicy::Ask),
        "replace" => Ok(CollisionPolicy::Replace),
        "skip" => Ok(CollisionPolicy::Skip),
        "fail" => Ok(CollisionPolicy::Fail),
        "rename" => Ok(CollisionPolicy::Rename),
        _ => Err(AppError::Unknown(format!(
            "unknown transfer collision policy: {value}"
        ))),
    }
}

fn unsigned_value(value: i64, label: &str) -> Result<u64, AppError> {
    u64::try_from(value)
        .map_err(|_| AppError::Unknown(format!("invalid transfer {label} in database")))
}

fn unsigned_option(value: Option<i64>, label: &str) -> Result<Option<u64>, AppError> {
    value.map(|value| unsigned_value(value, label)).transpose()
}

fn validate_schema_version(version: u16) -> Result<(), AppError> {
    if version != 1 {
        return Err(AppError::Validation(
            "unsupported Explorer state schema version".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_profile_exists(database: &Database, profile_id: &str) -> Result<(), AppError> {
    if profile_id.trim().is_empty() {
        return Err(AppError::Validation("profile ID is required".to_string()));
    }
    let exists = sqlx::query("SELECT 1 FROM connection_profiles WHERE id = ?")
        .bind(profile_id)
        .fetch_optional(&database.pool)
        .await?
        .is_some();
    if !exists {
        return Err(AppError::ProfileNotFound(profile_id.to_string()));
    }
    Ok(())
}

async fn validate_location(
    database: &Database,
    profile_id: &str,
    bucket: &str,
    prefix: &str,
) -> Result<(), AppError> {
    let row = sqlx::query("SELECT root_prefix FROM connection_profiles WHERE id = ?")
        .bind(profile_id)
        .fetch_optional(&database.pool)
        .await?;
    let Some(row) = row else {
        return Err(AppError::ProfileNotFound(profile_id.to_string()));
    };
    let root_prefix: Option<String> = row.try_get("root_prefix")?;
    let bucket_len = bucket.len();
    if !(3..=255).contains(&bucket_len)
        || bucket.contains(['/', '\\'])
        || bucket.chars().any(char::is_control)
    {
        return Err(AppError::Validation("bucket name is invalid".to_string()));
    }
    if prefix.contains('\0')
        || prefix.contains('\\')
        || prefix.split('/').any(|segment| segment == "..")
    {
        return Err(AppError::Validation(
            "Explorer prefix is invalid".to_string(),
        ));
    }
    if root_prefix
        .as_deref()
        .is_some_and(|root| !prefix.starts_with(root))
    {
        return Err(AppError::RootPrefixViolation);
    }
    Ok(())
}

fn validate_bookmark_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "bookmark name must be 1–256 printable characters".to_string(),
        ));
    }
    Ok(name.to_string())
}

fn row_to_bookmark(row: SqliteRow) -> Result<Bookmark, AppError> {
    Ok(Bookmark {
        schema_version: 1,
        id: row.try_get("id")?,
        profile_id: row.try_get("profile_id")?,
        bucket: row.try_get("bucket")?,
        prefix: row.try_get("prefix")?,
        name: row.try_get("name")?,
        sort_order: row.try_get("sort_order")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_recent_location(row: SqliteRow) -> Result<RecentLocation, AppError> {
    Ok(RecentLocation {
        schema_version: 1,
        id: row.try_get("id")?,
        profile_id: row.try_get("profile_id")?,
        bucket: row.try_get("bucket")?,
        prefix: row.try_get("prefix")?,
        opened_at: row.try_get("opened_at")?,
    })
}

fn row_to_profile(row: SqliteRow) -> Result<ConnectionProfile, AppError> {
    let id = Uuid::parse_str(row.try_get::<String, _>("id")?.as_str())
        .map_err(|error| AppError::Unknown(format!("invalid profile ID in database: {error}")))?;
    let created_at = parse_timestamp(row.try_get::<String, _>("created_at")?.as_str())?;
    let updated_at = parse_timestamp(row.try_get::<String, _>("updated_at")?.as_str())?;
    Ok(ConnectionProfile {
        id,
        name: row.try_get("name")?,
        provider: ProviderType::parse_known(row.try_get::<String, _>("provider")?.as_str())
            .ok_or_else(|| AppError::Unknown("unknown provider in profile database".to_string()))?,
        endpoint: row.try_get("endpoint")?,
        region: row.try_get("region")?,
        credential_mode: match row.try_get::<String, _>("credential_mode")?.as_str() {
            "temporarySession" => CredentialMode::TemporarySession,
            "static" => CredentialMode::Static,
            _ => {
                return Err(AppError::Unknown(
                    "unknown credential mode in profile database".to_string(),
                ))
            }
        },
        access_key_id: row.try_get("access_key_id")?,
        secret_reference: row
            .try_get::<Option<String>, _>("secret_reference")?
            .map(SecretReference),
        session_reference: row
            .try_get::<Option<String>, _>("session_reference")?
            .map(SecretReference),
        default_bucket: row.try_get("default_bucket")?,
        root_prefix: row.try_get("root_prefix")?,
        addressing_style: match row.try_get::<String, _>("addressing_style")?.as_str() {
            "path" => AddressingStyle::Path,
            "virtualHosted" => AddressingStyle::VirtualHosted,
            _ => {
                return Err(AppError::Unknown(
                    "unknown addressing style in profile database".to_string(),
                ))
            }
        },
        allow_insecure_http: row.try_get::<i64, _>("allow_insecure_http")? != 0,
        favorite: row.try_get::<i64, _>("favorite")? != 0,
        favorite_order: row.try_get("favorite_order")?,
        revision: row.try_get("revision")?,
        created_at,
        updated_at,
    })
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| AppError::Unknown(format!("invalid profile timestamp: {error}")))
}

fn redacted_endpoint_display(value: Option<String>) -> Option<String> {
    let endpoint = value?;
    let parsed = url::Url::parse(&endpoint).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?;
    let authority = parsed
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or_else(|| host.to_string());
    Some(format!("{}://{}", parsed.scheme(), authority))
}
