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
    pub code: AppErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    pub details: BTreeMap<String, Value>,
}

impl From<AppError> for PublicError {
    fn from(error: AppError) -> Self {
        let message = error.to_string();
        let (code, retryable) = match &error {
            AppError::Validation(_) => (AppErrorCode::ValidationFailed, false),
            AppError::Database(_) => (AppErrorCode::DatabaseError, true),
            AppError::DatabaseMigration(_) => (AppErrorCode::DatabaseError, true),
            AppError::CredentialStore(_) => (AppErrorCode::CredentialStoreUnavailable, true),
            AppError::Io(_) => (AppErrorCode::LocalPathInvalid, false),
            AppError::Unknown(_) => (AppErrorCode::Unknown, false),
        };
        Self {
            code,
            message,
            retryable,
            request_id: None,
            details: BTreeMap::new(),
        }
    }
}
