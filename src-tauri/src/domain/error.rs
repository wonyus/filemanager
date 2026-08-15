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
    LocalPathTooLong,
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
    #[error("local path is too long")]
    LocalPathTooLong,
    #[error("local disk is full")]
    LocalDiskFull,
    #[error("local file changed during transfer")]
    LocalFileChanged,
    #[error("signed update verification failed: {0}")]
    UpdateVerificationFailed(String),
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

/// Recognize only provider messages that clearly describe an expired or
/// invalid session/security token. This intentionally does not classify an
/// invalid access-key ID or a generic expired request/presigned URL as an
/// expired credential.
pub fn is_credential_expired_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("credential expired") {
        return true;
    }
    let expired_token_code = lower.contains("expiredtoken") || lower.contains("expired_token");
    let expired_session = (lower.contains("security token")
        || lower.contains("session token")
        || lower.contains("credential"))
        && lower.contains("expired");
    let invalid_token_code = lower == "invalidtoken" || lower.starts_with("invalidtoken:");
    let invalid_session = (invalid_token_code
        || lower.contains("invalidtoken")
        || lower.contains("invalid_token")
        || lower.contains("invalid token")
        || lower.contains("invalidsecuritytoken")
        || lower.contains("invalidsessiontoken")
        || lower.contains("sessiontokeninvalid")
        || lower.contains("tokeninvalid"))
        && (lower.contains("security")
            || lower.contains("session")
            || lower.contains("credential"));
    expired_token_code || expired_session || invalid_session
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
        let reason = match &error {
            // These values are validated or sanitized at the command
            // boundary where possible, then normalized again here. Keeping
            // them in structured details makes failures actionable without
            // exposing raw SDK errors.
            AppError::CredentialMissing(reason)
            | AppError::UnsupportedProviderFeature(reason)
            | AppError::TransferStateConflict(reason) => safe_reason(reason),
            AppError::Provider(reason) => safe_provider_reason(reason),
            _ => None,
        };
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
            AppError::Provider(message) if is_credential_expired_message(message) => (
                AppErrorCode::CredentialExpired,
                false,
                "The credential has expired; update the profile and try again.".to_string(),
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
            AppError::LocalPathTooLong => (
                AppErrorCode::LocalPathTooLong,
                false,
                "The local destination path is too long.".to_string(),
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
            AppError::UpdateVerificationFailed(_) => (
                AppErrorCode::UpdateVerificationFailed,
                false,
                "The signed update could not be verified and was not installed.".to_string(),
            ),
            AppError::Unknown(_) => (
                AppErrorCode::Unknown,
                false,
                "The operation could not be completed.".to_string(),
            ),
        };
        let mut details = BTreeMap::new();
        if let Some(reason) = reason.filter(|value| !value.trim().is_empty()) {
            details.insert("reason".to_string(), Value::String(reason));
        }
        Self {
            schema_version: 1,
            code,
            message,
            retryable,
            request_id: None,
            provider_request_id: None,
            correlation_id: None,
            field_errors: BTreeMap::new(),
            details,
        }
    }
}

fn safe_reason(reason: &str) -> Option<String> {
    let normalized = reason.replace(['\r', '\n'], " ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut value = trimmed.chars().take(240).collect::<String>();
    if trimmed.chars().count() > 240 {
        value.push('…');
    }
    Some(value)
}

fn safe_provider_reason(reason: &str) -> Option<String> {
    let normalized = reason.replace(['\r', '\n'], " ");
    let lower = normalized.to_ascii_lowercase();
    if is_credential_expired_message(&normalized) {
        return Some("credential expired".to_string());
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
        return Some("provider request failed (sensitive details redacted)".to_string());
    }
    safe_reason(&normalized)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{is_credential_expired_message, AppError, AppErrorCode, PublicError};

    #[test]
    fn recognizes_expired_or_invalid_session_tokens_only() {
        assert!(is_credential_expired_message(
            "ExpiredToken: The security token included in the request is expired"
        ));
        assert!(is_credential_expired_message("InvalidSessionToken"));
        assert!(is_credential_expired_message("credential expired"));
        assert!(!is_credential_expired_message("InvalidAccessKeyId"));
        assert!(!is_credential_expired_message("Request has expired"));
        assert!(!is_credential_expired_message("AccessDenied"));
    }

    #[test]
    fn maps_provider_expiry_to_public_credential_expired_code() {
        let error = PublicError::from(AppError::Provider(
            "ExpiredToken: security token is expired".to_string(),
        ));
        assert!(matches!(error.code, AppErrorCode::CredentialExpired));
        assert!(!error.retryable);
    }

    #[test]
    fn preserves_safe_provider_reason_in_details() {
        let error = PublicError::from(AppError::Provider("AccessDenied".to_string()));
        assert_eq!(
            error.details.get("reason"),
            Some(&Value::String("AccessDenied".to_string()))
        );
        assert_eq!(
            error.message,
            "The provider operation could not be completed."
        );
    }

    #[test]
    fn redacts_sensitive_provider_reason_in_details() {
        let error = PublicError::from(AppError::Provider(
            "request failed: https://access:secret@example.test?X-Amz-Signature=abc".to_string(),
        ));
        assert_eq!(
            error.details.get("reason"),
            Some(&Value::String(
                "provider request failed (sensitive details redacted)".to_string()
            ))
        );
    }
}
