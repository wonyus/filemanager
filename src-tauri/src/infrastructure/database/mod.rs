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
        profile::{ConnectionState, CredentialState, ProfileSummary},
        settings::SettingsSnapshot,
    },
};

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
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
