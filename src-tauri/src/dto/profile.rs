use std::fmt;

use serde::{de::Deserializer, Deserialize, Serialize};

use crate::domain::provider::{AddressingStyle, CredentialMode, ProviderType};

pub const PROFILE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialState {
    Configured,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Unknown,
    Connected,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub provider: ProviderType,
    pub endpoint_display: Option<String>,
    pub region: String,
    pub default_bucket: Option<String>,
    pub root_prefix: String,
    pub favorite: bool,
    pub last_connected_at: Option<String>,
    pub credential_state: CredentialState,
    pub connection_state: ConnectionState,
}

/// Secret input deliberately has no `Serialize` implementation.  A missing
/// field means keep the current value, a string replaces it, and JSON null
/// clears it.  This keeps secret material one-way across the IPC boundary.
#[derive(Clone, Default)]
pub enum SecretInput {
    #[default]
    Unchanged,
    Replace(String),
    Clear,
}

impl<'de> Deserialize<'de> for SecretInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value {
            Some(value) => Self::Replace(value),
            None => Self::Clear,
        })
    }
}

impl fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unchanged => formatter.write_str("Unchanged"),
            Self::Replace(_) => formatter.write_str("Replace(<redacted>)"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDraft {
    pub schema_version: u16,
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub provider: ProviderType,
    pub account_id: Option<String>,
    pub endpoint: Option<String>,
    pub region: String,
    pub credential_mode: CredentialMode,
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: SecretInput,
    #[serde(default)]
    pub session_token: SecretInput,
    pub default_bucket: Option<String>,
    pub root_prefix: Option<String>,
    #[serde(alias = "addressingMode")]
    pub addressing_style: Option<AddressingStyle>,
    #[serde(alias = "allowPlainHttp")]
    pub allow_insecure_http: bool,
    pub favorite: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDetail {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub provider: ProviderType,
    pub endpoint: Option<String>,
    pub region: String,
    pub credential_mode: CredentialMode,
    pub access_key_preview: Option<String>,
    pub has_secret_access_key: bool,
    pub has_session_token: bool,
    pub default_bucket: Option<String>,
    pub root_prefix: Option<String>,
    pub addressing_style: AddressingStyle,
    pub allow_insecure_http: bool,
    pub favorite: bool,
    pub favorite_order: i64,
    pub revision: i64,
}

impl SecretInput {
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    pub fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    pub fn replacement(&self) -> Option<&str> {
        match self {
            Self::Replace(value) => Some(value.as_str()),
            Self::Unchanged | Self::Clear => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SecretInput;

    #[test]
    fn secret_input_distinguishes_replace_and_clear() {
        let replace: SecretInput = serde_json::from_str("\"new-secret\"").unwrap();
        assert!(matches!(replace, SecretInput::Replace(value) if value == "new-secret"));
        let clear: SecretInput = serde_json::from_str("null").unwrap();
        assert!(matches!(clear, SecretInput::Clear));
        assert!(SecretInput::default().is_unchanged());
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub schema_version: u16,
    pub success: bool,
    pub latency_ms: u64,
    pub bucket_access: bool,
    pub message: String,
    pub provider_request_id: Option<String>,
    pub can_list_buckets: Option<bool>,
    pub can_head_bucket: Option<bool>,
    pub supports_multipart_upload: Option<bool>,
    pub supports_presigned_get: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketSummary {
    pub schema_version: u16,
    pub name: String,
    pub creation_date: Option<String>,
}
