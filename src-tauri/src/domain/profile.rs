use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    error::AppError,
    provider::{AddressingStyle, CredentialMode, ProviderType},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretReference(pub String);

impl SecretReference {
    pub fn new(profile_id: Uuid, name: &str) -> Self {
        Self(format!("s3fm/profile/{profile_id}/{name}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    pub provider: ProviderType,
    pub endpoint: Option<String>,
    pub region: String,
    pub credential_mode: CredentialMode,
    pub access_key_id: Option<String>,
    pub secret_reference: Option<SecretReference>,
    pub session_reference: Option<SecretReference>,
    pub default_bucket: Option<String>,
    pub root_prefix: Option<String>,
    pub addressing_style: AddressingStyle,
    pub allow_insecure_http: bool,
    pub favorite: bool,
    pub favorite_order: i64,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ConnectionProfile {
    pub fn validate(&self) -> Result<(), AppError> {
        self.validate_configuration()?;
        if self.access_key_id.as_deref().unwrap_or("").is_empty() {
            return Err(AppError::Validation(
                "Access key ID is required for credentials".to_string(),
            ));
        }
        if self.secret_reference.is_none() {
            return Err(AppError::Validation(
                "A stored secret is required for credentials".to_string(),
            ));
        }
        if self.credential_mode == CredentialMode::TemporarySession
            && self.session_reference.is_none()
        {
            return Err(AppError::Validation(
                "Session token is required for temporary credentials".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate the portable provider/profile configuration without requiring
    /// credentials. Imported profiles use this before local credential entry.
    pub fn validate_configuration(&self) -> Result<(), AppError> {
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            return Err(AppError::Validation(
                "Profile name must be 1–80 characters".to_string(),
            ));
        }
        if self.region.is_empty() || self.region.len() > 64 {
            return Err(AppError::Validation(
                "Region must be 1–64 characters".to_string(),
            ));
        }
        if let Some(access_key_id) = &self.access_key_id {
            if access_key_id.is_empty()
                || access_key_id.chars().count() > 256
                || access_key_id.chars().any(char::is_control)
            {
                return Err(AppError::Validation(
                    "Access key ID must be 1–256 characters".to_string(),
                ));
            }
        }
        if !self
            .region
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(AppError::Validation(
                "Region must use ASCII letters, numbers, and hyphens".to_string(),
            ));
        }
        if let Some(root_prefix) = &self.root_prefix {
            if root_prefix.len() > 1_024
                || root_prefix.starts_with('/')
                || root_prefix.contains("..")
                || root_prefix.contains('\\')
                || root_prefix.chars().any(char::is_control)
            {
                return Err(AppError::Validation("Root prefix is invalid".to_string()));
            }
            if !root_prefix.is_empty() && !root_prefix.ends_with('/') {
                return Err(AppError::Validation(
                    "Root prefix must end with `/`".to_string(),
                ));
            }
        }
        if let Some(bucket) = &self.default_bucket {
            if !(3..=255).contains(&bucket.chars().count())
                || bucket.starts_with('/')
                || bucket.ends_with('/')
                || bucket.chars().any(char::is_control)
            {
                return Err(AppError::Validation(
                    "Default bucket must be 3–255 characters".to_string(),
                ));
            }
        }
        if let Some(endpoint) = &self.endpoint {
            let parsed = url::Url::parse(endpoint)
                .map_err(|_| AppError::Validation("Endpoint must be a valid URL".to_string()))?;
            if parsed.username() != "" || parsed.password().is_some() {
                return Err(AppError::Validation(
                    "Endpoint URLs cannot contain credentials".to_string(),
                ));
            }
            if parsed.scheme() == "http" && !self.allow_insecure_http {
                return Err(AppError::Validation(
                    "HTTP endpoints require explicit opt-in".to_string(),
                ));
            }
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(AppError::Validation(
                    "Endpoint scheme must be http or https".to_string(),
                ));
            }
            if parsed.host_str().is_none() {
                return Err(AppError::Validation(
                    "Endpoint must include a host".to_string(),
                ));
            }
            if parsed.query().is_some() || parsed.fragment().is_some() {
                return Err(AppError::Validation(
                    "Endpoint cannot contain query or fragment".to_string(),
                ));
            }
        }
        match self.provider {
            ProviderType::AwsS3 if self.endpoint.is_some() => {
                return Err(AppError::Validation(
                    "AWS S3 uses its SDK-managed endpoint".to_string(),
                ))
            }
            ProviderType::CloudflareR2
            | ProviderType::Minio
            | ProviderType::Wasabi
            | ProviderType::CustomS3
                if self.endpoint.is_none() =>
            {
                return Err(AppError::Validation(
                    "The selected provider requires an endpoint".to_string(),
                ))
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(endpoint: Option<&str>, allow_insecure_http: bool) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "Test profile".to_string(),
            provider: ProviderType::CustomS3,
            endpoint: endpoint.map(str::to_string),
            region: "us-east-1".to_string(),
            credential_mode: CredentialMode::Static,
            access_key_id: Some("access-key".to_string()),
            secret_reference: Some(SecretReference::new(Uuid::new_v4(), "test-secret")),
            session_reference: None,
            default_bucket: None,
            root_prefix: None,
            addressing_style: AddressingStyle::Path,
            allow_insecure_http,
            favorite: false,
            favorite_order: 0,
            revision: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn rejects_credentials_embedded_in_endpoint() {
        let result = profile(Some("https://access:secret@example.test"), false).validate();
        assert!(result.is_err());
    }

    #[test]
    fn requires_explicit_opt_in_for_http() {
        let result = profile(Some("http://localhost:9000"), false).validate();
        assert!(result.is_err());
        assert!(profile(Some("http://localhost:9000"), true)
            .validate()
            .is_ok());
    }
}
