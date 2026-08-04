use std::time::Duration;

use crate::domain::error::{is_credential_expired_message, AppError};

/// Coarse error classes used by the scheduler.  The provider adapters can map
/// their richer SDK errors into these classes without leaking provider details
/// to the renderer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RetryClass {
    Retryable,
    NonRetryable,
    Unknown,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RetryPolicy {
    /// Maximum number of retries after the first attempt.
    pub retry_limit: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            retry_limit: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    pub fn validate(self) -> Result<Self, String> {
        if self.base_delay.is_zero() {
            return Err("retry base delay must be greater than zero".to_string());
        }
        if self.max_delay < self.base_delay {
            return Err("retry maximum delay must be >= base delay".to_string());
        }
        if self.retry_limit > 10 {
            return Err("retry limit must be between 0 and 10".to_string());
        }
        Ok(self)
    }

    /// Exponential backoff before jitter.  The caller may apply full jitter
    /// when scheduling a real request; keeping this calculation deterministic
    /// makes it testable and keeps retry decisions reproducible in logs.
    pub fn delay_for_retry(&self, retry_number: u32) -> Duration {
        let exponent = retry_number.min(31);
        let multiplier = 1_u128 << exponent;
        let millis = self.base_delay.as_millis().saturating_mul(multiplier);
        let capped = millis.min(self.max_delay.as_millis());
        Duration::from_millis(capped.min(u128::from(u64::MAX)) as u64)
    }

    pub fn should_retry(&self, retry_number: u32, class: RetryClass) -> bool {
        class == RetryClass::Retryable && retry_number < self.retry_limit
    }
}

/// Map internal errors to a conservative retry class.  Authentication,
/// validation, authorization, object conflicts and local disk errors must not
/// be retried automatically.  Network/provider failures are retryable because
/// they commonly represent a transient outage or throttling response.
pub fn classify_error(error: &AppError) -> RetryClass {
    match error {
        AppError::Validation(_)
        | AppError::ProfileRevisionConflict
        | AppError::CredentialMissing(_)
        | AppError::ProfileNotFound(_)
        | AppError::CredentialStore(_)
        | AppError::Io(_)
        | AppError::Database(_)
        | AppError::DatabaseMigration(_)
        | AppError::CredentialExpired
        | AppError::UnsupportedProviderFeature(_)
        | AppError::InvalidEndpoint
        | AppError::InsecureEndpointBlocked
        | AppError::RootPrefixViolation
        | AppError::TransferStateConflict(_)
        | AppError::ObjectNotFound
        | AppError::BucketAccessDenied
        | AppError::LocalPermissionDenied
        | AppError::LocalDiskFull
        | AppError::LocalFileChanged => RetryClass::NonRetryable,
        AppError::NetworkUnavailable | AppError::RequestTimedOut => RetryClass::Retryable,
        AppError::Provider(message) => classify_provider_message(message),
        AppError::Unknown(message) => classify_provider_message(message),
    }
}

fn classify_provider_message(message: &str) -> RetryClass {
    let lower = message.to_ascii_lowercase();
    if is_credential_expired_message(message) {
        return RetryClass::NonRetryable;
    }
    if [
        "accessdenied",
        "access denied",
        "invalidaccesskeyid",
        "signaturedoesnotmatch",
        "nosuchbucket",
        "nosuchkey",
        "not found",
        "invalid bucket",
        "destination exists",
        "object lock",
        "retention",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return RetryClass::NonRetryable;
    }
    if [
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "temporary dns",
        "dns",
        "429",
        "too many requests",
        "rate limit",
        "throttl",
        " 500",
        " 502",
        " 503",
        " 504",
        "service unavailable",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return RetryClass::Retryable;
    }
    RetryClass::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_delay_is_capped() {
        let policy = RetryPolicy {
            retry_limit: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(3),
        };
        assert_eq!(policy.delay_for_retry(0), Duration::from_millis(500));
        assert_eq!(policy.delay_for_retry(2), Duration::from_secs(2));
        assert_eq!(policy.delay_for_retry(3), Duration::from_secs(3));
        assert!(policy.should_retry(4, RetryClass::Retryable));
        assert!(!policy.should_retry(5, RetryClass::Retryable));
    }

    #[test]
    fn provider_error_classification_is_conservative() {
        assert_eq!(
            classify_error(&AppError::Provider(
                "HTTP 503 Service Unavailable".to_string()
            )),
            RetryClass::Retryable
        );
        assert_eq!(
            classify_error(&AppError::Provider("AccessDenied".to_string())),
            RetryClass::NonRetryable
        );
        assert_eq!(
            classify_error(&AppError::Provider(
                "ExpiredToken: The security token included in the request is expired".to_string()
            )),
            RetryClass::NonRetryable
        );
        assert_eq!(
            classify_error(&AppError::Validation("bad request".to_string())),
            RetryClass::NonRetryable
        );
    }
}
