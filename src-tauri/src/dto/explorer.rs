use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerLocation {
    pub profile_id: String,
    pub bucket: String,
    pub prefix: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Prefix,
    FolderMarker,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrySummary {
    pub schema_version: u16,
    pub id: String,
    pub kind: EntryKind,
    pub display_name: String,
    pub key: String,
    pub size: Option<u64>,
    pub last_modified: Option<String>,
    pub storage_class: Option<String>,
    pub content_type_hint: Option<String>,
    pub is_folder_marker: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListEntriesRequest {
    pub schema_version: u16,
    pub location: ExplorerLocation,
    pub continuation_token: Option<String>,
    pub page_size: u16,
    pub request_generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListEntriesPage {
    pub schema_version: u16,
    pub request_generation: u64,
    pub location: ExplorerLocation,
    pub entries: Vec<EntrySummary>,
    pub next_token: Option<String>,
    pub is_complete: bool,
    pub provider_request_id: Option<String>,
}
