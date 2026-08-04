use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const METADATA_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_PREVIEW_LIMIT_BYTES: u32 = 2_097_152;
pub const MAX_SHARE_SECONDS: u32 = 604_800;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    Text,
    Image,
    Audio,
    Video,
    Pdf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRequest {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataEditRequest {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    /// None preserves the existing value; a supplied value replaces it.
    pub content_type: Option<String>,
    /// None preserves the existing value; a supplied value replaces it.
    pub content_disposition: Option<String>,
    /// None preserves the existing value; a supplied value replaces it.
    pub cache_control: Option<String>,
    /// Replaces the complete user-metadata map when supplied.
    pub user_metadata: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataEditResult {
    pub schema_version: u16,
    pub metadata: ObjectMetadata,
    pub warning: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    pub max_bytes: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLinkRequest {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    pub expires_in_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMetadata {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    pub size: Option<u64>,
    pub etag: Option<String>,
    pub version_id: Option<String>,
    pub last_modified: Option<String>,
    pub storage_class: Option<String>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<String>,
    pub checksum_sha256: Option<String>,
    pub checksum_sha1: Option<String>,
    pub checksum_crc32: Option<String>,
    pub checksum_crc32c: Option<String>,
    pub encryption: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    pub preview_supported: bool,
    pub preview_kind: Option<PreviewKind>,
    pub preview_reason: Option<String>,
    /// Whether a temporary presigned GET can be created for this profile.
    /// Unknown/custom providers stay disabled until a capability observation
    /// explicitly confirms support.
    pub share_supported: bool,
    pub share_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    pub preview_kind: PreviewKind,
    pub content_type: String,
    pub text: String,
    pub url: Option<String>,
    pub expires_at: Option<String>,
    pub bytes_read: u32,
    pub total_size: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLink {
    pub schema_version: u16,
    pub profile_id: String,
    pub bucket: String,
    pub key: String,
    pub url: String,
    pub expires_at: String,
    pub expires_in_seconds: u32,
}
