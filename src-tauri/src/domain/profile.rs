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
}

impl ConnectionProfile {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.name.trim().is_empty() || self.name.chars().count() > 128 {
            return Err(AppError::Validation(
                "Profile name must be 1–128 characters".to_string(),
            ));
        }
        if self.region.is_empty() || self.region.len() > 64 {
            return Err(AppError::Validation(
                "Region must be 1–64 characters".to_string(),
            ));
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
            secret_reference: None,
            session_reference: None,
            default_bucket: None,
            root_prefix: None,
            addressing_style: AddressingStyle::Path,
            allow_insecure_http,
            favorite: false,
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
