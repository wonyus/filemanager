use serde::Serialize;

use crate::domain::provider::ProviderType;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub provider: ProviderType,
    pub region: String,
    pub default_bucket: Option<String>,
    pub favorite: bool,
    pub last_connected_at: Option<String>,
}
