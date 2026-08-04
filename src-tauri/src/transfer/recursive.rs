//! Planning and execution primitives for recursive transfer operations.
//!
//! The provider-facing adapters deliberately live outside this module.  The
//! planner owns path/key safety, deterministic ordering, collision decisions,
//! and the root boundary.  The worker then applies a provider adapter one item
//! at a time, which gives us a safe cancellation point and an explicit
//! partial-failure report for upload/download/copy/move prefix jobs.

use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    domain::error::{AppError, PublicError},
    dto::transfer::{CollisionPolicy, TransferEndpoint, TransferOperation, TransferStatus},
};

use super::path_mapping::{collision_path, map_key_to_local};

pub const MAX_RECURSIVE_ITEMS: usize = 100_000;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RemoteObject {
    pub key: String,
    pub size_bytes: Option<u64>,
    pub is_folder_marker: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CollisionResolution {
    Replace,
    Skip,
    Fail,
    Ask,
}

impl CollisionResolution {
    pub fn from_policy(policy: CollisionPolicy, destination_exists: bool) -> Self {
        if !destination_exists {
            return Self::Replace;
        }
        match policy {
            CollisionPolicy::Ask => Self::Ask,
            CollisionPolicy::Replace => Self::Replace,
            CollisionPolicy::Skip => Self::Skip,
            CollisionPolicy::Fail => Self::Fail,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RecursiveItem {
    pub id: String,
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub size_bytes: Option<u64>,
    pub is_directory: bool,
    pub collision: CollisionResolution,
}

impl RecursiveItem {
    fn destination_key(&self) -> String {
        match &self.destination {
            TransferEndpoint::Remote { bucket, key, .. } => format!("s3://{bucket}/{key}"),
            TransferEndpoint::Local { path } => path.clone(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RecursivePlan {
    pub schema_version: u16,
    pub operation: TransferOperation,
    pub items: Vec<RecursiveItem>,
    pub total_bytes: Option<u64>,
    pub total_items: u64,
    pub warning_count: u32,
}

impl RecursivePlan {
    fn new(operation: TransferOperation, mut items: Vec<RecursiveItem>) -> Result<Self, AppError> {
        if items.len() > MAX_RECURSIVE_ITEMS {
            return Err(AppError::Validation(format!(
                "recursive transfer contains more than {MAX_RECURSIVE_ITEMS} items"
            )));
        }
        items.sort_by(|left, right| {
            left.destination_key()
                .cmp(&right.destination_key())
                .then_with(|| left.id.cmp(&right.id))
        });
        let total_bytes = items
            .iter()
            .map(|item| item.size_bytes)
            .try_fold(0_u64, |total, size| {
                size.map(|value| total.saturating_add(value))
            })
            .map(Some)
            .unwrap_or(None);
        Ok(Self {
            schema_version: 1,
            operation,
            total_items: items.len() as u64,
            items,
            total_bytes,
            warning_count: 0,
        })
    }
}

/// Build an upload plan from a local directory.  Symlinks are rejected rather
/// than followed so a local link cannot escape the user-selected source root.
pub fn plan_upload_directory(
    source_root: &Path,
    profile_id: &str,
    bucket: &str,
    destination_prefix: &str,
    collision_policy: CollisionPolicy,
    existing_destination_keys: &HashSet<String>,
    preserve_empty_folders: bool,
) -> Result<RecursivePlan, AppError> {
    validate_profile_bucket(profile_id, bucket)?;
    if destination_prefix.contains('\0') || destination_prefix.starts_with('/') {
        return Err(AppError::Validation(
            "destination prefix is not a safe object prefix".to_string(),
        ));
    }
    let root = std::fs::canonicalize(source_root).map_err(AppError::Io)?;
    if !root.is_dir() {
        return Err(AppError::Validation(
            "upload source must be a directory".to_string(),
        ));
    }
    let mut items = Vec::new();
    walk_local_directory(
        &root,
        &root,
        profile_id,
        bucket,
        destination_prefix,
        collision_policy,
        existing_destination_keys,
        preserve_empty_folders,
        &mut items,
    )?;
    RecursivePlan::new(TransferOperation::UploadDirectory, items)
}

/// Build a download plan from the provider's already-enumerated remote object
/// page(s).  The caller is responsible for pagination; this function is purely
/// deterministic and never trusts a key outside the selected prefix.  When
/// `preserve_empty_folders` is false, marker-only directory objects are
/// omitted while markers that contain regular objects remain in the plan.
#[allow(clippy::too_many_arguments)]
pub fn plan_download_prefix(
    profile_id: &str,
    bucket: &str,
    source_prefix: &str,
    destination_root: &Path,
    objects: &[RemoteObject],
    collision_policy: CollisionPolicy,
    existing_local_paths: &HashSet<String>,
    preserve_empty_folders: bool,
) -> Result<RecursivePlan, AppError> {
    validate_profile_bucket(profile_id, bucket)?;
    let root = lexical_root(destination_root)?;
    let normalized_source = source_prefix.trim_matches('/');
    let mut items = Vec::new();
    let mut seen_destinations = BTreeSet::new();
    for object in objects {
        // Preserve the trailing slash for folder markers.  S3 represents an
        // empty folder as a zero-byte object whose key ends in `/`; trimming
        // it would turn a marker into a regular object during copy/move.
        let is_folder_marker = object.is_folder_marker || object.key.ends_with('/');
        let key = object.key.trim_end_matches('/');
        if key.is_empty() {
            continue;
        }
        if is_folder_marker && !preserve_empty_folders {
            let marker_prefix = format!("{key}/");
            let has_file_descendant = objects.iter().any(|candidate| {
                let candidate_is_marker =
                    candidate.is_folder_marker || candidate.key.ends_with('/');
                !candidate_is_marker && candidate.key.starts_with(&marker_prefix)
            });
            if !has_file_descendant {
                continue;
            }
        }
        let is_source_root_marker =
            is_folder_marker && !normalized_source.is_empty() && key == normalized_source;
        let mut local = if is_source_root_marker {
            root.clone()
        } else {
            map_key_to_local(&root, source_prefix, key)?
        };
        ensure_local_descendant(&root, &local)?;
        let mut destination_id = normalize_local_identity(&local);
        if !seen_destinations.insert(destination_id.clone()) {
            local = collision_path(&local, &object.key);
            ensure_local_descendant(&root, &local)?;
            destination_id = normalize_local_identity(&local);
            if !seen_destinations.insert(destination_id.clone()) {
                return Err(AppError::Validation(format!(
                    "remote keys map to the same local path: {}",
                    local.display()
                )));
            }
        }
        let exists = !is_source_root_marker
            && (local.exists()
                || existing_local_paths
                    .iter()
                    .any(|value| normalize_local_identity(Path::new(value)) == destination_id));
        let source = TransferEndpoint::Remote {
            profile_id: profile_id.to_string(),
            bucket: bucket.to_string(),
            key: object.key.clone(),
        };
        let destination = TransferEndpoint::Local {
            path: local.to_string_lossy().into_owned(),
        };
        items.push(RecursiveItem {
            id: object.key.clone(),
            source,
            destination,
            size_bytes: object.size_bytes,
            is_directory: is_folder_marker,
            collision: CollisionResolution::from_policy(collision_policy, exists),
        });
    }
    RecursivePlan::new(TransferOperation::DownloadPrefix, items)
}

/// Build a copy/move plan within one profile.  Cross-profile server-side
/// recursive transfers are intentionally rejected by the MVP contract.
#[allow(clippy::too_many_arguments)]
pub fn plan_remote_prefix(
    operation: TransferOperation,
    profile_id: &str,
    bucket: &str,
    source_prefix: &str,
    destination_prefix: &str,
    objects: &[RemoteObject],
    collision_policy: CollisionPolicy,
    existing_destination_keys: &HashSet<String>,
) -> Result<RecursivePlan, AppError> {
    if !matches!(
        operation,
        TransferOperation::CopyPrefix | TransferOperation::MovePrefix
    ) {
        return Err(AppError::Validation(
            "remote recursive planning only supports copyPrefix or movePrefix".to_string(),
        ));
    }
    validate_profile_bucket(profile_id, bucket)?;
    let source = normalize_prefix(source_prefix)?;
    let destination = normalize_prefix(destination_prefix)?;
    if source.is_empty() && destination.is_empty() {
        return Err(AppError::Validation(
            "source and destination prefixes must differ".to_string(),
        ));
    }
    if !source.is_empty()
        && (destination == source || destination.starts_with(&format!("{source}/")))
    {
        return Err(AppError::Validation(
            "destination prefix cannot be inside the source prefix".to_string(),
        ));
    }
    let mut items = Vec::new();
    let mut seen_destinations = BTreeSet::new();
    for object in objects {
        // Keep the marker bit separate from the normalized key.  A folder
        // marker is a real zero-byte S3 object and its trailing slash must be
        // retained at the destination (for example `foo/` -> `archive/foo/`).
        let is_folder_marker = object.is_folder_marker || object.key.ends_with('/');
        let key = object.key.trim_end_matches('/');
        // A root-prefix copy/move enumerates the whole bucket.  Do not feed
        // the destination subtree back into that plan, or `archive/*` would
        // become `archive/archive/*` (and a move could recursively delete its
        // own destination).
        if source.is_empty()
            && !destination.is_empty()
            && (key == destination || key.starts_with(&format!("{destination}/")))
        {
            continue;
        }
        let suffix = if source.is_empty() {
            key
        } else if key == source {
            ""
        } else if let Some(rest) = key.strip_prefix(&format!("{source}/")) {
            rest
        } else {
            continue;
        };
        if suffix.is_empty() && !is_folder_marker {
            continue;
        }
        let destination_key = if is_folder_marker {
            let destination_key = join_prefix(&destination, suffix);
            if destination_key.ends_with('/') {
                destination_key
            } else {
                format!("{destination_key}/")
            }
        } else {
            join_prefix(&destination, suffix)
        };
        if !seen_destinations.insert(destination_key.to_string()) {
            return Err(AppError::Validation(format!(
                "duplicate destination key planned: {destination_key}"
            )));
        }
        let exists = existing_destination_keys.contains(&destination_key);
        let source_endpoint = TransferEndpoint::Remote {
            profile_id: profile_id.to_string(),
            bucket: bucket.to_string(),
            key: object.key.clone(),
        };
        let destination_endpoint = TransferEndpoint::Remote {
            profile_id: profile_id.to_string(),
            bucket: bucket.to_string(),
            key: destination_key.clone(),
        };
        items.push(RecursiveItem {
            id: object.key.clone(),
            source: source_endpoint,
            destination: destination_endpoint,
            size_bytes: object.size_bytes,
            is_directory: is_folder_marker,
            collision: CollisionResolution::from_policy(collision_policy, exists),
        });
    }
    RecursivePlan::new(operation, items)
}

#[allow(clippy::too_many_arguments)]
fn walk_local_directory(
    root: &Path,
    current: &Path,
    profile_id: &str,
    bucket: &str,
    destination_prefix: &str,
    collision_policy: CollisionPolicy,
    existing_destination_keys: &HashSet<String>,
    preserve_empty_folders: bool,
    items: &mut Vec<RecursiveItem>,
) -> Result<(), AppError> {
    let mut entries = std::fs::read_dir(current)
        .map_err(AppError::Io)?
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut child_count = 0;
    for entry in entries {
        child_count += 1;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(AppError::Io)?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::Validation(format!(
                "symbolic links are not allowed in recursive uploads: {}",
                path.display()
            )));
        }
        ensure_local_descendant(root, &path)?;
        if metadata.is_dir() {
            let before = items.len();
            walk_local_directory(
                root,
                &path,
                profile_id,
                bucket,
                destination_prefix,
                collision_policy,
                existing_destination_keys,
                preserve_empty_folders,
                items,
            )?;
            if preserve_empty_folders && before == items.len() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| AppError::Validation("source escaped root".to_string()))?;
                let suffix = relative_to_key(relative)?;
                let key = format!("{}{}/", prefix_with_slash(destination_prefix), suffix);
                items.push(local_item(
                    profile_id,
                    bucket,
                    key,
                    &path,
                    None,
                    true,
                    collision_policy,
                    existing_destination_keys,
                ));
            }
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| AppError::Validation("source escaped root".to_string()))?;
            let suffix = relative_to_key(relative)?;
            let key = format!("{}{}", prefix_with_slash(destination_prefix), suffix);
            items.push(local_item(
                profile_id,
                bucket,
                key,
                &path,
                Some(metadata.len()),
                false,
                collision_policy,
                existing_destination_keys,
            ));
            if items.len() > MAX_RECURSIVE_ITEMS {
                return Err(AppError::Validation(format!(
                    "recursive transfer contains more than {MAX_RECURSIVE_ITEMS} items"
                )));
            }
        }
    }
    if child_count == 0 && current != root && preserve_empty_folders {
        // The caller adds the marker for this directory after the recursive
        // call, keeping traversal deterministic and avoiding duplicate rows.
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn local_item(
    profile_id: &str,
    bucket: &str,
    key: String,
    local_path: &Path,
    size_bytes: Option<u64>,
    is_directory: bool,
    collision_policy: CollisionPolicy,
    existing_destination_keys: &HashSet<String>,
) -> RecursiveItem {
    RecursiveItem {
        id: local_path.to_string_lossy().into_owned(),
        source: TransferEndpoint::Local {
            path: local_path.to_string_lossy().into_owned(),
        },
        destination: TransferEndpoint::Remote {
            profile_id: profile_id.to_string(),
            bucket: bucket.to_string(),
            key: key.clone(),
        },
        size_bytes,
        is_directory,
        collision: CollisionResolution::from_policy(
            collision_policy,
            existing_destination_keys.contains(&key),
        ),
    }
}

fn validate_profile_bucket(profile_id: &str, bucket: &str) -> Result<(), AppError> {
    if profile_id.trim().is_empty() || bucket.trim().is_empty() || bucket.contains('\0') {
        return Err(AppError::Validation(
            "profile ID and bucket are required".to_string(),
        ));
    }
    Ok(())
}

fn lexical_root(path: &Path) -> Result<PathBuf, AppError> {
    if path.as_os_str().is_empty() {
        return Err(AppError::Validation(
            "destination root is required".to_string(),
        ));
    }
    let root = if path.exists() {
        std::fs::canonicalize(path).map_err(AppError::Io)?
    } else {
        path.to_path_buf()
    };
    if !root.is_absolute() {
        return Err(AppError::Validation(
            "destination root must be an absolute path".to_string(),
        ));
    }
    Ok(root)
}

fn normalize_local_identity(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_ascii_lowercase()
}

fn ensure_local_descendant(root: &Path, candidate: &Path) -> Result<(), AppError> {
    if candidate.strip_prefix(root).is_err() {
        return Err(AppError::Validation(
            "local path escapes the selected root".to_string(),
        ));
    }
    if candidate.exists() {
        let canonical = std::fs::canonicalize(candidate).map_err(AppError::Io)?;
        if canonical.strip_prefix(root).is_err() {
            return Err(AppError::Validation(
                "local path escapes the selected root through a link".to_string(),
            ));
        }
    }
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| AppError::Validation("local path escapes the selected root".to_string()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                return Err(AppError::Validation(
                    "reparse points and symbolic links are not allowed under transfer roots"
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn normalize_prefix(prefix: &str) -> Result<String, AppError> {
    let normalized = prefix.trim_matches('/');
    if normalized.contains('\0') || normalized.contains('\\') {
        return Err(AppError::Validation(
            "object prefix contains an unsafe separator".to_string(),
        ));
    }
    if normalized.is_empty() {
        return Ok(String::new());
    }
    if normalized
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(AppError::Validation(
            "object prefix contains an unsafe segment".to_string(),
        ));
    }
    Ok(normalized.to_string())
}

fn prefix_with_slash(prefix: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    }
}

fn join_prefix(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        suffix.to_string()
    } else {
        format!("{prefix}/{suffix}")
    }
}

fn relative_to_key(relative: &Path) -> Result<String, AppError> {
    let value = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>()
        .join("/");
    if value.is_empty() || value.contains('\0') || value.starts_with('/') {
        return Err(AppError::Validation(
            "local path has no safe relative key".to_string(),
        ));
    }
    Ok(value)
}

#[derive(Clone, Default)]
pub struct CancellationFlag(Arc<AtomicBool>);

impl CancellationFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveProgress {
    pub schema_version: u16,
    pub completed_items: u64,
    pub failed_items: u64,
    pub skipped_items: u64,
    pub transferred_bytes: u64,
    pub total_items: u64,
    pub total_bytes: Option<u64>,
    pub current_item: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveFailure {
    pub schema_version: u16,
    pub item_id: String,
    pub error: PublicError,
    pub cleanup_required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveExecutionResult {
    pub schema_version: u16,
    pub status: TransferStatus,
    pub completed_items: u64,
    pub failed_items: u64,
    pub skipped_items: u64,
    pub cleanup_required_items: u64,
    pub transferred_bytes: u64,
    pub failures: Vec<RecursiveFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingManifest {
    pub schema_version: u16,
    pub source: MappingManifestSource,
    pub created_at: String,
    pub entries: Vec<MappingManifestEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingManifestSource {
    pub profile_id: String,
    pub bucket: String,
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingManifestEntry {
    pub key: String,
    pub relative_path: String,
}

/// Persist the reversible remote-key mapping for a recursive download.  The
/// manifest itself is local-only and never contains credentials or presigned
/// URLs. Existing user objects are never overwritten.
pub fn write_mapping_manifest(
    destination_root: &Path,
    profile_id: &str,
    bucket: &str,
    prefix: &str,
    plan: &RecursivePlan,
) -> Result<PathBuf, AppError> {
    let manifest_root =
        std::fs::canonicalize(destination_root).unwrap_or_else(|_| destination_root.to_path_buf());
    let mut entries = Vec::new();
    for item in &plan.items {
        let (TransferEndpoint::Remote { key, .. }, TransferEndpoint::Local { path }) =
            (&item.source, &item.destination)
        else {
            continue;
        };
        let relative = Path::new(path)
            .strip_prefix(&manifest_root)
            .map_err(|_| AppError::Validation("manifest path escaped download root".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(MappingManifestEntry {
            key: key.clone(),
            relative_path: relative,
        });
    }
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    let manifest = MappingManifest {
        schema_version: 1,
        source: MappingManifestSource {
            profile_id: profile_id.to_string(),
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
        },
        created_at: chrono::Utc::now().to_rfc3339(),
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| AppError::Unknown(format!("manifest serialization failed: {error}")))?;
    std::fs::create_dir_all(destination_root).map_err(AppError::Io)?;
    for attempt in 0_u32..100 {
        let path = if attempt == 0 {
            destination_root.join(".s3-key-map.json")
        } else {
            let mut input = bytes.clone();
            input.extend_from_slice(&attempt.to_le_bytes());
            let suffix_digest = Sha256::digest(&input);
            let suffix = suffix_digest[..4]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            destination_root.join(format!(".s3-key-map.{suffix}.json"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, &bytes).map_err(AppError::Io)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    Err(AppError::Validation(
        "unable to allocate a collision-free mapping manifest name".to_string(),
    ))
}

#[async_trait]
pub trait RecursiveExecutor: Send + Sync {
    async fn execute_item(
        &self,
        item: &RecursiveItem,
        cancellation: &CancellationFlag,
    ) -> Result<u64, AppError>;

    async fn delete_source(
        &self,
        item: &RecursiveItem,
        cancellation: &CancellationFlag,
    ) -> Result<(), AppError>;
}

/// Execute one deterministic plan.  The provider adapter is responsible for
/// streaming the actual bytes; this worker owns cancellation checkpoints,
/// move's copy-then-delete safety, and a partial-failure result.
pub async fn execute_recursive<E: RecursiveExecutor>(
    plan: &RecursivePlan,
    executor: &E,
    cancellation: &CancellationFlag,
    progress: Option<mpsc::Sender<RecursiveProgress>>,
) -> RecursiveExecutionResult {
    let mut completed_items = 0_u64;
    let mut failed_items = 0_u64;
    let mut skipped_items = 0_u64;
    let mut cleanup_required_items = 0_u64;
    let mut transferred_bytes = 0_u64;
    let mut failures = Vec::new();

    for item in &plan.items {
        if cancellation.is_cancelled() {
            break;
        }
        if item.collision == CollisionResolution::Skip {
            skipped_items += 1;
            emit_progress(
                &progress,
                completed_items,
                failed_items,
                skipped_items,
                transferred_bytes,
                plan,
                Some(item.id.clone()),
            )
            .await;
            continue;
        }
        if item.collision == CollisionResolution::Ask {
            failed_items += 1;
            failures.push(failure(
                item,
                AppError::Validation(
                    "destination collision requires user confirmation".to_string(),
                ),
                false,
            ));
            continue;
        }
        if item.collision == CollisionResolution::Fail {
            failed_items += 1;
            failures.push(failure(
                item,
                AppError::Validation("destination already exists".to_string()),
                false,
            ));
            continue;
        }
        match executor.execute_item(item, cancellation).await {
            Ok(bytes) => {
                transferred_bytes = transferred_bytes.saturating_add(bytes);
                if plan.operation == TransferOperation::MovePrefix {
                    if let Err(error) = executor.delete_source(item, cancellation).await {
                        cleanup_required_items += 1;
                        failed_items += 1;
                        failures.push(failure(item, error, true));
                        continue;
                    }
                }
                completed_items += 1;
            }
            Err(error) => {
                failed_items += 1;
                failures.push(failure(item, error, false));
            }
        }
        emit_progress(
            &progress,
            completed_items,
            failed_items,
            skipped_items,
            transferred_bytes,
            plan,
            Some(item.id.clone()),
        )
        .await;
    }

    let status = if cancellation.is_cancelled() {
        TransferStatus::Cancelled
    } else if failed_items > 0 || cleanup_required_items > 0 {
        TransferStatus::CompletedWithWarnings
    } else {
        TransferStatus::Completed
    };
    RecursiveExecutionResult {
        schema_version: 1,
        status,
        completed_items,
        failed_items,
        skipped_items,
        cleanup_required_items,
        transferred_bytes,
        failures,
    }
}

fn failure(item: &RecursiveItem, error: AppError, cleanup_required: bool) -> RecursiveFailure {
    RecursiveFailure {
        schema_version: 1,
        item_id: item.id.clone(),
        error: PublicError::from(error),
        cleanup_required,
    }
}

async fn emit_progress(
    sender: &Option<mpsc::Sender<RecursiveProgress>>,
    completed_items: u64,
    failed_items: u64,
    skipped_items: u64,
    transferred_bytes: u64,
    plan: &RecursivePlan,
    current_item: Option<String>,
) {
    if let Some(sender) = sender {
        let _ = sender
            .send(RecursiveProgress {
                schema_version: 1,
                completed_items,
                failed_items,
                skipped_items,
                transferred_bytes,
                total_items: plan.total_items,
                total_bytes: plan.total_bytes,
                current_item,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        fs::{self, File},
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "s3-file-manager-recursive-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn upload_planner_is_deterministic_and_preserves_empty_folders() {
        let root = temp_dir("upload");
        fs::create_dir_all(root.join("z/empty")).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        let mut file = File::create(root.join("z/file.txt")).unwrap();
        file.write_all(b"hello").unwrap();
        let plan = plan_upload_directory(
            &root,
            "profile",
            "bucket",
            "incoming",
            CollisionPolicy::Replace,
            &HashSet::new(),
            true,
        )
        .unwrap();
        assert_eq!(plan.operation, TransferOperation::UploadDirectory);
        assert_eq!(plan.total_items, 3);
        assert!(plan.items[0].is_directory);
        assert!(plan.items.iter().any(|item| item.is_directory
            && matches!(
                &item.destination,
                TransferEndpoint::Remote { key, .. } if key == "incoming/z/empty/"
            )));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_planner_rejects_colliding_mapped_paths_and_traversal() {
        let root = temp_dir("download");
        let objects = [
            RemoteObject {
                key: "prefix/a:b.txt".to_string(),
                size_bytes: Some(1),
                is_folder_marker: false,
            },
            RemoteObject {
                key: "prefix/a?b.txt".to_string(),
                size_bytes: Some(1),
                is_folder_marker: false,
            },
        ];
        let mut existing = HashSet::new();
        let existing_path = map_key_to_local(&root, "prefix", "prefix/a:b.txt").unwrap();
        existing.insert(existing_path.to_string_lossy().to_ascii_lowercase());
        let collision_plan = plan_download_prefix(
            "profile",
            "bucket",
            "prefix",
            &root,
            &objects[..1],
            CollisionPolicy::Skip,
            &existing,
            true,
        )
        .unwrap();
        assert_eq!(collision_plan.items[0].collision, CollisionResolution::Skip);
        let traversal = vec![RemoteObject {
            key: "prefix/../secret.txt".to_string(),
            size_bytes: Some(1),
            is_folder_marker: false,
        }];
        let traversal_plan = plan_download_prefix(
            "profile",
            "bucket",
            "prefix",
            &root,
            &traversal,
            CollisionPolicy::Replace,
            &HashSet::new(),
            true,
        )
        .unwrap();
        assert!(matches!(
            &traversal_plan.items[0].destination,
            TransferEndpoint::Local { path } if path.contains("_s3x2E_")
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_planner_maps_source_root_marker_to_destination_root() {
        let root = temp_dir("download-root-marker");
        let objects = [RemoteObject {
            key: "photos/".to_string(),
            size_bytes: Some(0),
            is_folder_marker: true,
        }];
        let plan = plan_download_prefix(
            "profile",
            "bucket",
            "photos/",
            &root,
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
            true,
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert!(plan.items[0].is_directory);
        assert!(matches!(
            &plan.items[0].destination,
            TransferEndpoint::Local { path } if Path::new(path).canonicalize().ok() == root.canonicalize().ok()
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_planner_skips_marker_only_directories_when_disabled() {
        let root = temp_dir("download-no-empty");
        let objects = [
            RemoteObject {
                key: "prefix/empty/".to_string(),
                size_bytes: Some(0),
                is_folder_marker: true,
            },
            RemoteObject {
                key: "prefix/filled/".to_string(),
                size_bytes: Some(0),
                is_folder_marker: true,
            },
            RemoteObject {
                key: "prefix/filled/file.txt".to_string(),
                size_bytes: Some(1),
                is_folder_marker: false,
            },
        ];
        let plan = plan_download_prefix(
            "profile",
            "bucket",
            "prefix",
            &root,
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
            false,
        )
        .unwrap();
        assert_eq!(plan.items.len(), 2);
        assert!(!plan.items.iter().any(|item| item.id == "prefix/empty/"));
        assert!(plan.items.iter().any(|item| item.id == "prefix/filled/"));
        assert!(plan
            .items
            .iter()
            .any(|item| item.id == "prefix/filled/file.txt"));
        assert!(plan.items.iter().any(|item| matches!(
            &item.destination,
            TransferEndpoint::Local { path } if path.ends_with("filled")
        )));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_planner_rejects_self_copy_and_resolves_collisions() {
        let objects = vec![RemoteObject {
            key: "photos/a.txt".to_string(),
            size_bytes: Some(3),
            is_folder_marker: false,
        }];
        assert!(plan_remote_prefix(
            TransferOperation::CopyPrefix,
            "profile",
            "bucket",
            "photos",
            "photos/archive",
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
        )
        .is_err());
        let mut existing = HashSet::new();
        existing.insert("archive/a.txt".to_string());
        let plan = plan_remote_prefix(
            TransferOperation::MovePrefix,
            "profile",
            "bucket",
            "photos",
            "archive",
            &objects,
            CollisionPolicy::Skip,
            &existing,
        )
        .unwrap();
        assert_eq!(plan.items[0].collision, CollisionResolution::Skip);
    }

    #[test]
    fn remote_root_planner_excludes_destination_subtree() {
        let objects = vec![
            RemoteObject {
                key: "archive/old.txt".to_string(),
                size_bytes: Some(1),
                is_folder_marker: false,
            },
            RemoteObject {
                key: "archive/".to_string(),
                size_bytes: Some(0),
                is_folder_marker: true,
            },
            RemoteObject {
                key: "other.txt".to_string(),
                size_bytes: Some(2),
                is_folder_marker: false,
            },
        ];
        let plan = plan_remote_prefix(
            TransferOperation::CopyPrefix,
            "profile",
            "bucket",
            "",
            "archive",
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].id, "other.txt");
        assert!(plan_remote_prefix(
            TransferOperation::CopyPrefix,
            "profile",
            "bucket",
            "",
            "",
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
        )
        .is_err());
    }

    #[test]
    fn remote_planner_preserves_folder_marker_trailing_slash() {
        let objects = vec![RemoteObject {
            key: "photos/empty/".to_string(),
            size_bytes: Some(0),
            is_folder_marker: true,
        }];
        let plan = plan_remote_prefix(
            TransferOperation::CopyPrefix,
            "profile",
            "bucket",
            "photos",
            "archive",
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert!(matches!(
            &plan.items[0].destination,
            TransferEndpoint::Remote { key, .. } if key == "archive/empty/"
        ));
        assert!(plan.items[0].is_directory);
    }

    #[test]
    fn mapping_manifest_is_created_without_overwriting_existing_objects() {
        let root = temp_dir("manifest");
        let objects = vec![RemoteObject {
            key: "prefix/a:b.txt".to_string(),
            size_bytes: Some(1),
            is_folder_marker: false,
        }];
        let plan = plan_download_prefix(
            "profile",
            "bucket",
            "prefix",
            &root,
            &objects,
            CollisionPolicy::Replace,
            &HashSet::new(),
            true,
        )
        .unwrap();
        let first = write_mapping_manifest(&root, "profile", "bucket", "prefix", &plan).unwrap();
        assert_eq!(first.file_name().unwrap(), ".s3-key-map.json");
        let second = write_mapping_manifest(&root, "profile", "bucket", "prefix", &plan).unwrap();
        assert_ne!(first, second);
        assert!(second
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".s3-key-map."));
        let _ = fs::remove_dir_all(root);
    }

    struct FakeExecutor {
        fail_id: Option<String>,
        delete_fail: bool,
    }

    #[async_trait]
    impl RecursiveExecutor for FakeExecutor {
        async fn execute_item(
            &self,
            item: &RecursiveItem,
            _cancellation: &CancellationFlag,
        ) -> Result<u64, AppError> {
            if self.fail_id.as_deref() == Some(item.id.as_str()) {
                Err(AppError::Provider("simulated transfer failure".to_string()))
            } else {
                Ok(item.size_bytes.unwrap_or_default())
            }
        }

        async fn delete_source(
            &self,
            _item: &RecursiveItem,
            _cancellation: &CancellationFlag,
        ) -> Result<(), AppError> {
            if self.delete_fail {
                Err(AppError::Provider("simulated delete failure".to_string()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn worker_reports_partial_failures_and_move_cleanup() {
        let items = vec![
            RecursiveItem {
                id: "ok".to_string(),
                source: TransferEndpoint::Remote {
                    profile_id: "p".to_string(),
                    bucket: "b".to_string(),
                    key: "a".to_string(),
                },
                destination: TransferEndpoint::Remote {
                    profile_id: "p".to_string(),
                    bucket: "b".to_string(),
                    key: "x/a".to_string(),
                },
                size_bytes: Some(4),
                is_directory: false,
                collision: CollisionResolution::Replace,
            },
            RecursiveItem {
                id: "bad".to_string(),
                source: TransferEndpoint::Remote {
                    profile_id: "p".to_string(),
                    bucket: "b".to_string(),
                    key: "b".to_string(),
                },
                destination: TransferEndpoint::Remote {
                    profile_id: "p".to_string(),
                    bucket: "b".to_string(),
                    key: "x/b".to_string(),
                },
                size_bytes: Some(2),
                is_directory: false,
                collision: CollisionResolution::Replace,
            },
        ];
        let plan = RecursivePlan::new(TransferOperation::MovePrefix, items).unwrap();
        let result = execute_recursive(
            &plan,
            &FakeExecutor {
                fail_id: Some("bad".to_string()),
                delete_fail: false,
            },
            &CancellationFlag::default(),
            None,
        )
        .await;
        assert_eq!(result.status, TransferStatus::CompletedWithWarnings);
        assert_eq!(result.completed_items, 1);
        assert_eq!(result.failed_items, 1);
        assert_eq!(result.transferred_bytes, 4);
    }

    #[tokio::test]
    async fn worker_honors_cancellation_between_items() {
        let mut items = Vec::new();
        for id in ["a", "b"] {
            items.push(RecursiveItem {
                id: id.to_string(),
                source: TransferEndpoint::Local {
                    path: format!("C:/{id}"),
                },
                destination: TransferEndpoint::Remote {
                    profile_id: "p".to_string(),
                    bucket: "b".to_string(),
                    key: id.to_string(),
                },
                size_bytes: Some(1),
                is_directory: false,
                collision: CollisionResolution::Replace,
            });
        }
        let plan = RecursivePlan::new(TransferOperation::UploadDirectory, items).unwrap();
        let cancel = CancellationFlag::default();
        cancel.cancel();
        let result = execute_recursive(
            &plan,
            &FakeExecutor {
                fail_id: None,
                delete_fail: false,
            },
            &cancel,
            None,
        )
        .await;
        assert_eq!(result.status, TransferStatus::Cancelled);
        assert_eq!(result.completed_items, 0);
    }
}
