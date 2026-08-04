use serde::{Deserialize, Serialize};

use super::explorer::ExplorerLocation;

const DTO_SCHEMA_VERSION: u16 = 1;

fn default_schema_version() -> u16 {
    DTO_SCHEMA_VERSION
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: i64,
    pub profile_id: String,
    pub bucket: String,
    pub prefix: String,
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBookmarkRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub prefix: String,
    pub name: String,
    #[serde(default)]
    pub sort_order: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListBookmarksRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub profile_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveBookmarkRequest {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentLocation {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: i64,
    pub profile_id: String,
    pub bucket: String,
    pub prefix: String,
    pub opened_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRecentLocationRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub location: ExplorerLocation,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRecentLocationsRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub profile_id: String,
    #[serde(default)]
    pub limit: Option<u16>,
}
