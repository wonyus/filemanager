use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AppErrorCode {
    ValidationFailed,
    ProfileNotFound,
    ProfileRevisionConflict,
    InvalidEndpoint,
    InsecureEndpointBlocked,
    CredentialMissing,
    CredentialExpired,
    CredentialStoreUnavailable,
    CredentialRejected,
    BucketNotFound,
    BucketAccessDenied,
    ObjectNotFound,
    DestinationExists,
    RootPrefixViolation,
    UnsupportedProviderFeature,
    NetworkUnavailable,
    RequestTimedOut,
    TlsError,
    RateLimited,
    ProviderUnavailable,
    LocalPathInvalid,
    LocalPermissionDenied,
    LocalDiskFull,
    LocalFileChanged,
    TransferCancelled,
    TransferStateConflict,
    DatabaseError,
    UpdateVerificationFailed,
    Unknown,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("profile revision changed; reload before saving")]
    ProfileRevisionConflict,
    #[error("credential missing: {0}")]
    CredentialMissing(String),
    #[error("credential expired")]
    CredentialExpired,
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("provider operation failed: {0}")]
    Provider(String),
    #[error("unsupported provider feature: {0}")]
    UnsupportedProviderFeature(String),
    #[error("invalid endpoint")]
    InvalidEndpoint,
    #[error("insecure endpoint blocked")]
    InsecureEndpointBlocked,
    #[error("root prefix violation")]
    RootPrefixViolation,
    #[error("transfer state conflict: {0}")]
    TransferStateConflict(String),
    #[error("object not found")]
    ObjectNotFound,
    #[error("bucket access denied")]
    BucketAccessDenied,
    #[error("network unavailable")]
    NetworkUnavailable,
    #[error("request timed out")]
    RequestTimedOut,
    #[error("local permission denied")]
    LocalPermissionDenied,
    #[error("local disk is full")]
    LocalDiskFull,
    #[error("local file changed during transfer")]
    LocalFileChanged,
    #[error("database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    DatabaseMigration(#[from] sqlx::migrate::MigrateError),
    #[error("credential store unavailable: {0}")]
    CredentialStore(String),
    #[error("local I/O operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Unknown(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicError {
    pub schema_version: u16,
    pub code: AppErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub correlation_id: Option<String>,
    pub field_errors: BTreeMap<String, String>,
    pub details: BTreeMap<String, Value>,
}

impl From<AppError> for PublicError {
    fn from(error: AppError) -> Self {
        let (code, retryable, message) = match &error {
            AppError::Validation(message) => {
                (AppErrorCode::ValidationFailed, false, message.clone())
            }
            AppError::ProfileRevisionConflict => (
                AppErrorCode::ProfileRevisionConflict,
                false,
                "The profile changed elsewhere; reload it before saving.".to_string(),
            ),
            AppError::CredentialMissing(_) => (
                AppErrorCode::CredentialMissing,
                false,
                "A required credential is missing.".to_string(),
            ),
            AppError::CredentialExpired => (
                AppErrorCode::CredentialExpired,
                false,
                "The credential has expired; update the profile and try again.".to_string(),
            ),
            AppError::ProfileNotFound(_) => (
                AppErrorCode::ProfileNotFound,
                false,
                "The requested profile was not found.".to_string(),
            ),
            AppError::Provider(_) => (
                AppErrorCode::ProviderUnavailable,
                true,
                "The provider operation could not be completed.".to_string(),
            ),
            AppError::UnsupportedProviderFeature(_) => (
                AppErrorCode::UnsupportedProviderFeature,
                false,
                "This provider does not support the requested operation.".to_string(),
            ),
            AppError::InvalidEndpoint => (
                AppErrorCode::InvalidEndpoint,
                false,
                "The endpoint is invalid.".to_string(),
            ),
            AppError::InsecureEndpointBlocked => (
                AppErrorCode::InsecureEndpointBlocked,
                false,
                "HTTP endpoints require explicit opt-in.".to_string(),
            ),
            AppError::RootPrefixViolation => (
                AppErrorCode::RootPrefixViolation,
                false,
                "The requested location is outside the profile root.".to_string(),
            ),
            AppError::TransferStateConflict(_) => (
                AppErrorCode::TransferStateConflict,
                false,
                "The transfer is not in a state that allows this action.".to_string(),
            ),
            AppError::ObjectNotFound => (
                AppErrorCode::ObjectNotFound,
                false,
                "The object was not found.".to_string(),
            ),
            AppError::BucketAccessDenied => (
                AppErrorCode::BucketAccessDenied,
                false,
                "Access to the bucket was denied.".to_string(),
            ),
            AppError::NetworkUnavailable => (
                AppErrorCode::NetworkUnavailable,
                true,
                "The network is unavailable.".to_string(),
            ),
            AppError::RequestTimedOut => (
                AppErrorCode::RequestTimedOut,
                true,
                "The provider request timed out.".to_string(),
            ),
            AppError::Database(_) => (
                AppErrorCode::DatabaseError,
                true,
                "The local database operation failed.".to_string(),
            ),
            AppError::DatabaseMigration(_) => (
                AppErrorCode::DatabaseError,
                true,
                "The local database could not be migrated.".to_string(),
            ),
            AppError::CredentialStore(_) => (
                AppErrorCode::CredentialStoreUnavailable,
                true,
                "Secure credential storage is unavailable.".to_string(),
            ),
            AppError::Io(_) => (
                AppErrorCode::LocalPathInvalid,
                false,
                "The local file operation could not be completed.".to_string(),
            ),
            AppError::LocalPermissionDenied => (
                AppErrorCode::LocalPermissionDenied,
                false,
                "Permission was denied for the local path.".to_string(),
            ),
            AppError::LocalDiskFull => (
                AppErrorCode::LocalDiskFull,
                false,
                "There is not enough local disk space.".to_string(),
            ),
            AppError::LocalFileChanged => (
                AppErrorCode::LocalFileChanged,
                false,
                "The local file changed during upload; the object was not trusted as a stable snapshot.".to_string(),
            ),
            AppError::Unknown(_) => (
                AppErrorCode::Unknown,
                false,
                "The operation could not be completed.".to_string(),
            ),
        };
        Self {
            schema_version: 1,
            code,
            message,
            retryable,
            request_id: None,
            provider_request_id: None,
            correlation_id: None,
            field_errors: BTreeMap::new(),
            details: BTreeMap::new(),
        }
    }
}
