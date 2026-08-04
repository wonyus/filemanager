use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub schema_version: i32,
    pub transfer_concurrency: u32,
    pub part_concurrency: u32,
    pub retry_limit: u32,
    pub preview_cache_quota_bytes: u64,
}

impl Default for SettingsSnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            transfer_concurrency: 4,
            part_concurrency: 4,
            retry_limit: 3,
            preview_cache_quota_bytes: 536_870_912,
        }
    }
}
