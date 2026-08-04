use serde::{Deserialize, Serialize};

use super::{
    profile::{ConnectionState, CredentialState, ProfileSummary},
    settings::SettingsSnapshot,
};

pub const DTO_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExportRequest {
    pub schema_version: u16,
    pub destination_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLogEntry {
    pub timestamp: String,
    pub level: String,
    pub component: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub schema_version: u16,
    pub app_version: String,
    pub platform: String,
    pub architecture: String,
    pub database_schema_version: u32,
    pub providers: Vec<String>,
    pub settings: SettingsSnapshot,
    pub profiles: Vec<RedactedProfile>,
    pub recent_logs: Vec<DiagnosticLogEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedProfile {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub region: String,
    pub credential_state: String,
    pub connection_state: String,
}

impl From<ProfileSummary> for RedactedProfile {
    fn from(profile: ProfileSummary) -> Self {
        Self {
            id: profile.id.to_string(),
            name: profile.name,
            provider: profile.provider.as_str().to_string(),
            region: profile.region,
            credential_state: credential_state_name(profile.credential_state).to_string(),
            connection_state: connection_state_name(profile.connection_state).to_string(),
        }
    }
}

fn credential_state_name(value: CredentialState) -> &'static str {
    match value {
        CredentialState::Configured => "configured",
        CredentialState::Missing => "missing",
        CredentialState::Unavailable => "unavailable",
    }
}

fn connection_state_name(value: ConnectionState) -> &'static str {
    match value {
        ConnectionState::Unknown => "unknown",
        ConnectionState::Connected => "connected",
        ConnectionState::Failed => "failed",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExportResult {
    pub schema_version: u16,
    pub path: String,
    pub bytes_written: u64,
    pub redacted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogDirectoryResult {
    pub schema_version: u16,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub schema_version: u16,
    pub channel: String,
    pub available: bool,
    pub message: String,
    /// Version announced by the verified manifest, when one is available.
    pub version: Option<String>,
    /// Release notes are supplied by the signed manifest and are never used
    /// as executable content.
    pub notes: Option<String>,
    pub published_at: Option<String>,
    /// Installation is always an explicit, user-confirmed action.  The
    /// backend never installs as part of a check.
    pub install_requires_confirmation: bool,
    pub can_install: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallUpdateRequest {
    pub schema_version: u16,
    pub expected_version: String,
    /// Exact confirmation phrase shown by the UI, e.g. `INSTALL UPDATE 1.2.3`.
    pub confirmation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallUpdateResult {
    pub schema_version: u16,
    pub installed: bool,
    pub version: String,
    pub message: String,
}
