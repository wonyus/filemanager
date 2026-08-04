use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::error::PublicError;
use crate::dto::settings::SettingsSnapshot;

pub const DTO_SCHEMA_VERSION: u16 = 1;

fn default_schema_version() -> u16 {
    DTO_SCHEMA_VERSION
}

fn default_preserve_root() -> bool {
    true
}

/// A long-running operation that is owned by the Rust transfer manager.
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferOperation {
    CreateFolder,
    UploadFile,
    UploadDirectory,
    DownloadFile,
    DownloadPrefix,
    CopyObject,
    CopyPrefix,
    MoveObject,
    MovePrefix,
    DeleteObjects,
}

impl TransferOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateFolder => "createFolder",
            Self::UploadFile => "uploadFile",
            Self::UploadDirectory => "uploadDirectory",
            Self::DownloadFile => "downloadFile",
            Self::DownloadPrefix => "downloadPrefix",
            Self::CopyObject => "copyObject",
            Self::CopyPrefix => "copyPrefix",
            Self::MoveObject => "moveObject",
            Self::MovePrefix => "movePrefix",
            Self::DeleteObjects => "deleteObjects",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferStatus {
    Queued,
    Planning,
    WaitingForUser,
    Running,
    Pausing,
    Paused,
    Retrying,
    Cancelling,
    Completed,
    CompletedWithWarnings,
    Failed,
    Cancelled,
    Interrupted,
}

impl TransferStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Planning => "planning",
            Self::WaitingForUser => "waitingForUser",
            Self::Running => "running",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Retrying => "retrying",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::CompletedWithWarnings => "completedWithWarnings",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::CompletedWithWarnings
                | Self::Failed
                | Self::Cancelled
                | Self::Interrupted
        )
    }

    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CollisionPolicy {
    #[default]
    Ask,
    Replace,
    Skip,
    Fail,
    Rename,
}

impl CollisionPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Replace => "replace",
            Self::Skip => "skip",
            Self::Fail => "fail",
            Self::Rename => "rename",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TransferEndpoint {
    Remote {
        profile_id: String,
        bucket: String,
        key: String,
    },
    Local {
        path: String,
    },
}

/// Optional HTTP metadata applied to uploaded objects. Values are validated at
/// the command boundary and are never treated as credential material.
#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadMetadata {
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
    pub cache_control: Option<String>,
    #[serde(default)]
    pub user_metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferJob {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: Uuid,
    pub operation: TransferOperation,
    pub profile_id: Option<String>,
    pub source: TransferEndpoint,
    pub destination: Option<TransferEndpoint>,
    pub status: TransferStatus,
    pub collision_policy: CollisionPolicy,
    pub total_bytes: Option<u64>,
    pub transferred_bytes: u64,
    pub total_items: Option<u64>,
    pub completed_items: u64,
    pub failed_items: u64,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub retry_count: u32,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<PublicError>,
    #[serde(default)]
    pub mapping_manifest_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSummary {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: Uuid,
    pub operation: TransferOperation,
    pub status: TransferStatus,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub failed_items: u64,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferItem {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: String,
    pub source_key: Option<String>,
    pub destination_key: Option<String>,
    pub local_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub status: TransferStatus,
    pub retry_count: u32,
    pub error: Option<PublicError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferDetails {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub job: TransferJob,
    pub items: Vec<TransferItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub transfer_id: Uuid,
    pub status: TransferStatus,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub transfer_id: Uuid,
    pub item_id: String,
    pub status: TransferStatus,
    pub bytes: u64,
    pub error: Option<PublicError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicWarning {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub code: String,
    pub message: String,
    pub details: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum TransferChannelMessage {
    Snapshot(TransferJob),
    Progress(TransferProgress),
    ItemCompleted(ItemResult),
    Warning(PublicWarning),
    StateChanged(TransferStatus),
    Finished(TransferResult),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub transfer_id: Uuid,
    pub status: TransferStatus,
    pub completed_items: u64,
    pub failed_items: u64,
    pub cleanup_required_items: u64,
    pub error: Option<PublicError>,
    #[serde(default)]
    pub mapping_manifest_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTransfersRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub profile_id: Option<String>,
    pub include_active: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListTransfersRequest {
    fn default() -> Self {
        Self {
            schema_version: DTO_SCHEMA_VERSION,
            profile_id: None,
            include_active: true,
            limit: 100,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTransferRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub operation: TransferOperation,
    pub profile_id: Option<String>,
    pub source: TransferEndpoint,
    pub destination: Option<TransferEndpoint>,
    #[serde(default)]
    pub collision_policy: CollisionPolicy,
    pub total_bytes: Option<u64>,
    pub total_items: Option<u64>,
    /// Destructive operations require an explicit confirmation token.  The
    /// token is never persisted in transfer history.
    pub confirmation: Option<String>,
    /// Optional explicit object selection for one batched delete job. The
    /// source endpoint supplies the profile and bucket for every key.
    #[serde(default)]
    pub delete_keys: Option<Vec<String>>,
    /// Delete operations remove the object identified by `source` by default.
    /// When set, or when the key ends in `/`, the object is treated as a
    /// prefix and all matching objects are deleted in bounded batches.
    #[serde(default)]
    pub recursive: bool,
    /// Internal retry marker used for Copy → Verify → Delete cleanup jobs.
    #[serde(skip)]
    pub cleanup_only: bool,
    /// Upload-directory policy: include the selected folder name in the
    /// destination prefix when enabled.
    #[serde(default = "default_preserve_root")]
    pub preserve_root: bool,
    /// Copy metadata policy. `false` preserves source metadata; `true` uses
    /// replacement metadata supplied in `metadata` (or clears it when absent).
    #[serde(default)]
    pub replace_metadata: bool,
    /// Optional HTTP and user metadata for upload operations.
    #[serde(default)]
    pub metadata: Option<UploadMetadata>,
    /// Filled by the Rust command boundary when a job is created.  It is
    /// skipped in renderer serialization so settings never become a UI input.
    #[serde(skip)]
    pub settings_snapshot: Option<SettingsSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferHistoryPage {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub items: Vec<TransferSummary>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearTransferHistoryRequest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub before: Option<String>,
    pub include_failed: bool,
}

impl Default for ClearTransferHistoryRequest {
    fn default() -> Self {
        Self {
            schema_version: DTO_SCHEMA_VERSION,
            before: None,
            include_failed: false,
        }
    }
}
