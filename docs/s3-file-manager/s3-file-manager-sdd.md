# Software Design Document (SDD)

## S3 File Manager Desktop

**Document ID:** S3FM-SDD-001  
**Version:** 2.0 Final Implementation Specification  
**Status:** Approved for MVP implementation  
**Date:** 2026-08-04  
**Primary platform:** Windows 10/11 x64  
**Future platforms:** Windows ARM64, macOS, Linux  
**Desktop framework:** Tauri 2  
**Frontend:** React + TypeScript + Vite  
**Backend:** Rust + Tokio  

---

## 1. Document control

### 1.1 Purpose

This document defines the software architecture and detailed design for a cross-platform desktop application that manages files in Amazon S3 and S3-compatible object storage. The application presents an experience similar to a desktop file explorer while preserving the actual semantics and limitations of object storage.

The design is normative for MVP implementation, code review, security review, testing, release engineering, and future maintenance. All MVP decisions that previously remained open are resolved in this version.

### 1.2 Intended audience

- Product owner
- Software architect
- Rust backend developers
- React frontend developers
- QA and test engineers
- Security reviewers
- Release and DevOps engineers
- Technical writers

### 1.3 Revision history

| Version | Date | Description |
|---|---:|---|
| 0.1 | 2026-08-04 | Initial architecture based on S3 File Explorer concept |
| 0.5 | 2026-08-04 | Changed desktop stack from Wails/Go to Tauri 2/Rust |
| 0.8 | 2026-08-04 | Added multi-profile, transfer manager, security, and packaging design |
| 1.0 Draft | 2026-08-04 | Consolidated architecture baseline SDD |
| 2.0 Final | 2026-08-04 | Completed all MVP interaction, DTO, persistence, transfer, provider, packaging, test, and acceptance specifications |

### 1.4 Normative language

The terms **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** indicate requirement strength.

---

## 2. Executive summary

S3 File Manager Desktop is a Windows-first desktop application for browsing and managing objects across multiple Amazon S3 and S3-compatible connections. It supports connection profiles for AWS S3, Cloudflare R2, MinIO, Wasabi, and custom S3-compatible endpoints.

The application uses:

- Tauri 2 for desktop packaging, native integration, IPC, permissions, and update support
- React and TypeScript for the user interface
- Rust and Tokio for all privileged operations and asynchronous work
- AWS SDK for Rust for S3 operations
- SQLite and SQLx for non-secret local application data
- An operating-system credential-store abstraction for secret storage
- A persistent transfer subsystem for uploads, downloads, copies, moves, and recursive operations

The frontend never receives permanent access secrets. Local file and network operations are executed in Rust. The application uses Tauri commands for request-response operations and Tauri channels for ordered transfer progress.

The first release targets Windows 10/11 x64 and is distributed as an NSIS `setup.exe`. macOS and Linux support are architectural goals but are outside the first production release.

---

## 3. Product definition

### 3.1 Product statement

> A secure desktop file manager for Amazon S3 and S3-compatible object storage, with multiple connection profiles, familiar explorer-style navigation, and reliable large-file transfers.

### 3.2 Primary user groups

1. Developers managing development and production buckets
2. Infrastructure and operations staff managing object storage
3. Content creators managing audio, video, image, and album assets
4. Users of Cloudflare R2, MinIO, Wasabi, or custom S3-compatible services
5. Teams requiring a desktop utility without installing a full cloud administration suite

### 3.3 Core user value

- Switch quickly between independent storage accounts and endpoints
- Browse S3 prefixes as familiar folders
- Upload and download large files with progress and retry
- Perform common object operations without using a command line
- Keep permanent credentials out of browser JavaScript
- Support AWS S3 and compatible services from one application

---

## 4. Goals and non-goals

### 4.1 Goals

The system MUST:

1. Support multiple saved connection profiles.
2. Support AWS S3 and selected S3-compatible providers.
3. Present buckets, prefixes, and objects in an explorer-style interface.
4. Support reliable upload and download of large files.
5. Support object copy, move, rename, delete, and folder-style recursive operations.
6. Keep permanent secrets outside the React runtime and SQLite database.
7. Remain responsive during network and local filesystem operations.
8. Expose meaningful errors instead of raw SDK debug output.
9. Package as a signed Windows desktop installer.
10. Use an architecture that can later support macOS and Linux.

### 4.2 Non-goals for the first release

The first release does NOT aim to:

- Mount S3 as a Windows drive letter.
- Provide bidirectional filesystem synchronization.
- Replace the AWS management console for IAM or bucket policy administration.
- Implement a multi-user server or web portal.
- Provide mobile applications.
- Guarantee feature parity across every S3-compatible implementation.
- Provide real filesystem semantics or atomic directory renames.
- Provide global full-text search without scanning or indexing.
- Support AWS IAM Identity Center/SSO in the MVP.
- Support cross-profile copy in the MVP.

---

## 5. Scope

### 5.1 MVP scope

#### Profiles and connectivity

- Create, edit, duplicate, delete, favorite, and test profiles
- AWS S3 preset
- Cloudflare R2 preset
- MinIO preset
- Wasabi preset
- Custom S3-compatible preset
- Static access key and secret key credentials
- Optional temporary session token
- Known/default bucket support when `ListBuckets` is denied
- Custom endpoint and region
- Virtual-hosted and path-style addressing configuration
- Explicit warning for insecure HTTP endpoints

#### Explorer

- List buckets where permitted
- Open a configured bucket directly
- Browse object prefixes as folders
- Breadcrumb navigation
- Back, forward, up, and refresh
- Paginated listing
- List and grid views
- Sort loaded results
- Filter the current loaded folder
- Multi-select
- Keyboard navigation and common shortcuts
- File and folder context menus
- Favorites and recent locations

#### Object operations

- Create zero-byte folder markers
- Upload files
- Upload folders recursively
- Multipart upload
- Download files
- Download folders recursively
- Copy and move objects within one profile
- Rename a single object
- Rename/move a prefix recursively
- Delete one or many objects
- Recursive prefix deletion
- Generate short-lived share/download links when supported
- Basic object metadata display

#### Transfer management

- Transfer queue
- Progress, transferred bytes, total bytes, speed, and ETA
- Configurable concurrency
- Retry with exponential backoff and jitter
- Cancel
- Pause/resume during the current application session where technically safe
- Partial-failure reporting for recursive operations
- Transfer history

#### Security and diagnostics

- OS credential store for permanent secrets
- Least-privilege Tauri capabilities
- Secret redaction
- Local structured logs
- Diagnostic export without secrets
- Signed update support architecture

#### Distribution

- Windows 10/11 x64
- NSIS per-user installer
- WebView2 bootstrapper handling
- Code-signing-ready release pipeline

### 5.2 Deferred scope

- AWS IAM Identity Center/SSO
- AssumeRole workflow
- Full AWS shared configuration/profile support
- Cross-profile and cross-provider streaming copy
- Persistent transfer resume after application restart
- Global recursive search and local object index
- Version browser and undelete workflow
- Object Lock administration
- Storage class migration tools
- Sync engine
- Scheduled transfers
- Bandwidth schedules
- Multiple application windows
- Plugin system
- Windows shell integration
- macOS and Linux production releases
- Portable encrypted profile package containing credentials

---

## 6. Assumptions and constraints

### 6.1 Assumptions

- The user possesses credentials and endpoint information for each profile.
- Providers implement enough of the S3 API for required operations.
- Local storage is available for SQLite, logs, temporary downloads, and preview cache.
- Windows users have or can install the WebView2 runtime.
- Network behavior can be unreliable and operations must be retryable.
- Object keys can contain Unicode and characters that require careful encoding.

### 6.2 Constraints

- S3 does not provide real folders for general-purpose buckets; folders are represented by prefixes.
- Object rename and move are normally copy-then-delete operations and are not atomic.
- Prefix rename/move/delete may involve large numbers of objects.
- Provider implementations may differ from AWS behavior.
- A desktop application cannot guarantee data recovery after permanent deletion when bucket versioning is disabled.
- Large files MUST NOT be loaded entirely into process memory.
- Permanent secret values MUST NOT enter React state, browser storage, logs, or analytics.

---

## 7. Definitions

| Term | Definition |
|---|---|
| Profile | Saved connection configuration for one account/endpoint and credential set |
| Provider | AWS S3 or an S3-compatible service |
| Bucket | Top-level S3 object container |
| Object | A value stored under an S3 object key |
| Key | Full object name, such as `music/album/song.mp3` |
| Prefix | Initial portion of object keys used to emulate folder hierarchy |
| Folder marker | Zero-byte object whose key ends with `/` |
| Transfer | Upload, download, copy, move, or recursive operation |
| Capability | Detected or configured provider feature support |
| IPC | Communication between React webview and Rust backend |
| Secret reference | Opaque identifier pointing to a secret in the credential store |

---

## 8. User roles and use cases

### 8.1 User roles

The desktop application is single-user at the operating-system account level. It does not implement application-level team accounts in the MVP.

- **Standard user:** Creates profiles and manages accessible objects.
- **Power user:** Uses custom endpoints and advanced transfer settings.
- **Support engineer:** Reads redacted logs and diagnostic reports.

### 8.2 Primary use cases

#### UC-001 Create a connection profile

1. User selects Add Profile.
2. User selects provider preset.
3. UI displays provider-appropriate fields.
4. User enters endpoint, region, bucket, and credentials.
5. Rust validates configuration.
6. Rust tests connectivity.
7. Non-secret metadata is saved to SQLite.
8. Secrets are saved to the OS credential store.

#### UC-002 Browse a bucket

1. User selects a profile.
2. Application creates or retrieves a cached S3 client.
3. Application lists buckets when permitted.
4. If bucket listing is denied, application opens the configured default bucket.
5. Application requests `ListObjectsV2` using prefix, delimiter, and continuation token.
6. UI renders `CommonPrefixes` as folders and `Contents` as files.

#### UC-003 Upload a large file

1. User selects local file and destination prefix.
2. Rust validates local path, object key, space, and permissions.
3. Transfer manager creates a job.
4. Small files use a single upload; large files use multipart upload.
5. Progress is streamed to the frontend through a Tauri channel.
6. Failed retryable parts are retried.
7. Completed multipart upload is verified and committed.

#### UC-004 Rename an object

1. User enters a destination object name.
2. Application checks for destination collision.
3. Application copies source to destination.
4. Application verifies destination result.
5. Application deletes source.
6. If source deletion fails, UI reports a completed copy with cleanup required.

#### UC-005 Rename a folder-style prefix

1. Application enumerates all objects under the source prefix.
2. Application creates an operation plan.
3. User confirms object count and impact.
4. Transfer manager copies objects to destination prefix.
5. Verified source objects are deleted according to policy.
6. Partial failures remain visible and retryable.

#### UC-006 Download a folder

1. Application enumerates source objects.
2. Object keys are mapped to safe local relative paths.
3. Each object downloads to a temporary `.partial` path.
4. Completed files are atomically renamed where supported.
5. Collisions follow the chosen overwrite policy.

---

## 9. Functional requirements

### 9.1 Profile requirements

- **FR-PRO-001:** The application MUST store multiple connection profiles.
- **FR-PRO-002:** Each profile MUST have a stable UUID independent of its display name.
- **FR-PRO-003:** Profile names MUST be unique only for display convenience; duplicate names MAY be allowed with distinct IDs.
- **FR-PRO-004:** The user MUST be able to test a profile before saving.
- **FR-PRO-005:** The application MUST support a default bucket for credentials without `ListBuckets` permission.
- **FR-PRO-006:** Editing endpoint, region, addressing mode, or credentials MUST invalidate the cached client.
- **FR-PRO-007:** Deleting a profile MUST delete its credential-store entries after user confirmation.
- **FR-PRO-008:** Profile export MUST exclude secrets by default.
- **FR-PRO-009:** Profile import MUST never trust imported endpoint, paths, or provider capability claims without validation.
- **FR-PRO-010:** The application MUST support an optional root prefix that constrains navigation.

### 9.2 Explorer requirements

- **FR-EXP-001:** The application MUST represent S3 prefixes as folders.
- **FR-EXP-002:** Listing MUST use pagination and MUST NOT assume one request returns the full folder.
- **FR-EXP-003:** The UI MUST distinguish files, prefixes, and folder markers.
- **FR-EXP-004:** The application MUST preserve exact object keys even when display names are normalized for UI.
- **FR-EXP-005:** Breadcrumbs MUST never allow navigation above a profile root prefix.
- **FR-EXP-006:** Refresh MUST replace stale folder results without duplicating entries.
- **FR-EXP-007:** Current-folder filtering MUST operate only on currently loaded entries in the MVP.
- **FR-EXP-008:** Sort MUST clearly indicate whether it applies to loaded entries or the full remote folder.
- **FR-EXP-009:** Navigation history MUST be scoped to a profile and bucket.
- **FR-EXP-010:** Listing cancellation MUST occur when the user navigates to a new location.

### 9.3 Upload requirements

- **FR-UP-001:** File content MUST be read in Rust, not JavaScript.
- **FR-UP-002:** Files over a configured threshold MUST use multipart upload.
- **FR-UP-003:** Multipart part size MUST respect S3 limits and provider capability settings.
- **FR-UP-004:** Uploads MUST support configurable destination collision policy.
- **FR-UP-005:** Upload cancellation MUST abort incomplete multipart upload when possible.
- **FR-UP-006:** Upload folder traversal MUST not follow unsafe filesystem links by default.
- **FR-UP-007:** The application MUST not build the entire upload body in memory.
- **FR-UP-008:** Metadata and content type SHOULD be inferred and MAY be edited before upload.
- **FR-UP-009:** Transfer progress MUST be rate-limited before delivery to the frontend.
- **FR-UP-010:** Completed upload SHOULD be validated using provider response metadata and optional checksum policy.

### 9.4 Download requirements

- **FR-DL-001:** Downloads MUST stream to local disk.
- **FR-DL-002:** Incomplete downloads MUST use a distinguishable temporary filename.
- **FR-DL-003:** A completed download MUST only be exposed under the final filename after success.
- **FR-DL-004:** Object keys MUST be sanitized into safe local paths without changing remote keys.
- **FR-DL-005:** Path traversal, reserved Windows names, and invalid local characters MUST be handled explicitly.
- **FR-DL-006:** Collision policy MUST support Ask, Skip, Overwrite, and Rename.
- **FR-DL-007:** Available disk space SHOULD be checked before known-size downloads.
- **FR-DL-008:** Session resume MAY use range requests after validating the remote object identity.
- **FR-DL-009:** The user MUST be able to open the destination folder after completion.

### 9.5 Copy, move, and rename requirements

- **FR-OP-001:** Rename and move MUST be modeled as compound operations unless a provider offers a verified atomic primitive.
- **FR-OP-002:** The destination MUST be verified before source deletion.
- **FR-OP-003:** Objects larger than the single-copy service limit MUST use multipart copy or another safe strategy.
- **FR-OP-004:** Recursive prefix operations MUST produce item-level progress.
- **FR-OP-005:** Recursive operations MUST retain a partial-failure list.
- **FR-OP-006:** The system MUST distinguish CopyCompleted/DeleteFailed from full success.
- **FR-OP-007:** Moving a folder marker alone MUST NOT imply children were moved.
- **FR-OP-008:** Cross-profile copy MUST be rejected in the MVP with a clear unsupported-feature message.

### 9.6 Delete requirements

- **FR-DEL-001:** Delete operations MUST require confirmation.
- **FR-DEL-002:** Large recursive deletes MUST display estimated object count and known total size.
- **FR-DEL-003:** High-risk deletes SHOULD require typed confirmation above a configurable threshold.
- **FR-DEL-004:** The UI MUST disclose that deletion behavior depends on bucket versioning and retention policies.
- **FR-DEL-005:** Object Lock or retention failures MUST be reported per object.
- **FR-DEL-006:** Batch deletion MUST respect provider request limits.

### 9.7 Preview and share requirements

- **FR-PRV-001:** Preview MUST never expose permanent credentials.
- **FR-PRV-002:** Preview MAY use a short-lived presigned URL when provider capability and object type allow it.
- **FR-PRV-003:** Presigned URLs MUST be treated as temporary bearer secrets and MUST NOT be logged.
- **FR-PRV-004:** A local temporary-cache fallback SHOULD be available for providers without reliable presigning.
- **FR-PRV-005:** Preview cache MUST have size and age limits.
- **FR-PRV-006:** Share links MUST have a user-selected expiry bounded by policy.

### 9.8 Transfer requirements

- **FR-TRN-001:** Every long-running operation MUST execute through the transfer manager.
- **FR-TRN-002:** Commands MUST return a transfer ID promptly instead of blocking until completion.
- **FR-TRN-003:** Transfer state changes MUST be serialized through an internal state machine.
- **FR-TRN-004:** Retry MUST occur only for classified retryable errors.
- **FR-TRN-005:** Users MUST be able to cancel queued and running jobs.
- **FR-TRN-006:** UI progress MUST include job state, bytes, speed, and item counts where available.
- **FR-TRN-007:** Transfer history MUST omit secrets and presigned URLs.
- **FR-TRN-008:** The application MUST limit concurrent jobs and per-job part concurrency.
- **FR-TRN-009:** Shutdown MUST prompt when transfers are active.
- **FR-TRN-010:** Crash recovery MUST mark interrupted jobs accurately; automatic restart resume is deferred.

### 9.9 Settings requirements

- **FR-SET-001:** Global settings MUST include concurrency, multipart threshold, default part size, retry limits, and overwrite behavior.
- **FR-SET-002:** Dangerous settings MUST be separated under an Advanced section.
- **FR-SET-003:** TLS verification MUST NOT be disableable in the MVP.
- **FR-SET-004:** Plain HTTP endpoints MAY be allowed only after an explicit warning and per-profile opt-in.
- **FR-SET-005:** Settings changes affecting active transfers MUST apply only to new jobs unless explicitly safe.

---

### 9.10 Completed MVP requirements

The following requirements close the remaining behavior and implementation gaps identified during the baseline review.

#### Profile lifecycle and provider presets

- **FR-PRO-011:** The user MUST be able to duplicate a profile; the duplicate MUST receive a new UUID, a display name suffixed with `Copy`, and no active client-cache entry.
- **FR-PRO-012:** The user MUST be able to mark or unmark a profile as favorite through `update_profile`; favorite ordering MUST be deterministic.
- **FR-PRO-013:** Saving a profile MUST use a compensating transaction across SQLite and the credential store so that partial success cannot leave an unusable profile without a visible recovery state.
- **FR-PRO-014:** Replacing credentials MUST write the new credential entry first, commit the new reference in SQLite second, and delete the old credential entry last.
- **FR-PRO-015:** A profile edit that changes endpoint, region, addressing mode, root prefix, bucket, or credentials while jobs are active MUST be rejected until affected jobs finish or are cancelled.
- **FR-PRO-016:** Root prefixes MUST be normalized to either an empty string or a non-empty value ending in `/`; `..`, backslash traversal, control characters, and leading `/` MUST be rejected.
- **FR-PRO-017:** The default bucket MUST be validated with `HeadBucket` during a successful connection test when supplied.
- **FR-PRO-018:** A temporary session token MUST be stored as a secret, MUST never be displayed after save, and MUST produce a distinct `CREDENTIAL_EXPIRED` error when rejected as expired.
- **FR-PRO-019:** Provider preset defaults MUST match Section 43 and MUST remain editable only where the preset allows editing.
- **FR-PRO-020:** Importing profiles MUST create new local UUIDs unless the user explicitly chooses an overwrite of a matching exported profile ID.
- **FR-PRO-021:** Exported profiles MUST include a schema version and MUST contain no secret reference that can be used outside the current installation.

#### Explorer interaction

- **FR-EXP-011:** List view MUST display Name, Type, Size, Last Modified, and Storage Class where available.
- **FR-EXP-012:** Grid view MUST display name, type icon or safe thumbnail, and size for objects; folders MUST use a folder tile.
- **FR-EXP-013:** Selection MUST be keyed by exact entry identity `(profile_id, bucket, key, kind)` rather than display name.
- **FR-EXP-014:** `Ctrl+A` MUST select all currently loaded entries only; the UI MUST disclose that unloaded pages are not selected.
- **FR-EXP-015:** Shift-range selection MUST operate on the current sorted loaded result set.
- **FR-EXP-016:** Selection MUST be cleared when profile, bucket, or prefix changes, except when an operation explicitly preserves it.
- **FR-EXP-017:** Double-clicking a prefix MUST navigate into it; double-clicking a supported file MUST open preview; unsupported files MUST open Properties.
- **FR-EXP-018:** Enter MUST activate the focused entry and Backspace MUST navigate up without crossing the root prefix.
- **FR-EXP-019:** F2 MUST start rename for exactly one selected entry and Delete MUST open a confirmation dialog.
- **FR-EXP-020:** Context menus MUST be generated from entry type, selection count, provider capabilities, and active-operation constraints.
- **FR-EXP-021:** Sorting MUST apply only to loaded entries in the MVP and the UI MUST show a `Loaded results` indicator while pagination remains incomplete.
- **FR-EXP-022:** Current-folder filtering MUST be case-insensitive by default, MUST not trigger remote requests, and MUST preserve the unfiltered selection model.
- **FR-EXP-023:** Back and forward history MUST retain at most 100 locations per profile and MUST discard forward history after a new branch navigation.
- **FR-EXP-024:** Recent locations MUST retain at most 30 unique locations per profile, ordered by last visit.
- **FR-EXP-025:** Bookmarks MUST be independent from profile favorites and MUST store profile, bucket, prefix, custom name, and sort order.
- **FR-EXP-026:** A listing request MUST expose loading, empty, error, cancelled, partial-page, and complete states.
- **FR-EXP-027:** Navigating while a listing is in progress MUST cancel the previous request and ignore any late result using a request-generation token.
- **FR-EXP-028:** Dragging selected remote entries to another prefix in the same profile MUST initiate copy by default; holding Shift MAY request move after confirmation.

#### Upload and local traversal

- **FR-UP-011:** Folder upload MUST preserve the selected folder name by default and MUST offer an `Upload contents only` option.
- **FR-UP-012:** Empty local directories MUST create folder markers only when `preserve_empty_folders` is enabled; the MVP default is enabled.
- **FR-UP-013:** Local symlinks, junctions, mount points, and Windows reparse points MUST not be followed by default.
- **FR-UP-014:** Hidden and system files MUST be included by default and MAY be excluded through a per-job option.
- **FR-UP-015:** The upload plan MUST capture source path, initial size, initial modified time, destination key, and collision policy before transfer.
- **FR-UP-016:** If a local file changes before its upload starts, the job MUST re-stat it and update the plan; if it changes during upload, the item MUST fail with `LOCAL_FILE_CHANGED` unless a stable snapshot is available.
- **FR-UP-017:** Upload key construction MUST use `/` independent of the local operating-system separator.
- **FR-UP-018:** Upload collision detection MUST use `HeadObject` only when policy requires existence knowledge and MUST avoid one request per item when provider-side listing can safely supply it.
- **FR-UP-019:** `Apply to all` collision choices MUST apply only to the current transfer job.
- **FR-UP-020:** Upload metadata editing in the MVP MUST include Content-Type, Content-Disposition, Cache-Control, and user metadata; object tags are read-only in the MVP.
- **FR-UP-021:** Single-request upload MUST be used only when object size is below the configured threshold and the provider supports the request size.

#### Download path mapping

- **FR-DL-010:** Folder download MUST map each remote key relative to the selected source prefix, never relative to the bucket root.
- **FR-DL-011:** Empty prefixes represented only by folder markers MUST create local directories when `preserve_empty_folders` is enabled.
- **FR-DL-012:** Windows-invalid characters `< > : " / \\ | ? *` and ASCII control characters MUST be encoded using the reversible `_s3xHH_` byte-escape format described in Section 47.
- **FR-DL-013:** Windows reserved base names such as `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, and `LPT1`–`LPT9` MUST be prefixed with `_s3r_`.
- **FR-DL-014:** Trailing spaces and periods in a path segment MUST be escaped rather than trimmed.
- **FR-DL-015:** Two remote keys that map to the same case-insensitive Windows path MUST be treated as a collision and MUST never overwrite silently.
- **FR-DL-016:** The application MUST write an optional `.s3-key-map.json` manifest for any download whose local names were encoded; the default is enabled for recursive downloads.
- **FR-DL-017:** Local output paths longer than the configured safety limit MUST fail per item with `LOCAL_PATH_TOO_LONG`; automatic flattening is not permitted.
- **FR-DL-018:** A download resume MUST require matching bucket, key, size, and ETag or equivalent version identity.
- **FR-DL-019:** If remote identity cannot be established, Resume MUST be disabled and Restart MUST be offered.
- **FR-DL-020:** `.partial` files MUST be created in the final destination directory to maximize atomic rename compatibility.
- **FR-DL-021:** Cancelling a download MUST retain the partial file only when the user selected `Keep partial files`; the MVP default is delete.

#### Object operations and delete

- **FR-OP-009:** A single-object copy MUST preserve Content-Type, Content-Disposition, Cache-Control, Content-Encoding, Content-Language, Expires, and user metadata by default.
- **FR-OP-010:** The user MUST be able to choose Replace metadata for a copy; unchanged metadata is the default.
- **FR-OP-011:** Server-side copy MUST be limited to source and destination accessible by the same profile and compatible endpoint.
- **FR-OP-012:** Recursive copy/move plans MUST exclude the destination prefix from source enumeration when destination is nested under source.
- **FR-OP-013:** Moving a prefix into itself or one of its descendants MUST be rejected.
- **FR-OP-014:** Rename input MUST be a single path segment for a file and a single folder-name segment for a prefix; separators MUST be rejected.
- **FR-OP-015:** Verification after copy MUST compare key, known size, and provider response identity; checksum comparison MUST be used when both sides expose a compatible checksum.
- **FR-OP-016:** A failed delete after verified copy MUST leave the destination intact and mark the item `CleanupRequired`.
- **FR-OP-017:** A retry of `CleanupRequired` MUST retry only source deletion unless the destination no longer verifies.
- **FR-DEL-007:** Deleting more than 100 objects or more than 10 GiB of known data MUST require typed confirmation of the final prefix or object count.
- **FR-DEL-008:** Delete planning MUST identify folder markers separately from child objects.
- **FR-DEL-009:** Delete requests MUST be chunked to the provider batch limit and MUST record per-object errors.
- **FR-DEL-010:** The application MUST never state that data is recoverable unless versioning capability and object version state were verified.
- **FR-DEL-011:** Delete of a profile root prefix MUST be rejected unless the user selects its children explicitly and completes typed confirmation.

#### Metadata, preview, and sharing

- **FR-MET-001:** Properties MUST display profile, bucket, exact key, type, size, last modified, ETag, version ID, storage class, Content-Type, Content-Disposition, Cache-Control, Content-Encoding, checksum fields, encryption summary, and user metadata when available.
- **FR-MET-002:** Missing provider fields MUST be displayed as `Unknown` rather than inferred.
- **FR-MET-003:** The MVP MUST allow editing Content-Type, Content-Disposition, Cache-Control, and user metadata through a metadata-replace copy operation.
- **FR-MET-004:** Metadata edit MUST warn that S3 commonly implements metadata replacement by copying the object onto itself and is not atomic.
- **FR-MET-005:** Object tags, ACLs, retention, legal hold, storage class, and encryption settings MUST be read-only or hidden in the MVP.
- **FR-PRV-007:** The MVP preview allowlist MUST include JPEG, PNG, GIF, WebP, BMP, plain text, JSON, XML, Markdown, PDF, MP3, WAV, OGG, MP4, WebM, and common UTF-8 log/config formats.
- **FR-PRV-008:** HTML, SVG, scripts, executable content, and unknown active content MUST never be rendered as privileged webview content.
- **FR-PRV-009:** Text preview MUST load at most 2 MiB and MUST indicate truncation.
- **FR-PRV-010:** Image preview MUST reject decoded dimensions above 100 megapixels unless the decoder supports safe downsampling.
- **FR-PRV-011:** Audio/video preview MUST use a presigned URL where supported; otherwise it MUST use a bounded local cache.
- **FR-PRV-012:** PDF preview MUST use the platform PDF viewer or a sandboxed bundled viewer without remote script execution.
- **FR-PRV-013:** Preview handles MUST expire after 15 minutes and MUST be revocable when the preview window closes.
- **FR-PRV-014:** Preview cache MUST default to 512 MiB and 24-hour maximum age with LRU eviction.
- **FR-PRV-015:** Share-link expiry MUST be selectable from 5 minutes, 15 minutes, 1 hour, 6 hours, 24 hours, and 7 days; default 1 hour; maximum 7 days.
- **FR-PRV-016:** Share links MUST be copied only after an explicit user action and MUST be omitted from transfer history and diagnostics.
- **FR-PRV-017:** Providers that do not support reliable presigning MUST disable Share Link with a capability explanation.

#### Transfer semantics and settings

- **FR-TRN-011:** Multipart upload pause MUST stop scheduling new parts and allow in-flight parts to finish before entering Paused.
- **FR-TRN-012:** Single-request upload MUST not support pause; the UI MUST offer cancel and restart.
- **FR-TRN-013:** Download pause MUST stop reading new response bytes; resume MUST use a validated Range request.
- **FR-TRN-014:** Server-side copy of one object MUST not support pause once submitted.
- **FR-TRN-015:** Recursive copy, move, and delete pause MUST stop planning and scheduling new items while allowing in-flight requests to settle.
- **FR-TRN-016:** Queue order MUST be FIFO within priority, with user-initiated foreground jobs above background planning jobs.
- **FR-TRN-017:** A job MUST snapshot relevant settings at creation and MUST not change behavior when global settings are edited.
- **FR-TRN-018:** Speed MUST use an exponentially weighted moving average over approximately five seconds.
- **FR-TRN-019:** ETA MUST be hidden when total bytes are unknown or speed confidence is insufficient.
- **FR-TRN-020:** Transfer history MUST retain completed jobs for 30 days or the latest 1,000 jobs, whichever bound is reached first.
- **FR-TRN-021:** Failed item detail MUST be retained for the same period as its parent job.
- **FR-TRN-022:** Clearing history MUST not remove active jobs or multipart cleanup records.
- **FR-SET-006:** Default concurrent jobs MUST be 4 with allowed range 1–16.
- **FR-SET-007:** Default per-job multipart concurrency MUST be 4 with allowed range 1–16.
- **FR-SET-008:** Default multipart threshold MUST be 64 MiB with allowed range 16 MiB–5 GiB.
- **FR-SET-009:** Default initial part size MUST be 16 MiB with allowed range 5 MiB–5 GiB; the planner MUST increase it when needed to remain within the provider part-count limit.
- **FR-SET-010:** Default retry limit MUST be 5 attempts with allowed range 0–10.
- **FR-SET-011:** Retry base delay MUST default to 500 ms and maximum delay to 30 seconds.
- **FR-SET-012:** Progress update frequency MUST default to 5 Hz with allowed range 1–10 Hz.
- **FR-SET-013:** Preview cache quota MUST default to 512 MiB with allowed range 64 MiB–10 GiB.
- **FR-SET-014:** Log retention MUST default to 14 days and 100 MiB total with allowed retention 1–90 days.
- **FR-SET-015:** Recursive typed-confirmation threshold MUST default to 100 objects or 10 GiB.
- **FR-SET-016:** Default collision policy MUST be Ask.
- **FR-SET-017:** Settings validation MUST reject values outside documented ranges and MUST return field-level errors.
- **FR-SET-018:** Reset Settings MUST restore documented defaults without deleting profiles, secrets, bookmarks, or history.

#### Packaging and release

- **FR-PKG-001:** The MVP product name MUST be `S3 File Manager` and the application identifier MUST be `com.s3filemanager.desktop` until an owned distribution domain replaces it.
- **FR-PKG-002:** The minimum supported Windows release MUST be Windows 10 version 2004, build 19041, x64.
- **FR-PKG-003:** The primary installer MUST be NSIS per-user and MUST not require administrator rights for the normal installation path.
- **FR-PKG-004:** The installer MUST use the WebView2 bootstrapper download mode and MUST fail with actionable guidance when runtime installation cannot complete.
- **FR-PKG-005:** Release builds MUST be compiled using the MSVC Windows target on a Windows CI runner.
- **FR-PKG-006:** Production application binaries and installers MUST be Authenticode-signed when a certificate is configured.
- **FR-PKG-007:** Updater artifacts MUST be signed with a key distinct from Authenticode signing and verified with an embedded public key.
- **FR-PKG-008:** The MVP MUST use a static signed update manifest with `stable` and `beta` channels hosted under separately configured HTTPS endpoints.
- **FR-PKG-009:** Automatic update checks MUST default to enabled once every 24 hours; automatic installation MUST be disabled and require user confirmation.
- **FR-PKG-010:** The updater MUST not install while transfers are active and MUST offer retry after transfers finish.

## 10. Non-functional requirements

### 10.1 Performance

- **NFR-PERF-001:** UI interactions SHOULD remain responsive under active transfers.
- **NFR-PERF-002:** No file operation may read an entire large object into memory.
- **NFR-PERF-003:** Listing MUST be incremental and paginated.
- **NFR-PERF-004:** Progress updates SHOULD be emitted no more than 4–10 times per second per visible job.
- **NFR-PERF-005:** The transfer manager MUST use bounded queues and semaphores.
- **NFR-PERF-006:** The application SHOULD support at least four concurrent file transfers on a typical desktop without UI degradation.
- **NFR-PERF-007:** SQLite work MUST not run synchronously on the UI thread.

### 10.2 Reliability

- **NFR-REL-001:** Retry policy MUST use exponential backoff and jitter.
- **NFR-REL-002:** Non-idempotent or compound operations MUST persist enough state to avoid reporting false success.
- **NFR-REL-003:** Download finalization MUST be crash-safe to the degree supported by the local filesystem.
- **NFR-REL-004:** Multipart uploads that are abandoned SHOULD be discoverable and abortable.
- **NFR-REL-005:** Database migrations MUST be transactional.
- **NFR-REL-006:** The application MUST tolerate provider responses that omit optional fields.

### 10.3 Security

- **NFR-SEC-001:** Permanent credentials MUST remain in Rust-controlled secret storage.
- **NFR-SEC-002:** Credentials MUST NOT be stored in SQLite, JSON export, localStorage, sessionStorage, logs, crash reports, or React state.
- **NFR-SEC-003:** Tauri capabilities MUST grant the minimum required plugin and core permissions.
- **NFR-SEC-004:** Main webview MUST load bundled local application content only.
- **NFR-SEC-005:** All IPC request input MUST be validated in Rust.
- **NFR-SEC-006:** File paths MUST be canonicalized and checked against user-selected scope.
- **NFR-SEC-007:** Diagnostic exports MUST pass secret-redaction tests.
- **NFR-SEC-008:** Update packages MUST be cryptographically verified.
- **NFR-SEC-009:** Windows releases SHOULD be Authenticode signed.
- **NFR-SEC-010:** Dependencies MUST be monitored for known vulnerabilities.

### 10.4 Usability and accessibility

- **NFR-UX-001:** Common Explorer keyboard behavior SHOULD be supported.
- **NFR-UX-002:** Destructive operations MUST be visually distinct.
- **NFR-UX-003:** Error messages MUST explain what happened, likely cause, and possible action.
- **NFR-UX-004:** UI controls SHOULD meet keyboard and screen-reader accessibility requirements.
- **NFR-UX-005:** Long object names MUST remain inspectable without breaking layout.
- **NFR-UX-006:** Byte sizes, timestamps, and durations MUST be localized consistently.

### 10.5 Compatibility

- **NFR-COMP-001:** AWS S3 general-purpose buckets are the reference behavior.
- **NFR-COMP-002:** Provider-specific behavior MUST be represented by a capability model.
- **NFR-COMP-003:** The application MUST NOT assume every compatible provider implements every AWS feature.
- **NFR-COMP-004:** The MVP MUST pass smoke tests against AWS S3, Cloudflare R2, and MinIO.

### 10.6 Maintainability

- **NFR-MNT-001:** Domain logic MUST be independent of Tauri command wrappers.
- **NFR-MNT-002:** Provider-specific exceptions MUST not spread through UI components.
- **NFR-MNT-003:** Public IPC types MUST be versionable and serializable.
- **NFR-MNT-004:** Core state transitions and key normalization MUST have unit tests.

---

## 11. Technology stack and rationale

| Area | Selection | Rationale |
|---|---|---|
| Desktop | Tauri 2 | Lightweight system webview, Rust backend, capabilities, installer and updater ecosystem |
| Frontend | React + TypeScript + Vite | Mature component ecosystem and strict typed UI development |
| UI | Tailwind CSS + shadcn/ui primitives | Fast consistent UI with controllable source components |
| Backend | Rust | Memory safety, strong concurrency model, native filesystem/network access |
| Async runtime | Tokio | Standard async runtime used by AWS SDK and transfer workers |
| S3 | AWS SDK for Rust | First-party AWS service client, async APIs, credentials, presigning, pagination |
| Database | SQLite + SQLx | Embedded transactional local storage and compile-time-oriented query workflow |
| Frontend state | Zustand | Small predictable client state layer |
| Serialization | Serde | Rust/JSON IPC model support |
| Logging | tracing + Tauri log integration | Structured spans and local diagnostics |
| Error types | thiserror + domain mapping | Internal typed errors and stable public error codes |
| Secrets | CredentialStore abstraction; Windows Credential Manager implementation | OS-bound security and good user experience |
| Installer | NSIS | Simple Windows setup executable and per-user install support |

### 11.1 Rejected alternatives

#### Electron

Rejected for the baseline because it bundles Chromium/Node and would require a separate Rust or Node backend boundary. It remains viable if Chromium consistency becomes more important than footprint.

#### Wails

Rejected after selecting Rust as the backend language. Wails is well suited to Go, but Tauri aligns directly with Rust.

#### WinUI 3

Rejected because it is Windows-specific and would move the UI stack away from React and cross-platform architecture.

#### Stronghold as mandatory MVP store

Not selected as the default because a mandatory master-password flow increases UX and recovery complexity. The secret-store interface is designed so an optional Stronghold portable vault can be added later.

---

## 12. System context

```text
┌──────────────────────┐
│ User                 │
└──────────┬───────────┘
           │ desktop interaction
┌──────────▼─────────────────────────────────────┐
│ S3 File Manager Desktop                       │
│ React UI + Tauri IPC + Rust application core  │
└───────┬──────────────┬──────────────┬──────────┘
        │              │              │
        │              │              └──────────────┐
        │              │                             │
┌───────▼──────┐ ┌─────▼─────────┐          ┌────────▼────────┐
│ Local files  │ │ Local stores  │          │ S3-compatible   │
│ and folders  │ │ SQLite/secret │          │ remote service  │
└──────────────┘ └───────────────┘          └─────────────────┘
```

### 12.1 Trust boundaries

1. **Frontend boundary:** React code is treated as less privileged than Rust.
2. **IPC boundary:** Every command input is untrusted and validated.
3. **Local filesystem boundary:** User-selected paths are authorized narrowly.
4. **Credential boundary:** Secret values exist only inside credential service and SDK credential providers.
5. **Remote provider boundary:** Responses and metadata are untrusted external input.
6. **Update boundary:** Update metadata and artifacts require signature verification.

---

## 13. High-level architecture

```text
┌────────────────────────────────────────────────────────────┐
│ Presentation Layer                                         │
│ React, TypeScript, routing, components, Zustand stores     │
└──────────────────────────┬─────────────────────────────────┘
                           │ Tauri commands / channels
┌──────────────────────────▼─────────────────────────────────┐
│ IPC Adapter Layer                                          │
│ Command validation, DTO mapping, channel lifecycle         │
└──────────────────────────┬─────────────────────────────────┘
                           │
┌──────────────────────────▼─────────────────────────────────┐
│ Application Services                                       │
│ Profile, Explorer, Transfer, Preview, Settings, Diagnostics│
└──────────────────────────┬─────────────────────────────────┘
                           │
┌──────────────────────────▼─────────────────────────────────┐
│ Domain Layer                                                │
│ Locations, keys, transfer state, policies, capability model│
└──────────────┬───────────────────┬─────────────────────────┘
               │                   │
┌──────────────▼─────────┐ ┌───────▼─────────────────────────┐
│ Local Infrastructure  │ │ Remote Infrastructure            │
│ SQLite, credential    │ │ AWS SDK S3 clients, presigning,  │
│ store, filesystem     │ │ provider adapters                │
└────────────────────────┘ └──────────────────────────────────┘
```

### 13.1 Architectural principles

- Privileged operations stay in Rust.
- Commands are thin adapters, not business-logic containers.
- Long operations become jobs managed by the transfer subsystem.
- Provider differences are represented explicitly.
- Object storage semantics are not hidden behind false filesystem guarantees.
- Every compound operation has observable intermediate and partial-failure states.
- Secrets are referenced, never copied through DTOs.

---

## 14. Frontend architecture

### 14.1 Frontend modules

```text
src/
├── app/
│   ├── App.tsx
│   ├── router.tsx
│   └── providers.tsx
├── pages/
│   ├── ProfilesPage.tsx
│   ├── ExplorerPage.tsx
│   ├── TransfersPage.tsx
│   └── SettingsPage.tsx
├── features/
│   ├── profiles/
│   ├── explorer/
│   ├── transfers/
│   ├── preview/
│   └── diagnostics/
├── components/
│   ├── layout/
│   ├── ui/
│   └── feedback/
├── stores/
│   ├── profileStore.ts
│   ├── explorerStore.ts
│   ├── transferStore.ts
│   └── settingsStore.ts
├── ipc/
│   ├── commands.ts
│   ├── channels.ts
│   └── types.ts
└── utils/
```

### 14.2 Frontend responsibilities

The frontend is responsible for:

- Rendering application state
- Collecting user intent
- Calling typed command wrappers
- Displaying progress and errors
- Maintaining navigation and view state
- Performing non-security-sensitive filtering and sorting
- Accessibility and keyboard interaction

The frontend is not responsible for:

- Reading local file contents for transfer
- Holding credentials
- Constructing signed S3 requests
- Authorizing filesystem paths
- Implementing retry logic
- Determining final operation success

### 14.3 Main window layout

```text
┌──────────────────────────────────────────────────────────────┐
│ Profile selector | Address/Breadcrumb | Search/Filter | Menu │
├───────────────┬──────────────────────────────────────────────┤
│ Buckets       │ Toolbar                                      │
│ Favorites     ├──────────────────────────────────────────────┤
│ Recent        │ File list / grid                             │
│               │                                              │
│               │                                              │
├───────────────┴──────────────────────────────────────────────┤
│ Transfer drawer / status bar                                │
└──────────────────────────────────────────────────────────────┘
```

### 14.4 State ownership

| State | Owner |
|---|---|
| Profile list metadata | Rust repository; mirrored in profile store |
| Current location | Explorer store |
| Current listing | Explorer store; disposable cache |
| Transfer authoritative state | Rust transfer manager |
| Transfer visible projection | Transfer store |
| Secrets | Rust credential store only |
| Settings | Rust repository; mirrored in settings store |
| Dialog/form temporary values | React local state |

### 14.5 Navigation behavior

- Opening a profile creates a new navigation context.
- History entry contains profile ID, bucket, and prefix.
- Root-prefix profiles cannot navigate above the configured root.
- Navigating cancels the previous list request using a request/cancellation token.
- Refresh preserves selection only when exact keys remain present.
- Pagination may use Load More or virtualized incremental loading.

---

## 15. Rust backend architecture

### 15.1 Proposed module layout

```text
src-tauri/src/
├── lib.rs
├── app_state.rs
├── commands/
│   ├── profiles.rs
│   ├── explorer.rs
│   ├── transfers.rs
│   ├── preview.rs
│   ├── settings.rs
│   └── diagnostics.rs
├── application/
│   ├── profile_service.rs
│   ├── explorer_service.rs
│   ├── transfer_service.rs
│   ├── preview_service.rs
│   └── diagnostic_service.rs
├── domain/
│   ├── profile.rs
│   ├── provider.rs
│   ├── location.rs
│   ├── object_key.rs
│   ├── transfer.rs
│   ├── collision.rs
│   └── error.rs
├── infrastructure/
│   ├── database/
│   ├── credentials/
│   ├── filesystem/
│   ├── logging/
│   └── s3/
├── transfer/
│   ├── manager.rs
│   ├── scheduler.rs
│   ├── progress.rs
│   ├── upload.rs
│   ├── download.rs
│   ├── copy.rs
│   ├── move_job.rs
│   └── recursive.rs
└── dto/
```

### 15.2 AppState

```rust
pub struct AppState {
    pub profiles: Arc<ProfileService>,
    pub explorer: Arc<ExplorerService>,
    pub transfers: Arc<TransferManager>,
    pub previews: Arc<PreviewService>,
    pub settings: Arc<SettingsService>,
    pub clients: Arc<S3ClientManager>,
}
```

All shared services MUST be thread-safe. Lock scope MUST be kept short and MUST NOT include awaited network calls.

### 15.3 Command design rules

- Commands validate DTO shape and call application services.
- Commands return stable serializable DTOs.
- Commands do not expose SDK client objects or provider errors.
- Commands that initiate long operations return a `TransferId`.
- Ordered progress uses Tauri channels.
- Low-frequency global state changes may use events.

---

## 16. Domain model

### 16.1 Connection profile

```rust
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    pub provider: ProviderType,
    pub endpoint: Option<Url>,
    pub region: String,
    pub credential_mode: CredentialMode,
    pub access_key_id: Option<String>,
    pub secret_reference: Option<SecretReference>,
    pub session_reference: Option<SecretReference>,
    pub default_bucket: Option<String>,
    pub root_prefix: Option<ObjectPrefix>,
    pub addressing_style: AddressingStyle,
    pub allow_insecure_http: bool,
    pub favorite: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### 16.2 Provider type

```rust
pub enum ProviderType {
    AwsS3,
    CloudflareR2,
    Minio,
    Wasabi,
    CustomS3,
}
```

### 16.3 Credential mode

MVP:

```rust
pub enum CredentialMode {
    Static,
    TemporarySession,
}
```

Future:

```rust
pub enum FutureCredentialMode {
    AwsSharedProfile,
    DefaultCredentialChain,
    AssumeRole,
    IamIdentityCenter,
    Anonymous,
}
```

### 16.4 Provider capabilities

```rust
pub struct ProviderCapabilities {
    pub list_buckets: CapabilityState,
    pub multipart_upload: CapabilityState,
    pub multipart_copy: CapabilityState,
    pub presigned_get: CapabilityState,
    pub presigned_put: CapabilityState,
    pub versioning: CapabilityState,
    pub object_lock: CapabilityState,
    pub checksums: CapabilityState,
    pub storage_classes: CapabilityState,
    pub tagging: CapabilityState,
}

pub enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
}
```

Capabilities derive from:

1. Provider preset defaults
2. Connection-test observations
3. Operation errors
4. User or support overrides where safe

### 16.5 Explorer location

```rust
pub struct ExplorerLocation {
    pub profile_id: Uuid,
    pub bucket: String,
    pub prefix: ObjectPrefix,
}
```

### 16.6 Object entry

```rust
pub enum EntryKind {
    Prefix,
    Object,
    FolderMarker,
}

pub struct ObjectEntry {
    pub kind: EntryKind,
    pub key: String,
    pub display_name: String,
    pub size: Option<u64>,
    pub last_modified: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
    pub content_type: Option<String>,
}
```

### 16.7 Key and prefix invariants

- Remote keys are opaque UTF-8/byte-oriented identifiers as represented by the SDK.
- The application MUST NOT apply local path normalization to remote keys.
- A prefix displayed as a folder normally ends with `/` internally.
- Empty path segments and repeated slashes MUST be preserved unless the user explicitly requests normalization.
- Root prefix constraints MUST be checked using canonical object-prefix logic, not substring matching.

---

## 17. Connection profile and credential design

### 17.1 Profile persistence split

SQLite stores:

- Profile ID and name
- Provider type
- Endpoint
- Region
- Access key ID when applicable
- Secret references
- Bucket and root prefix
- Addressing style
- Insecure HTTP opt-in
- Favorite and recent-use metadata

Credential store stores:

- Secret access key
- Session token
- Future refresh tokens or role cache secrets

### 17.2 CredentialStore interface

```rust
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn put(&self, key: &SecretReference, secret: SecretString) -> Result<()>;
    async fn get(&self, key: &SecretReference) -> Result<Option<SecretString>>;
    async fn delete(&self, key: &SecretReference) -> Result<()>;
}
```

### 17.3 MVP implementation

- Windows implementation uses Windows Credential Manager through a Rust credential-store adapter.
- Secret target names use an application namespace and profile UUID.
- Secrets are copied into SDK credential values only for client construction or refresh.
- Secret values use redaction-aware wrappers where practical.

Example secret references:

```text
s3fm/profile/<uuid>/secret-access-key
s3fm/profile/<uuid>/session-token
```

### 17.4 Future implementations

- macOS Keychain
- Linux Secret Service
- Optional Stronghold portable vault protected by a master password

### 17.5 Profile test algorithm

1. Validate profile fields locally.
2. Resolve credential references.
3. Build a non-cached temporary S3 client.
4. Attempt `ListBuckets` if no default bucket restriction exists.
5. If listing is denied or unsupported, attempt `HeadBucket` on the configured bucket.
6. If root prefix exists, optionally perform a one-entry `ListObjectsV2` probe.
7. Record latency, provider request ID, and observed capabilities.
8. Map failure to a stable public error.
9. Destroy temporary secret material as soon as practical.

### 17.6 Profile deletion transaction

Profile deletion is a compensating operation:

1. Block new jobs using the profile.
2. Warn if active jobs exist.
3. Delete credentials from the credential store.
4. Delete or tombstone profile metadata in SQLite.
5. Remove cached S3 client.
6. Remove recent locations and bookmarks or mark them orphaned.

If credential deletion fails, profile metadata MUST NOT silently disappear. The UI must offer retry or force metadata removal with a warning.

---

## 18. S3 client manager

### 18.1 Responsibilities

- Construct clients from profiles and credential providers
- Cache clients by profile ID and configuration revision
- Invalidate clients when connection-affecting profile fields change
- Track last-used time
- Prevent secrets from being logged
- Create separate temporary clients for profile testing
- Support provider endpoint and path-style configuration

### 18.2 Cache key

```text
(profile_id, profile_revision, credential_revision)
```

### 18.3 Cache behavior

- Cache entries use `Arc<Client>`.
- Idle entries MAY be removed after a configurable period.
- Profile update immediately invalidates the previous entry.
- Temporary session credential expiration is included in credential-provider behavior.
- Network failure does not automatically destroy a valid client.

### 18.4 Endpoint validation

- AWS preset normally uses SDK endpoint resolution.
- Custom endpoint must parse as an absolute URL.
- HTTPS is required by default.
- HTTP requires explicit `allow_insecure_http` and a blocking warning.
- Embedded credentials in endpoint URLs are forbidden.
- Query strings and fragments in endpoint URLs are forbidden.
- TLS certificate verification cannot be disabled in the MVP.

---

## 19. Explorer and listing design

### 19.1 Listing request

```rust
pub struct ListEntriesRequest {
    pub location: ExplorerLocation,
    pub continuation_token: Option<String>,
    pub page_size: u32,
    pub include_metadata: bool,
}
```

The S3 request uses:

- Bucket
- Prefix
- Delimiter `/`
- Continuation token
- Max keys up to the provider-supported limit

### 19.2 Listing response

```rust
pub struct ListEntriesPage {
    pub location: ExplorerLocation,
    pub entries: Vec<ObjectEntry>,
    pub next_token: Option<String>,
    pub is_truncated: bool,
    pub request_id: Option<String>,
}
```

### 19.3 Folder markers

A zero-byte object ending in `/` may appear alongside a `CommonPrefixes` entry. The UI should normally display one folder entry, while retaining marker metadata internally for operations.

### 19.4 Pagination and sorting

S3 returns at most a bounded page. Therefore:

- The UI MUST not claim globally sorted results unless all pages are loaded.
- Default sort can follow service order.
- User sort applies to loaded results in the MVP.
- The UI should show `Loaded N items` when a continuation token remains.

### 19.5 Metadata loading

List results do not contain every object property. Expensive properties such as full metadata, tags, versioning data, or content type may require additional calls.

The UI should load details lazily when the user opens the Properties panel.

---

## 20. File and folder operation design

### 20.1 Create folder

Creating a folder writes a zero-byte object key ending in `/`.

Rules:

- Reject empty folder name.
- Preserve user-intended Unicode.
- Disallow navigation separators that would create unintended nested paths unless confirmed.
- Apply root-prefix constraints.
- Detect existing object/prefix collision.

### 20.2 Copy single object

For a same-profile server-side copy:

1. Validate source and destination.
2. Check destination policy.
3. Obtain source metadata needed for policy.
4. Use single `CopyObject` where supported and within service limit.
5. Use multipart copy for larger objects.
6. Verify destination using response and optional `HeadObject`.
7. Record completion.

### 20.3 Move or rename single object

```text
Prepare → Copying → VerifyingDestination → DeletingSource → Completed
```

Alternative outcomes:

- `CopyFailed`
- `DestinationVerified_SourceDeleteFailed`
- `CancelledBeforeDelete`

Cancellation after destination verification but before source deletion leaves both objects and MUST be reported clearly.

### 20.4 Recursive prefix operations

A recursive operation has two levels:

- Parent operation job
- Child item records or summarized batches

Planning stage:

1. Enumerate keys using pagination.
2. Estimate count and size.
3. Map every source key to destination key.
4. Validate no destination escapes root constraints.
5. Detect obvious self-overlap, such as moving `a/` into `a/b/`.
6. Present plan and risk summary.

Execution stage:

- Use bounded concurrency.
- Persist item outcome for retry/reporting.
- Delete source only after destination success where move semantics apply.
- Folder markers are handled explicitly.

### 20.5 Recursive delete

- Enumerate objects.
- Display count and known total size.
- Use provider-supported batch delete limits.
- Record object-level errors.
- Handle versioning semantics as provider-reported behavior.
- Do not represent a partial delete as successful.

### 20.6 Collision policies

```rust
pub enum CollisionPolicy {
    Ask,
    Skip,
    Overwrite,
    Rename,
    Fail,
}
```

For recursive jobs, the user may select Apply to All. The decision is stored in the job, not globally unless explicitly requested.

---

## 21. Transfer manager design

### 21.1 Responsibilities

- Create and schedule jobs
- Enforce concurrency
- Maintain state transitions
- Emit ordered progress
- Retry transient failures
- Cancel safely
- Persist transfer history
- Coordinate multipart state
- Provide partial-failure reports

### 21.2 Transfer types

```rust
pub enum TransferOperation {
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
```

### 21.3 State machine

```rust
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
```

Valid high-level transitions:

```text
Queued → Planning → Running → Completed
Queued → Cancelled
Planning → WaitingForUser → Running
Running → Retrying → Running
Running → Pausing → Paused → Running
Running → Cancelling → Cancelled
Running → CompletedWithWarnings
Any active state → Failed
Any active state → Interrupted (unexpected shutdown)
```

Invalid transitions MUST be rejected and logged as internal errors.

### 21.4 Transfer job model

```rust
pub struct TransferJob {
    pub id: Uuid,
    pub operation: TransferOperation,
    pub profile_id: Uuid,
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
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}
```

### 21.5 Scheduling

Recommended defaults:

- Global concurrent jobs: 4
- Per-profile concurrent remote operations: 8
- Multipart parts per file: 4
- Listing/planning concurrency: 2
- Progress sample interval: 200–500 ms

All defaults are configurable within safe bounds.

### 21.6 Retry policy

Retryable examples:

- Timeouts
- Connection reset
- Temporary DNS failure
- 429 throttling
- Selected 5xx provider errors

Normally non-retryable examples:

- Invalid credentials
- Access denied
- Invalid bucket/key
- Local permission denied
- Destination conflict under Fail policy
- Retention or Object Lock rejection

Retry timing:

```text
base_delay × 2^attempt + random_jitter
```

A maximum delay and total attempt limit are enforced.

### 21.7 Progress calculation

- Byte progress comes from stream wrappers and completed multipart parts.
- Speed uses a rolling window rather than lifetime average.
- ETA is hidden when total size is unknown or speed is unstable.
- Parent recursive job progress aggregates child progress.
- Frontend updates are throttled and ordered through a channel.

### 21.8 Cancellation

A cancellation token is attached to each job.

- Queued jobs cancel immediately.
- Streaming operations stop at a safe cancellation point.
- Multipart uploads attempt `AbortMultipartUpload`.
- Compound moves do not delete source for items whose copy was not verified.
- Cancellation results report completed, remaining, and cleanup-required items.

### 21.9 Pause and resume

MVP session pause:

- Stop scheduling new parts/items.
- Allow in-flight operations to finish or cancel according to implementation safety.
- Keep multipart upload ID and completed parts in memory and SQLite.
- Resume within the same application session.

After application restart:

- Jobs are marked `Interrupted`.
- Automatic durable resume is deferred.
- The user may retry as a new job.
- Known incomplete multipart uploads may be offered for cleanup.

---

## 22. Upload design

### 22.1 Upload strategy selection

```text
size < multipart_threshold → single upload
size ≥ multipart_threshold → multipart upload
unknown size stream → multipart upload where supported
```

### 22.2 Multipart planning

Part size calculation must ensure:

- Minimum provider-supported part size
- Maximum allowed number of parts
- Bounded memory use
- Reasonable concurrency and retry cost

A configuration validator adjusts or rejects unsafe settings.

### 22.3 Upload stream

- Rust opens local file with asynchronous or blocking-safe file I/O.
- File regions are streamed to the SDK.
- Per-part buffers are bounded.
- Content type is inferred from extension and may be overridden.
- Optional checksum computation is streamed.

### 22.4 Completion and cleanup

- Complete request contains ordered part numbers and ETags.
- Completion response is validated.
- On failure, job stores upload ID for retry/cleanup.
- Cancel or unrecoverable failure attempts abort.
- A maintenance command can scan application-known orphaned multipart records.

---

## 23. Download design

### 23.1 Local path mapping

Remote key segments are mapped to local names with these rules:

- `..` and absolute path semantics are never honored.
- Windows reserved device names are escaped or renamed.
- Invalid local filename characters are replaced using a reversible mapping where practical.
- Collisions caused by mapping are detected.
- Final path must remain under the selected destination directory after canonicalization.

### 23.2 Temporary files

Example:

```text
track.mp3.s3fm-partial-<transfer-id>
```

The partial file is renamed to the final target only after successful completion and flush.

### 23.3 Resume validation

Session resume may use the partial length and a Range request only if:

- Remote object ETag/identity matches the value recorded at start.
- Remote size has not changed.
- Local partial path and length are valid.
- Provider supports range behavior.

Otherwise restart from zero or ask the user.

---

## 24. Preview and temporary access design

### 24.1 Preview strategy order

1. Short-lived presigned GET URL for supported providers and media types
2. Rust-managed temporary local cache
3. Explicit download and open externally

### 24.2 Presigned URL policy

- Default expiry: 5 minutes
- Maximum interactive preview expiry: 15 minutes
- URL exists only in memory
- URL is never persisted in history or logs
- Frontend clears URL when preview closes
- Content Security Policy is restricted to approved endpoint origins where feasible

### 24.3 Preview cache

- Stored in application cache directory
- Randomized internal names
- Maximum total cache size
- LRU/age cleanup
- Cleared on demand
- Does not contain credentials

---

## 25. Persistence design

### 25.1 Database location

SQLite resides in the platform application-data directory. It MUST NOT be stored beside the executable in normal installation mode.

### 25.2 Database schema

```sql
CREATE TABLE connection_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    endpoint TEXT,
    region TEXT NOT NULL,
    credential_mode TEXT NOT NULL,
    access_key_id TEXT,
    secret_reference TEXT,
    session_reference TEXT,
    default_bucket TEXT,
    root_prefix TEXT,
    addressing_style TEXT NOT NULL,
    allow_insecure_http INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 1,
    last_connected_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE profile_capabilities (
    profile_id TEXT PRIMARY KEY NOT NULL,
    capabilities_json TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE recent_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id TEXT NOT NULL,
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL,
    opened_at TEXT NOT NULL,
    UNIQUE(profile_id, bucket, prefix),
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE bookmarks (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL,
    name TEXT NOT NULL,
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE transfer_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    operation TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    source_json TEXT NOT NULL,
    destination_json TEXT,
    status TEXT NOT NULL,
    collision_policy TEXT NOT NULL,
    total_bytes INTEGER,
    transferred_bytes INTEGER NOT NULL DEFAULT 0,
    total_items INTEGER,
    completed_items INTEGER NOT NULL DEFAULT 0,
    failed_items INTEGER NOT NULL DEFAULT 0,
    retry_count INTEGER NOT NULL DEFAULT 0,
    public_error_json TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id)
);

CREATE TABLE transfer_items (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    source_key TEXT,
    destination_key TEXT,
    local_path TEXT,
    size_bytes INTEGER,
    status TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    public_error_json TEXT,
    FOREIGN KEY(job_id) REFERENCES transfer_jobs(id) ON DELETE CASCADE
);

CREATE TABLE multipart_uploads (
    job_id TEXT PRIMARY KEY NOT NULL,
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    upload_id TEXT NOT NULL,
    part_size INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(job_id) REFERENCES transfer_jobs(id) ON DELETE CASCADE
);

CREATE TABLE multipart_parts (
    job_id TEXT NOT NULL,
    part_number INTEGER NOT NULL,
    etag TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    PRIMARY KEY(job_id, part_number),
    FOREIGN KEY(job_id) REFERENCES multipart_uploads(job_id) ON DELETE CASCADE
);

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 25.3 Data retention

- Completed transfer history default retention: 30 days or configurable count.
- Failed jobs remain until acknowledged or retention expiry.
- Logs use size-based rotation.
- Preview cache uses size and time limits.
- Secret-store entries are deleted with profile deletion.

### 25.4 Migration strategy

- Embedded ordered migrations
- Transaction per migration where SQLite permits
- Database backup before destructive migration
- Schema version recorded
- Failure prevents application from modifying data and opens recovery guidance

---

## 26. IPC command contract

### 26.1 Command naming

Commands use snake_case Rust names and stable frontend wrappers. Request and response fields serialize as camelCase.

### 26.2 Profile commands

| Command | Request | Response |
|---|---|---|
| `list_profiles` | none | `ProfileSummary[]` |
| `get_profile` | profile ID | redacted `ProfileDetail` |
| `test_profile` | draft profile + secret input | `ConnectionTestResult` |
| `create_profile` | draft profile + secret input | `ProfileSummary` |
| `update_profile` | profile ID + revision + changes | `ProfileSummary` |
| `delete_profile` | profile ID + confirmation | empty |
| `duplicate_profile` | profile ID | draft profile without secret |
| `export_profiles` | profile IDs + destination | export summary |
| `import_profiles` | source file | validation/import result |

Secret input is accepted by Rust command and immediately routed to the credential service. Secret fields are never returned.

### 26.3 Explorer commands

| Command | Request | Response |
|---|---|---|
| `list_buckets` | profile ID | bucket summaries |
| `list_entries` | location + token + page size | page |
| `get_object_properties` | location + key | properties |
| `create_folder` | location + name | created entry |
| `generate_preview` | object reference | preview handle |
| `generate_share_link` | object reference + expiry | temporary URL response |
| `add_bookmark` | location + name | bookmark |
| `remove_bookmark` | bookmark ID | empty |

### 26.4 Transfer commands

| Command | Request | Response |
|---|---|---|
| `start_upload` | local selections + destination + policy + progress channel | transfer ID |
| `start_download` | remote selections + local destination + policy + channel | transfer ID |
| `start_copy` | source + destination + policy + channel | transfer ID |
| `start_move` | source + destination + policy + channel | transfer ID |
| `start_delete` | selections + confirmation + channel | transfer ID |
| `pause_transfer` | transfer ID | state |
| `resume_transfer` | transfer ID | state |
| `cancel_transfer` | transfer ID | state |
| `retry_transfer` | transfer ID | new or reset transfer ID |
| `list_transfers` | filter + paging | transfer summaries |
| `get_transfer_details` | transfer ID | details and failed items |
| `clear_transfer_history` | filter | count removed |

### 26.5 Settings and diagnostics commands

| Command | Description |
|---|---|
| `get_settings` | Returns redacted settings |
| `update_settings` | Validates and stores changes |
| `open_log_directory` | Opens platform log folder |
| `export_diagnostics` | Writes redacted diagnostic archive |
| `clear_logs` | Deletes eligible rotated logs |
| `check_for_updates` | Runs signed update check |

### 26.6 Channel messages

```rust
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum TransferChannelMessage {
    Snapshot(TransferSnapshot),
    Progress(TransferProgress),
    ItemCompleted(ItemResult),
    Warning(PublicWarning),
    StateChanged(TransferStatus),
    Finished(TransferResult),
}
```

Channels are preferred for transfer progress because ordering and throughput matter. Global events are reserved for low-frequency notifications such as profile invalidation or update availability.

---

## 27. Error handling

### 27.1 Public error envelope

```rust
pub struct PublicError {
    pub code: AppErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    pub details: BTreeMap<String, serde_json::Value>,
}
```

### 27.2 Error codes

```rust
pub enum AppErrorCode {
    ValidationFailed,
    ProfileNotFound,
    ProfileRevisionConflict,
    InvalidEndpoint,
    InsecureEndpointBlocked,
    CredentialMissing,
    CredentialStoreUnavailable,
    CredentialRejected,
    BucketNotFound,
    BucketAccessDenied,
    ObjectNotFound,
    DestinationExists,
    RootPrefixViolation,
    UnsupportedProviderFeature,
    NetworkUnavailable,
    RequestTimedOut,
    TlsError,
    RateLimited,
    ProviderUnavailable,
    LocalPathInvalid,
    LocalPermissionDenied,
    LocalDiskFull,
    TransferCancelled,
    TransferStateConflict,
    DatabaseError,
    UpdateVerificationFailed,
    Unknown,
}
```

### 27.3 Error mapping

Internal errors are classified from:

- Input validation
- Credential store
- SQLite/SQLx
- Local I/O
- AWS SDK timeout/dispatch/service errors
- Provider-specific service codes
- Cancellation

The public message is safe and actionable. Detailed error chains go to redacted local logs using provider request IDs when available.

### 27.4 Retry classification

Retryability is decided in Rust, not by the UI. A service error code alone is not always sufficient; operation idempotency and current compound-operation stage are considered.

---

## 28. Security design

### 28.1 Security objectives

1. Prevent permanent credential exposure to frontend code.
2. Minimize local filesystem authority.
3. Prevent remote web content from acquiring native privileges.
4. Prevent path traversal and unsafe overwrite.
5. Protect updates and release artifacts.
6. Keep logs useful without leaking secrets.
7. Make destructive actions explicit and auditable locally.

### 28.2 Tauri capabilities

- Main window receives only required dialog, shell-open, updater, and application commands.
- Generic unrestricted filesystem plugin access is avoided.
- Rust performs file reads and writes after native dialog selection.
- No remote URL is configured as main webview content.
- Additional windows, if added later, receive separate minimal capabilities.
- Capability files reside in `src-tauri/capabilities/` and are reviewed as security-sensitive code.

### 28.3 IPC validation

Every command validates:

- UUID format and profile existence
- Profile revision where updates are optimistic
- Bucket and key constraints
- Root-prefix authorization
- URL syntax and scheme
- Numeric configuration bounds
- File path authorization and canonical location
- Transfer state transitions

### 28.4 Secret handling

MUST redact:

- Secret access keys
- Session tokens
- Authorization headers
- Credential-store payloads
- Presigned URL signature query parameters
- Future SSO tokens
- Update signing private keys

Access key IDs are not equivalent to secret keys but SHOULD be masked in logs and diagnostic exports.

### 28.5 Filesystem security

- Use native file/folder pickers.
- Keep a Rust-side allowlist of selected roots for each command/job.
- Canonicalize local paths before access.
- Re-check path containment after joining mapped object-key segments.
- Avoid following directory symlinks during recursive upload by default.
- Use creation flags that reduce accidental overwrite where available.

### 28.6 Network security

- HTTPS by default.
- No TLS verification bypass.
- HTTP allowed only by explicit profile opt-in and prominent warning.
- Redirect behavior must not leak signed headers or credentials to an unrelated host.
- Endpoint host changes invalidate the client and prior presigned URLs.

### 28.7 Threat model summary

| Threat | Mitigation |
|---|---|
| Frontend XSS invokes native APIs | Bundled content only, restrictive CSP, minimal capabilities, Rust validation |
| Credential leakage through UI | Secrets never returned; credential references only |
| Path traversal on download | Safe path mapping, canonical containment checks |
| Accidental destructive delete | Confirmation, count/size plan, typed confirmation threshold |
| Malicious profile import | Validate endpoint and fields; no imported secrets by default |
| Log leakage | Central redaction layer and tests |
| Tampered update | Signed updater metadata/artifacts |
| Provider incompatibility | Capability model and conservative fallbacks |
| Move causes data loss | Verify destination before deleting source |
| Symlink traversal on upload | Do not follow by default; explicit policy later |

---

## 29. Logging, diagnostics, and privacy

### 29.1 Logging

Structured logs include:

- Timestamp
- Level
- Component
- Operation/transfer ID
- Profile ID, not profile secret
- Provider request ID
- Error classification
- Duration and retry count

### 29.2 Log levels

- ERROR: unrecoverable operation failure
- WARN: retry, partial failure, insecure configuration warning
- INFO: lifecycle and completed high-level actions
- DEBUG: redacted operational detail
- TRACE: development-only deep diagnostics

### 29.3 Diagnostic export

May contain:

- Application version and platform
- Redacted configuration
- Provider types and capability states
- Recent public errors
- Redacted logs
- Database schema version

Must not contain:

- Permanent credentials
- Session tokens
- Presigned URLs
- Full local personal file contents
- Full object content

### 29.4 Analytics

No remote telemetry is required in the MVP. If added later, it must be opt-in or transparently disclosed and must never include bucket names, object keys, local paths, or credentials by default.

---

## 30. Performance and resource management

### 30.1 Memory

- Use bounded byte buffers.
- Do not collect whole listings beyond configured UI cache limits.
- Stream object bodies.
- Release preview resources when closed.

### 30.2 CPU

- Checksum work may use a bounded blocking pool.
- UI progress aggregation should avoid high-frequency serialization.
- Recursive planning should yield to cancellation.

### 30.3 Network

- Limit global and per-profile concurrency.
- Respect throttling and provider retry hints.
- Allow future bandwidth limits through a token-bucket abstraction.
- Avoid `HeadObject` calls for every listed item unless requested.

### 30.4 SQLite

- Use WAL mode where validated for the platform.
- Serialize migrations.
- Batch transfer-item writes.
- Use indexes on profile IDs, job IDs, status, and timestamps.

---

## 31. Packaging and updates

### 31.1 Windows baseline

- Windows 10/11 x64
- Tauri NSIS `setup.exe`
- Per-user installation by default
- WebView2 downloaded bootstrapper by default
- Optional offline installer profile for controlled environments

### 31.2 Build artifacts

```text
S3FileManager_<version>_x64-setup.exe
S3FileManager_<version>_x64.exe
latest.json or equivalent signed update metadata
checksums.txt
SBOM artifact
```

### 31.3 Signing

Two independent concerns:

1. Windows Authenticode signing for executable and installer trust
2. Tauri updater signature verification for update authenticity

Private signing material MUST only exist in protected release infrastructure.

### 31.4 Update behavior

- Manual Check for Updates in MVP
- Optional automatic background check later
- Display version, release notes, and restart requirement
- Verify signature before installation
- Never replace the application with an unsigned artifact
- Provide recovery guidance for failed updates

### 31.5 Versioning

Use semantic versioning:

```text
MAJOR.MINOR.PATCH
```

Database migrations and IPC contracts are tied to application releases and tested for upgrade compatibility.

---

## 32. Build and repository structure

```text
s3-file-manager/
├── src/
├── src-tauri/
│   ├── capabilities/
│   ├── migrations/
│   ├── icons/
│   ├── src/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── tests/
│   ├── fixtures/
│   └── provider-smoke/
├── scripts/
├── docs/
│   ├── SDD.md
│   ├── SECURITY.md
│   └── RELEASE.md
├── package.json
├── pnpm-lock.yaml
├── Cargo.lock
└── README.md
```

### 32.1 Dependency policy

- Commit `Cargo.lock` and frontend lockfile.
- Pin release builds to lockfiles.
- Run Rust and npm dependency audits in CI.
- Review Tauri plugin permissions before upgrades.
- Avoid unnecessary plugins and native dependencies.

---

## 33. Testing strategy

### 33.1 Unit tests

Rust:

- Object-key and prefix invariants
- Root-prefix authorization
- Endpoint validation
- Local path mapping
- Windows reserved-name handling
- Collision policy
- Transfer state transitions
- Retry classification and backoff bounds
- Progress aggregation
- Error mapping
- Secret redaction
- Profile validation

Frontend:

- Explorer rendering
- Breadcrumb rules
- Selection and keyboard behavior
- Transfer store projections
- Error presentation
- Profile form preset behavior

### 33.2 Integration tests

Using MinIO in CI:

- Profile connection
- Bucket and object listing
- Pagination
- Folder markers
- Upload/download
- Multipart upload and abort
- Copy/move/rename
- Recursive delete
- Presigned URL where supported

SQLite:

- Fresh migration
- Upgrade migration
- Transaction rollback
- Transfer history retention

Credential store:

- Put/get/delete
- Missing entry
- Store unavailable behavior
- Profile deletion compensation

### 33.3 Provider smoke tests

Run controlled tests against:

- AWS S3
- Cloudflare R2
- MinIO

Smoke tests should use isolated buckets/prefixes and automatic cleanup.

### 33.4 Security tests

- Verify no command returns secret fields.
- Search logs and diagnostic archives for seeded test secrets.
- Path traversal test corpus.
- Malicious endpoint URL test corpus.
- Capability review test/checklist.
- Tampered update rejection.
- CSP and remote-content checks.

### 33.5 Performance tests

- Listing folders with large key counts using pagination
- Multipart upload with simulated latency and failures
- Concurrent transfers
- Recursive operations with tens of thousands of synthetic items
- UI progress update load
- Memory ceiling during multi-gigabyte transfer

### 33.6 Installer tests

- Clean Windows 10 VM
- Clean Windows 11 VM
- Missing WebView2 path
- Upgrade from previous version
- Uninstall while no jobs active
- Per-user install permissions
- Signed artifact verification

---

## 34. CI/CD design

### 34.1 Pull request pipeline

1. Format and lint TypeScript
2. Frontend unit tests
3. `cargo fmt --check`
4. `cargo clippy` with warnings policy
5. Rust unit tests
6. Database migration tests
7. MinIO integration tests
8. Dependency audit
9. Build unsigned development bundle

### 34.2 Release pipeline

1. Validate clean tag and version
2. Re-run full test suite
3. Build on trusted Windows runner
4. Produce executable and NSIS installer
5. Generate SBOM and checksums
6. Authenticode sign artifacts
7. Generate and sign Tauri updater artifacts
8. Publish staged release
9. Run clean-VM smoke test
10. Promote to stable channel

### 34.3 Release channels

- Stable
- Beta, optional after MVP
- Development/nightly, internal only

---

## 35. Implementation phases

### Phase 0: Foundation

- Tauri 2 project
- React shell and routing
- Rust module boundaries
- SQLite migrations
- Logging and error envelope
- Capability baseline

### Phase 1: Profiles and listing

- Profile CRUD
- Credential-store integration
- AWS/R2/MinIO presets
- Connection test
- Client manager
- List buckets and list entries
- Explorer navigation and pagination

### Phase 2: Basic transfers

- Transfer manager and state machine
- Single upload and download
- Progress channels
- Retry and cancel
- Transfer history

### Phase 3: Large and recursive operations

- Multipart upload
- Folder upload/download
- Copy/move/rename
- Multipart copy
- Recursive delete
- Partial-failure reports

### Phase 4: Preview, diagnostics, packaging

- Preview strategies
- Share URLs
- Diagnostic export
- Windows NSIS packaging
- Signing and updater workflow
- Accessibility and performance pass

### Phase 5: Post-MVP

- Persistent resume
- AWS profile/SSO credentials
- Cross-profile streaming transfers
- Search index
- Versions and recovery tools
- macOS/Linux production support

---

## 36. Acceptance criteria

The MVP is accepted when all of the following are demonstrated:

1. Multiple profiles can be created, edited, tested, and deleted.
2. Permanent secrets do not appear in SQLite, frontend state, logs, exports, or IPC responses.
3. AWS S3, Cloudflare R2, and MinIO smoke tests pass.
4. A configured bucket can be opened when bucket-list permission is unavailable.
5. Prefix navigation and pagination work correctly.
6. Large uploads use multipart and remain responsive.
7. Downloads stream to `.partial` files and finalize safely.
8. Transfer progress, retry, cancellation, and failure states are visible.
9. Single-object copy, move, rename, and delete work.
10. Recursive operations display progress and item-level failures.
11. Destination verification occurs before source deletion in move operations.
12. Root-prefix restrictions cannot be bypassed through UI or direct command invocation.
13. Download path traversal tests pass.
14. Diagnostic export passes seeded-secret scanning.
15. The Windows NSIS installer installs and launches on clean Windows 10/11 test systems.
16. A signed update artifact is rejected when modified.
17. UI remains usable during four concurrent transfers.
18. Application shutdown warns about active transfers and records interruption accurately.

---

## 37. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Provider compatibility differences | Features fail outside AWS | Presets, capabilities, smoke tests, conservative fallbacks |
| Recursive operations on huge prefixes | High cost and long duration | Planning, estimates, pagination, cancellation, warnings |
| Copy-then-delete partial failure | Duplicate objects or cleanup needed | Explicit compound states and verification |
| Credential-store failure | User cannot connect or delete secrets | Actionable errors, retry, compensation, diagnostic guidance |
| Frontend compromise | Native operations invoked maliciously | Bundled content, CSP, capabilities, Rust validation |
| Progress message overload | UI lag | Throttled ordered channels and aggregation |
| Multipart orphan leakage | Storage charges | Abort on cancel; track and offer cleanup |
| Local filename incompatibility | Download collision or failure | Safe mapping and collision reporting |
| Update signing compromise | Supply-chain compromise | Isolated keys, protected runner, staged publishing |
| Tauri/plugin changes | Build or permission regression | Lockfiles, upgrade review, CI installer tests |

---

## 38. Decision log

| ID | Decision | Status | Reason |
|---|---|---|---|
| ADR-001 | Use Tauri 2 | Accepted | Rust-native lightweight cross-platform desktop framework |
| ADR-002 | Use Rust for backend | Accepted | Secure native operations and strong async model |
| ADR-003 | Use React + TypeScript | Accepted | Mature explorer UI ecosystem |
| ADR-004 | Use AWS SDK for Rust | Accepted | First-party S3 and credential support |
| ADR-005 | Use SQLite + SQLx | Accepted | Embedded transactional metadata store |
| ADR-006 | Store secrets in OS credential store | Accepted for Windows MVP | Better UX and OS-bound protection |
| ADR-007 | Keep Stronghold as optional future vault | Accepted | Portable encrypted export/use case without forcing master password |
| ADR-008 | Use commands for RPC and channels for progress | Accepted | Typed request-response and ordered streaming updates |
| ADR-009 | Direct SDK transfer through Rust | Accepted | Desktop app does not need browser upload presigning |
| ADR-010 | Presigned URLs only for preview/share | Accepted | Temporary access without permanent credential exposure |
| ADR-011 | Defer cross-profile transfer | Accepted | Requires local streaming and more complex retry semantics |
| ADR-012 | Defer global search | Accepted | Requires costly scan or local index |
| ADR-013 | Windows-first NSIS release | Accepted | Focused production target and simple setup experience |
| ADR-014 | Do not allow TLS verification bypass | Accepted | Avoid dangerous permanent profile configuration |

---

## 39. Resolved implementation decisions

All baseline open questions are resolved for the MVP as follows:

| Topic | Final MVP decision |
|---|---|
| Product identity | `S3 File Manager`; application ID `com.s3filemanager.desktop` |
| Minimum Windows | Windows 10 version 2004, build 19041, x64 |
| Secret storage | Windows Credential Manager behind `CredentialStore`; no mandatory master password |
| Multipart threshold | 64 MiB |
| Initial multipart part size | 16 MiB, automatically raised to stay within provider part limit |
| Concurrent jobs | 4 by default; 1–16 configurable |
| Multipart part concurrency | 4 by default; 1–16 configurable |
| Share-link expiry | Default 1 hour; maximum 7 days |
| Audio/video preview | Included in MVP for allowlisted formats |
| Windows filename mapping | Reversible `_s3xHH_` escaping plus `_s3r_` reserved-name prefix and manifest |
| Recursive planning | Hybrid: persist items for plans up to 100,000 entries; checkpointed page planning above that |
| Transfer history retention | 30 days or 1,000 completed jobs |
| Log retention | 14 days and 100 MiB total |
| Update channels | Static signed manifests for stable and beta |
| Installer | NSIS per-user with WebView2 bootstrapper download mode |
| Code signing | Authenticode-ready pipeline; production release gate requires configured certificate |
| Crash reporting | Disabled in MVP; diagnostic export is user initiated |
| Analytics | Disabled in MVP |
| Cross-profile transfer | Deferred and rejected explicitly by MVP commands |
| Global search | Deferred; current loaded-folder filter only |

No unresolved product or architectural question blocks MVP implementation.

---

## 40. Review record

### 40.1 Review scope

The review covers the full product history and every MVP item, including the original browser-oriented concept, the Wails/Go option, the final Tauri 2/Rust architecture, multiple profiles, all provider presets, Explorer interactions, secret storage, large transfers, recursive operations, metadata, preview, security, Windows packaging, testing, and release behavior.

### 40.2 Review passes completed

1. Architecture and trust-boundary review
2. Functional-requirement completeness review
3. UI and interaction-state review
4. S3 semantics and provider-compatibility review
5. Transfer state-machine and failure-mode review
6. SQLite and credential-store consistency review
7. Typed IPC and Tauri capability review
8. Local filesystem and path-traversal review
9. Preview and temporary bearer-secret review
10. Windows installer, signing, and updater review
11. Requirement-to-acceptance traceability review
12. Automated duplicate-ID and secret-pattern scan

### 40.3 Corrections made across both reviews

1. Removed browser presigned upload/download architecture from normal desktop transfers.
2. Limited presigned URLs to temporary preview/share workflows.
3. Selected Windows Credential Manager through a portable `CredentialStore` abstraction.
4. Defined compensation workflows between SQLite and the OS credential store.
5. Defined provider presets, validation, and capability states.
6. Specified list/grid behavior, loaded-page selection, shortcuts, context menus, and all explorer states.
7. Defined reversible Windows path encoding and recursive mapping manifests.
8. Defined metadata fields and metadata-replacement operation.
9. Included audio/video preview with allowlist, bounded cache, and active-content restrictions.
10. Defined pause/resume support separately for every transfer type.
11. Selected hybrid recursive planning: materialized up to 100,000 items and checkpointed beyond that.
12. Added complete typed DTOs, error envelopes, destructive confirmation tokens, and command authorization.
13. Locked settings defaults and ranges.
14. Locked Windows build, installer, WebView2, signing, update, privacy, and retention decisions.
15. Added detailed acceptance criteria for every MVP feature group.

### 40.4 Review conclusion

**Result: APPROVED FOR MVP IMPLEMENTATION.**

Section 39 contains resolved decisions rather than open questions. Sections 42–57 provide the detailed implementation specification and one-to-one MVP coverage needed to begin development without another product-design pass.

---

## 41. Traceability summary

| Product goal | Requirements | Primary design | Detailed acceptance |
|---|---|---|---|
| Multiple secure profiles | FR-PRO-*; NFR-SEC-* | 16–18, 42–43, 50, 52 | AC-PRO-*; AC-SEC-* |
| Explorer-style navigation | FR-EXP-*; NFR-UX-* | 14, 19, 44–45, 50 | AC-EXP-* |
| Reliable uploads | FR-UP-*; FR-TRN-* | 21–22, 45, 48–52 | AC-UP-*; AC-TRN-* |
| Reliable downloads | FR-DL-*; FR-TRN-* | 21, 23, 45, 48–52 | AC-DL-*; AC-TRN-* |
| Safe copy/move/delete | FR-OP-*; FR-DEL-* | 20–21, 48–50 | AC-OP-*; AC-DEL-* |
| Metadata and preview | FR-MET-*; FR-PRV-* | 24, 46–47, 50–52 | AC-MET-*; AC-PRV-* |
| Provider compatibility | FR-PRO-019; NFR-COMP-* | 18, 42 | AC-PRO-008; provider smoke tests |
| Settings and persistence | FR-SET-*; NFR-REL-* | 25, 51–52 | AC-SET-*; fault tests |
| Windows distribution | FR-PKG-* | 31, 34, 53 | AC-PKG-* |

The exhaustive group and MVP feature matrices are in Section 55 and the generated traceability CSV delivered with this document.

---

## 42. Provider preset and capability specification

### 42.1 Preset principles

A provider preset supplies validated defaults and a capability baseline. It MUST NOT claim unsupported capabilities merely because an endpoint accepts S3 authentication. Runtime capability probes MAY reduce or increase the baseline only when a safe, non-destructive request proves the result.

All presets use SigV4 authentication through the AWS SDK for Rust. Secrets are resolved only in Rust. Endpoint URLs are canonicalized before client creation.

### 42.2 Provider preset matrix

| Provider | Endpoint behavior | Region behavior | Addressing default | Bucket-list behavior | MVP notes |
|---|---|---|---|---|---|
| AWS S3 | SDK-managed endpoint; custom endpoint hidden | Required user selection; no silent production default | Virtual-hosted | Attempt `ListBuckets`; direct default bucket fallback | Reference implementation |
| Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` | Fixed `auto` | SDK default; no forced path-style | Attempt `ListBuckets`; direct bucket fallback | Account ID field generates endpoint |
| MinIO | User-supplied HTTPS or explicitly approved HTTP endpoint | Default `us-east-1`, editable | Path-style enabled | Attempt `ListBuckets`; direct bucket fallback | HTTP requires warning and opt-in |
| Wasabi | Region-specific HTTPS endpoint generated from selected region | Required selected Wasabi region | Virtual-hosted | Attempt `ListBuckets`; direct bucket fallback | Endpoint list is packaged as versioned preset data |
| Custom S3 | User-supplied HTTPS or explicitly approved HTTP endpoint | Required, default suggestion `us-east-1` | User-selectable | Conservative; try list then head default bucket | No capability is assumed beyond successful probes |

### 42.3 AWS S3 preset

Required fields:

- Profile name
- Region
- Access key ID
- Secret access key

Optional fields:

- Session token
- Default bucket
- Root prefix

Locked fields:

- Endpoint: SDK-managed
- TLS verification: enabled

Connection test sequence:

1. Resolve credentials.
2. Create regional client.
3. Attempt `ListBuckets`.
4. If access is denied and a default bucket is present, call `HeadBucket`.
5. If a root prefix is present, request one-entry `ListObjectsV2` under that prefix.
6. Probe presigning locally without transmitting a request.
7. Store tested capability results and timestamp.

### 42.4 Cloudflare R2 preset

The UI accepts an Account ID and derives the endpoint. Manual endpoint editing is hidden under Advanced and is permitted only for testing custom R2 gateways.

Defaults:

```text
region = auto
endpoint = https://<account-id>.r2.cloudflarestorage.com
force_path_style = false
session_token = unsupported in the standard preset
```

Capability baseline:

```text
list_buckets               unknown until tested
multipart_upload            true
multipart_copy              probe_required
presigned_get               true
presigned_put               true but not used for normal desktop transfer
versioning                  false in MVP UI
object_lock                 false in MVP UI
storage_class_edit          false
checksum                    provider-reported only
```

The capability record MUST retain the date of detection because R2 S3 compatibility evolves independently of the application.

### 42.5 MinIO preset

Defaults:

```text
region = us-east-1
force_path_style = true
endpoint = user supplied
```

Validation:

- Endpoint MUST include scheme and host.
- Private IP addresses and local hostnames are allowed because MinIO is commonly deployed on private networks.
- Plain HTTP is allowed only after an explicit warning that credentials and data can be intercepted.
- Self-signed TLS certificates are not bypassed in the MVP; the user must install the issuing CA into the operating-system trust store.

### 42.6 Wasabi preset

The application packages a JSON table of supported Wasabi regions and endpoints. The table is part of the application release and is not downloaded at runtime.

Example record shape:

```json
{
  "region": "us-east-1",
  "endpoint": "https://s3.wasabisys.com",
  "label": "US East 1"
}
```

The exact table MUST be verified against Wasabi documentation during each release. A user can always choose Custom S3 when a newer endpoint is not yet packaged.

### 42.7 Custom S3 preset

The custom preset exposes:

- Endpoint
- Region
- Addressing mode
- Default bucket
- Root prefix
- HTTP opt-in

It does not expose TLS verification bypass. Capability defaults are conservative:

```text
can_list_buckets       = unknown
multipart_upload       = unknown until tested
multipart_copy         = unknown
presigned_get          = unknown
versioning             = unknown
object_lock            = unknown
```

### 42.8 Capability persistence

```rust
pub struct ProviderCapabilityRecord {
    pub profile_id: Uuid,
    pub tested_at: DateTime<Utc>,
    pub sdk_version: String,
    pub can_list_buckets: CapabilityState,
    pub can_head_bucket: CapabilityState,
    pub supports_multipart_upload: CapabilityState,
    pub supports_multipart_copy: CapabilityState,
    pub supports_presigned_get: CapabilityState,
    pub supports_checksum: CapabilityState,
    pub supports_versioning: CapabilityState,
    pub supports_object_lock: CapabilityState,
}

pub enum CapabilityState {
    Supported,
    Unsupported,
    AccessDenied,
    Unknown,
}
```

Capability failures caused by access denial MUST NOT be interpreted as provider non-support.

---

## 43. Profile lifecycle and transactional behavior

### 43.1 Profile form states

The profile editor has these states:

```text
New
Dirty
Validating
Testing
TestPassed
TestFailed
Saving
Saved
SaveFailed
Deleting
Deleted
```

The Save button is enabled when local field validation passes. A successful connection test is recommended but not mandatory when the user explicitly selects `Save without testing`.

### 43.2 Field validation

| Field | Rule |
|---|---|
| Name | 1–80 Unicode scalar values after trimming; cannot be only whitespace |
| Endpoint | Absolute `https://` URL, or `http://` only with explicit opt-in; no user info, fragment, or query |
| Region | 1–64 ASCII letters, numbers, and hyphens |
| Access key ID | 1–256 characters; masked after save except first/last four characters |
| Secret key | Required for static credentials; never returned after save |
| Session token | Optional; 1–16,384 characters; never returned after save |
| Default bucket | 3–255 characters, provider validation applied |
| Root prefix | Empty or normalized key prefix ending `/`; maximum 1,024 UTF-8 bytes |

### 43.3 Create transaction

Create is a saga because SQLite and Windows Credential Manager do not share a transaction.

```text
1. Validate draft.
2. Generate profile UUID and credential reference.
3. Write secret bundle to credential store.
4. Insert SQLite profile row referencing the secret.
5. If SQLite insert fails, delete the newly written secret.
6. If compensation fails, write a credential_cleanup record and surface a warning.
7. Invalidate profile list cache and publish low-frequency profile-created event.
```

### 43.4 Update transaction

```text
1. Load current profile and active-job count.
2. Reject protected-field changes when active jobs reference the profile.
3. If secrets changed, write a new secret entry under a new reference.
4. Update SQLite profile row in one transaction.
5. Delete old secret entry only after commit.
6. If old-secret deletion fails, queue cleanup without failing the profile update.
7. Remove cached S3 client.
8. Mark stored capability record stale.
```

### 43.5 Duplicate

Duplicate copies non-secret configuration and bookmarks are not copied. The user must choose one of:

- Reuse credentials: duplicate references the same secret bundle through a ref-counted logical credential record.
- Enter new credentials: creates an independent secret bundle.

MVP default: Reuse credentials. Deleting either profile decrements the logical reference count and removes the OS credential only when no profile references it.

### 43.6 Delete

Before deletion the UI shows:

- Profile name
- Endpoint/provider
- Number of active jobs
- Number of bookmarks
- Whether credentials are shared with another profile

Deletion is blocked while active jobs reference the profile. Bookmarks and recents owned by the profile are deleted transactionally. Transfer history remains but stores a profile display snapshot so history remains understandable.

### 43.7 Favorite ordering

Favorite profiles are ordered by explicit `favorite_order`; remaining profiles are ordered by case-folded display name and UUID as a stable tie-breaker. Dragging favorites updates order in one SQLite transaction.

### 43.8 Import/export schema

```json
{
  "schemaVersion": 1,
  "exportedAt": "2026-08-04T04:00:00Z",
  "application": "S3 File Manager",
  "profiles": [
    {
      "exportId": "uuid",
      "name": "R2 Music",
      "provider": "cloudflare-r2",
      "endpoint": "https://ACCOUNT_ID.r2.cloudflarestorage.com",
      "region": "auto",
      "defaultBucket": "music",
      "rootPrefix": "albums/",
      "forcePathStyle": false,
      "favorite": true,
      "hasCredentials": false
    }
  ]
}
```

`hasCredentials` is informational only. Secret references, tokens, presigned URLs, capability claims, and last error detail are never exported.

---

## 44. Explorer UX and interaction specification

### 44.1 Window layout

```text
┌─────────────────────────────────────────────────────────────────────┐
│ App menu │ Profile selector │ Bucket selector │ Search/filter      │
├───────────────┬─────────────────────────────────────────────────────┤
│ Profiles      │ Toolbar: Back Forward Up Refresh Upload New Folder │
│ Favorites     ├─────────────────────────────────────────────────────┤
│ Buckets       │ Breadcrumb                                          │
│ Bookmarks     ├─────────────────────────────────────────────────────┤
│ Recent        │ File area: list or grid                             │
│               │                                                     │
├───────────────┴─────────────────────────────────────────────────────┤
│ Status: loaded count │ selection │ total selected size │ transfers │
├─────────────────────────────────────────────────────────────────────┤
│ Collapsible transfer queue                                         │
└─────────────────────────────────────────────────────────────────────┘
```

### 44.2 List columns

| Column | Folder | Object | Sort behavior |
|---|---|---|---|
| Name | Display segment | Display segment | Locale-aware case-folded; exact key tie-breaker |
| Type | Folder | MIME category/extension | Loaded entries only |
| Size | Blank | Bytes | Unknown values last |
| Last Modified | Blank/marker value | Timestamp | Unknown values last |
| Storage Class | Blank | Provider value | Unknown values last |

Column widths and visibility persist per user. Name cannot be hidden.

### 44.3 Grid tiles

Grid tile size has Small, Medium, and Large modes. Safe image thumbnails are generated only for allowlisted raster formats. Other types use deterministic icons. A failed thumbnail does not mark the object as failed.

### 44.4 Focus and selection

- One entry has keyboard focus independently of selection.
- Plain click selects one entry.
- Ctrl-click toggles one entry.
- Shift-click selects the contiguous range within the current loaded sorted view.
- Ctrl+A selects loaded visible entries after filtering.
- Escape clears selection; a second Escape closes an open context menu or dialog.
- Selection never implicitly includes unloaded pages.

### 44.5 Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Enter | Open focused folder or preview/properties |
| Backspace / Alt+Up | Navigate up |
| Alt+Left / Alt+Right | Back / forward |
| F2 | Rename single selection |
| Delete | Delete confirmation |
| Ctrl+C | Stage remote copy selection internally |
| Ctrl+X | Stage remote move selection internally |
| Ctrl+V | Execute staged operation into current prefix |
| Ctrl+A | Select loaded filtered entries |
| Ctrl+F | Focus current-folder filter |
| Ctrl+L | Focus breadcrumb/path entry |
| F5 | Refresh |
| Ctrl+U | Choose files to upload |
| Ctrl+Shift+U | Choose folder to upload |
| Ctrl+Shift+N | New folder marker |
| Alt+Enter | Properties |

The internal clipboard stores only opaque remote references and expires when the application closes. It does not place credentials or presigned URLs on the Windows clipboard.

### 44.6 Context menus

Single object:

```text
Preview
Download
Copy
Move
Rename
Generate Share Link
Properties
Delete
```

Single prefix:

```text
Open
Download Folder
Copy Folder
Move Folder
Rename Folder
Bookmark
Properties
Delete Recursively
```

Multiple entries:

```text
Download
Copy
Move
Delete
Properties Summary
```

Unsupported actions are disabled with an explanatory tooltip rather than silently omitted when discoverability is useful.

### 44.7 Loading and error states

| State | Required UI |
|---|---|
| Initial loading | Skeleton rows and cancellable spinner |
| Empty | `This location contains no objects` plus Upload/New Folder actions |
| Access denied | Provider-neutral explanation and Test Profile/Open Default Bucket actions |
| Offline | Retry and diagnostics link |
| Partial page | Loaded count plus Load More control or automatic virtualized fetch |
| Filter no match | `No loaded items match`; clear filter action |
| Cancelled | No error toast unless cancellation was unexpected |
| Stale response | Silently ignored using generation token |

### 44.8 Pagination

The default page request is 500 entries and the allowed range is 100–1,000. Auto-load begins when the user scrolls within 20% of the end. At most one next-page request is active per location. The continuation token is never interpreted by the frontend.

### 44.9 Sorting and filtering

Sorting is client-side over loaded entries. When the folder is incomplete, the header displays `Sorted loaded results`. Filtering is a local substring match over display name and optional extension/type filters. No request is sent while typing.

### 44.10 Navigation history

A history item contains profile ID, bucket, prefix, scroll anchor, sort, view mode, and selected focused key. Selection is not restored; focus MAY be restored if the key remains loaded.

### 44.11 Drag and drop

Local-to-remote drag:

- Windows file paths are accepted only through Tauri/native drop events.
- React receives opaque path handles or validated path strings according to Tauri API behavior.
- Drop on a folder uploads into that folder.
- Drop on empty file area uploads into current prefix.

Remote-to-remote drag:

- Same profile: Copy by default, Move with Shift.
- Different profile: rejected in MVP with Post-MVP explanation.

Remote-to-Windows Explorer drag-out is deferred because secure shell data-object integration is outside MVP scope.

---

## 45. Local and remote naming specification

### 45.1 Remote key invariants

- Keys are stored and transmitted exactly as UTF-8 byte sequences accepted by the SDK/provider.
- Display segmentation uses `/` only.
- Backslash is a normal remote key character and is not treated as a separator.
- Leading `/` is not generated by the UI but existing objects with leading `/` remain accessible under an escaped display segment.
- Empty path segments in existing keys are preserved.

### 45.2 Upload mapping

For selected local folder `D:\Music\Album` uploaded to prefix `library/`:

Default mapping:

```text
D:\Music\Album\song.mp3 -> library/Album/song.mp3
```

With `Upload contents only`:

```text
D:\Music\Album\song.mp3 -> library/song.mp3
```

Local path separators are replaced with `/`. Local names are not otherwise sanitized because S3 keys can represent them; however NUL and invalid UTF-16 sequences cannot originate from normal Windows file APIs.

### 45.3 Download reversible escaping

Each Windows path segment is encoded from its UTF-8 bytes. Safe printable characters are preserved except underscore sequences that could be misinterpreted as escapes.

Encoding rules:

1. Each invalid Windows character or control byte is encoded as `_s3xHH_`, where `HH` is uppercase hexadecimal.
2. Literal text matching `_s3x[0-9A-Fa-f]{2}_` is escaped by encoding the first underscore as `_s3x5F_`.
3. A trailing space is `_s3x20_`; a trailing period is `_s3x2E_`.
4. Reserved base names are prefixed `_s3r_` after character escaping.
5. Case-collision suffixes use `_s3c_<8hex>` derived from SHA-256 of the full remote key.
6. The mapping manifest stores exact remote key to relative local path.

Examples:

| Remote segment | Local segment |
|---|---|
| `a:b.txt` | `a_s3x3A_b.txt` |
| `CON.txt` | `_s3r_CON.txt` |
| `name.` | `name_s3x2E_` |
| `x_s3x3A_y` | `x_s3x5F_s3x3A_y` |

### 45.4 Mapping manifest

```json
{
  "schemaVersion": 1,
  "source": {
    "profileId": "uuid",
    "bucket": "bucket",
    "prefix": "music/"
  },
  "createdAt": "2026-08-04T04:00:00Z",
  "entries": [
    {
      "key": "music/a:b.txt",
      "relativePath": "a_s3x3A_b.txt"
    }
  ]
}
```

The manifest is named `.s3-key-map.json`. If that name conflicts with a downloaded object, the application uses `.s3-key-map.<8hex>.json` and records the name in transfer results.

### 45.5 Path safety

- The final path is joined from validated relative segments.
- The result is canonicalized to the extent possible before creation.
- Existing parent directories are checked for reparse points.
- The final path MUST remain under the user-selected destination root.
- Maximum normalized path safety limit defaults to 30,000 UTF-16 code units, below the extended Windows maximum.
- Each path segment MUST remain within filesystem limits; failures are item-specific.

### 45.6 Collision resolution

Collision is detected using case-insensitive Windows comparison and existing filesystem entries.

Policies:

- Ask
- Skip
- Overwrite
- Rename

Rename format:

```text
file.ext
file (1).ext
file (2).ext
```

For deterministic recursive jobs, the planner reserves generated names before any write begins.

---

## 46. Object metadata and properties specification

### 46.1 Properties DTO

```rust
pub struct ObjectProperties {
    pub reference: ObjectReference,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub last_modified: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub version_id: Option<String>,
    pub storage_class: Option<String>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<DateTime<Utc>>,
    pub checksum: Vec<ChecksumValue>,
    pub server_side_encryption: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    pub object_lock: Option<ObjectLockSummary>,
    pub tags_available: bool,
}
```

ETags are displayed without being described as checksums because multipart and provider implementations can produce non-content-hash ETags.

### 46.2 Metadata edit

Editable fields:

- Content-Type
- Content-Disposition
- Cache-Control
- User metadata

Validation:

- Metadata key: 1–128 visible ASCII characters, normalized to provider behavior.
- Metadata value: maximum 2,048 UTF-8 bytes per item.
- Total metadata is validated conservatively against provider limits.
- Header injection characters CR and LF are rejected.

Operation:

```text
1. Head object and capture current metadata and identity.
2. User edits fields.
3. Submit copy-to-self with metadata replacement and copy-source precondition where supported.
4. Verify resulting object properties.
5. Report non-atomic replacement behavior.
```

### 46.3 Folder properties

Folder-style prefix Properties shows:

- Prefix
- Whether an explicit folder marker exists
- Currently known child count and size
- `Calculate` action to run a background recursive scan
- Scan timestamp

A calculated count is a snapshot and MUST not be presented as continuously current.

---

## 47. Preview and share-link specification

### 47.1 Preview classification

Classification uses trusted response metadata plus extension fallback. Content-Type alone is not trusted to render active content.

| Class | Formats | Strategy |
|---|---|---|
| Raster image | JPEG, PNG, GIF, WebP, BMP | Presigned URL or bounded cache; decode safely |
| Text | TXT, JSON, XML, YAML, TOML, INI, LOG, MD | Range/GET up to 2 MiB; render escaped text |
| PDF | PDF | Platform viewer or sandboxed bundled viewer |
| Audio | MP3, WAV, OGG | Presigned streaming URL; cache fallback |
| Video | MP4, WebM | Presigned streaming URL; cache fallback |
| Unsupported | Others | Properties and Download only |

SVG and HTML are treated as text, not rendered markup.

### 47.2 Preview handle

```rust
pub struct PreviewHandle {
    pub id: Uuid,
    pub kind: PreviewKind,
    pub source: PreviewSource,
    pub expires_at: DateTime<Utc>,
    pub content_type: Option<String>,
    pub display_name: String,
    pub truncated: bool,
}

pub enum PreviewSource {
    PresignedUrl { url: SecretString },
    LocalProtocol { token: String },
    InlineText { text: String },
}
```

A presigned URL is never serialized into application logs or persisted. When returned to the frontend for media playback, it is held only in component memory and cleared on close.

### 47.3 Local preview protocol

Cache fallback is exposed through a random opaque token, for example:

```text
s3fm-preview://<random-token>
```

The custom protocol handler maps the token to a Rust-owned cache entry. It does not accept arbitrary paths. Tokens expire after 15 minutes and are single-profile/object scoped.

### 47.4 Cache policy

- Directory: platform cache directory under application ID.
- Default quota: 512 MiB.
- Maximum age: 24 hours.
- Eviction: LRU, skipping in-use entries.
- File names: random UUID, not object key.
- Cache index: SQLite table with token hash, object identity, size, created, last access, and path.
- Secrets and presigned URLs are never stored in cache metadata.

### 47.5 Share links

Share link generation requires provider capability and a selected expiry. UI warning:

> Anyone with this link can access the object until it expires. Treat it like a temporary password.

The application provides Copy Link and Open in Browser. It does not automatically place the URL on clipboard or retain it in history.

---

## 48. Transfer operation semantics

### 48.1 Operation support matrix

| Operation | Queue | Pause | Resume | Retry | Persist after completion |
|---|---:|---:|---:|---:|---:|
| Single PUT upload | Yes | No | No | Restart item | Yes |
| Multipart upload | Yes | Yes | Same session | Failed parts | Yes |
| Single download | Yes | Yes | Validated Range | Continue/restart | Yes |
| Recursive upload | Yes | Yes | Same session | Failed items | Yes |
| Recursive download | Yes | Yes | Same session | Failed items | Yes |
| Single server-side copy | Yes | No | No | Whole request | Yes |
| Recursive copy | Yes | Yes | Same session | Failed items | Yes |
| Single move | Yes | No during copy request | Compound retry | Stage-specific | Yes |
| Recursive move | Yes | Yes between items | Same session | Failed stages | Yes |
| Delete batch | Yes | Between batches | Remaining items | Failed items | Yes |
| Metadata replacement | Yes | No | No | Whole request | Yes |

### 48.2 Transfer state machine

```text
Draft -> Planning -> Queued -> Preparing -> Running
Running -> Pausing -> Paused -> Running
Running -> Retrying -> Running
Running -> Cancelling -> Cancelled
Running -> Completed
Running -> CompletedWithErrors
Running -> Failed
Any nonterminal -> Interrupted on unclean shutdown
```

Invalid transitions return `INVALID_TRANSFER_STATE` and do not mutate persistence.

### 48.3 Job priorities

```text
100 Interactive single-item operations
80  User-started uploads/downloads
60  User-started recursive transfer execution
40  Recursive planning and property calculation
20  Preview cache downloads
10  Cleanup and housekeeping
```

FIFO applies within equal priority. A lower-priority running network request is not preempted; priority affects new scheduling only.

### 48.4 Concurrency

- Global active job semaphore: default 4.
- Per-job multipart part semaphore: default 4.
- Per-profile request semaphore: default 8.
- Preview requests share a separate semaphore of 2.
- Planning requests share a semaphore of 2.

A job captures settings at creation.

### 48.5 Retry schedule

Default attempt delays before jitter:

```text
0.5s, 1s, 2s, 4s, 8s, capped at 30s
```

Full jitter chooses a random delay from zero to the capped delay. Provider `Retry-After` is respected when reasonable. Authentication, validation, access denied, object lock, destination collision, and local disk errors are not retried automatically.

### 48.6 Progress

```rust
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub state: TransferStatus,
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub current_item: Option<String>,
    pub warnings: u32,
    pub failures: u32,
}
```

Progress messages are coalesced to default 5 Hz. Final state is always sent immediately.

### 48.7 Shutdown behavior

When active jobs exist, Close displays:

- Keep app open
- Cancel transfers and exit
- Exit now

`Exit now` marks running jobs Interrupted. It does not promise clean multipart abort. On next startup, the application lists interrupted jobs and orphan cleanup candidates.

---

## 49. Recursive operation planning and persistence

### 49.1 Planning modes

The planner selects one of two modes:

| Estimated item count | Mode | Persistence |
|---:|---|---|
| Up to 100,000 | Materialized | One `transfer_items` row per item before execution |
| Above 100,000 or unknown large | Checkpointed streaming | Page checkpoints plus rows for active, failed, and completed exceptions |

The threshold is an implementation constant in MVP, not user configurable.

### 49.2 Materialized plan

Advantages:

- Exact total item count and known total bytes
- Deterministic collision reservation
- Easy item retry and progress

Planning completes before execution. The user can cancel during planning.

### 49.3 Checkpointed plan

Checkpoint record:

```rust
pub struct PlanningCheckpoint {
    pub transfer_id: Uuid,
    pub source_prefix: String,
    pub continuation_token_encrypted: Option<Vec<u8>>,
    pub page_number: u64,
    pub planned_items: u64,
    pub planned_bytes: u64,
    pub enumeration_complete: bool,
    pub updated_at: DateTime<Utc>,
}
```

Continuation tokens are opaque. Because some providers may encode sensitive internal information, tokens are stored encrypted using a local application data-protection key and are omitted from diagnostics.

The executor keeps a bounded window of planned items, default 2,000. Planning pauses when the execution window is full.

### 49.4 Destination-under-source protection

Before planning recursive copy/move:

```text
source = a/
destination = a/b/
```

The operation is rejected because newly copied objects would re-enter enumeration. A destination equal to source is also rejected except metadata-replacement operations.

### 49.5 Recursive progress

Before enumeration completes, total item count and bytes are displayed as `at least N`. ETA remains hidden. After enumeration completes, totals become exact.

### 49.6 Cost and impact warning

Before large operations the confirmation dialog shows:

- Estimated or exact object count
- Known total bytes
- Request types expected
- Whether operation performs copy and delete
- Whether destination overwrite is possible
- Versioning/object-lock status if known
- Statement that cloud request and transfer charges may apply

The app does not estimate currency cost in MVP.

---

## 50. Typed IPC contract

### 50.1 Contract rules

- Every request and response has `schema_version: u16`, initially `1`.
- Rust types use `serde(rename_all = "camelCase")`.
- TypeScript types are generated from Rust or checked against golden JSON fixtures in CI.
- Unknown response fields are tolerated by the frontend.
- Unknown enum variants map to `unknown` display state rather than causing a crash.
- Secrets use Rust-only types and never implement Serialize.

### 50.2 Common identifiers

```rust
pub type ProfileId = Uuid;
pub type TransferId = Uuid;
pub type BookmarkId = Uuid;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectReference {
    pub profile_id: ProfileId,
    pub bucket: String,
    pub key: String,
    pub version_id: Option<String>,
}

pub struct LocationReference {
    pub profile_id: ProfileId,
    pub bucket: String,
    pub prefix: String,
}
```

### 50.3 Profile DTOs

```rust
pub struct ProfileSummary {
    pub schema_version: u16,
    pub id: ProfileId,
    pub name: String,
    pub provider: ProviderType,
    pub endpoint_display: Option<String>,
    pub region: String,
    pub default_bucket: Option<String>,
    pub root_prefix: String,
    pub favorite: bool,
    pub last_connected_at: Option<DateTime<Utc>>,
    pub credential_state: CredentialState,
    pub connection_state: ConnectionState,
}

pub struct ProfileDraft {
    pub schema_version: u16,
    pub id: Option<ProfileId>,
    pub name: String,
    pub provider: ProviderType,
    pub account_id: Option<String>,
    pub endpoint: Option<String>,
    pub region: String,
    pub addressing_mode: AddressingMode,
    pub default_bucket: Option<String>,
    pub root_prefix: String,
    pub allow_plain_http: bool,
    pub favorite: bool,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<SecretInput>,
    pub session_token: Option<SecretInput>,
}

pub enum SecretInput {
    Unchanged,
    Replace(String),
    Clear,
}

pub struct ConnectionTestResult {
    pub schema_version: u16,
    pub success: bool,
    pub latency_ms: u64,
    pub identity_display: Option<String>,
    pub bucket_access: BucketAccessResult,
    pub capabilities: ProviderCapabilities,
    pub warnings: Vec<PublicWarning>,
}
```

`SecretInput` is accepted by Rust command deserialization but is never echoed. Frontend code clears secret form values immediately after command completion.

### 50.4 Listing DTOs

```rust
pub struct ListEntriesRequest {
    pub schema_version: u16,
    pub location: LocationReference,
    pub continuation_token: Option<String>,
    pub page_size: u16,
    pub request_generation: u64,
}

pub struct EntrySummary {
    pub id: String,
    pub kind: EntryKind,
    pub display_name: String,
    pub key: String,
    pub size: Option<u64>,
    pub last_modified: Option<DateTime<Utc>>,
    pub storage_class: Option<String>,
    pub content_type_hint: Option<String>,
    pub is_folder_marker: bool,
}

pub struct ListEntriesPage {
    pub schema_version: u16,
    pub request_generation: u64,
    pub location: LocationReference,
    pub entries: Vec<EntrySummary>,
    pub next_token: Option<String>,
    pub is_complete: bool,
    pub provider_request_id: Option<String>,
}
```

Continuation tokens are returned only to the frontend location store and MUST NOT be logged.

### 50.5 Transfer request DTOs

```rust
pub struct StartUploadRequest {
    pub schema_version: u16,
    pub local_paths: Vec<String>,
    pub destination: LocationReference,
    pub options: UploadOptions,
    pub progress: tauri::ipc::Channel<TransferChannelMessage>,
}

pub struct UploadOptions {
    pub include_root_folder: bool,
    pub preserve_empty_folders: bool,
    pub include_hidden_system: bool,
    pub follow_links: bool,
    pub collision_policy: CollisionPolicy,
    pub metadata: UploadMetadata,
}

pub struct StartDownloadRequest {
    pub schema_version: u16,
    pub sources: Vec<RemoteSelection>,
    pub destination_directory: String,
    pub options: DownloadOptions,
    pub progress: tauri::ipc::Channel<TransferChannelMessage>,
}

pub struct RemoteSelection {
    pub reference: ObjectReference,
    pub kind: EntryKind,
}

pub enum CollisionPolicy {
    Ask,
    Skip,
    Overwrite,
    Rename,
}
```

Maximum selections in one IPC request: 10,000. Larger loaded selections are converted to a backend-side selection plan before operation start.

### 50.6 Error envelope

```rust
pub struct PublicError {
    pub schema_version: u16,
    pub code: AppErrorCode,
    pub message: String,
    pub retryable: bool,
    pub field_errors: BTreeMap<String, String>,
    pub provider_request_id: Option<String>,
    pub correlation_id: Uuid,
    pub details: BTreeMap<String, PublicValue>,
}
```

`details` is allowlisted per error code. Raw SDK debug strings and URLs are excluded.

### 50.7 Command authorization table

| Command group | Capability | Additional Rust check |
|---|---|---|
| Profile read | `profile-read` | Main window only |
| Profile write | `profile-write` | Local bundled origin; field validation |
| Explorer read | `s3-read` | Profile/root-prefix authorization |
| Transfer write | `s3-write` | Explicit user-selected local paths and remote authorization |
| Delete | `s3-delete` | Confirmation token and selection digest |
| Diagnostics | `diagnostics` | Redaction and user-selected destination |
| Update | `updater` | Signed metadata and no active transfers |

### 50.8 Destructive confirmation token

Delete and move commands receive a short-lived backend-issued token created from:

```text
operation type
profile ID
bucket
normalized selection digest
destination where applicable
expiry
```

The token expires after 60 seconds and prevents a stale or modified frontend dialog from authorizing a different destructive operation.

---

## 51. Settings specification

### 51.1 Settings schema

```rust
pub struct AppSettings {
    pub schema_version: u16,
    pub concurrent_jobs: u8,
    pub per_job_part_concurrency: u8,
    pub per_profile_request_limit: u8,
    pub multipart_threshold_bytes: u64,
    pub initial_part_size_bytes: u64,
    pub retry_limit: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub progress_hz: u8,
    pub default_collision_policy: CollisionPolicy,
    pub preserve_empty_folders: bool,
    pub keep_partial_downloads: bool,
    pub preview_cache_bytes: u64,
    pub preview_cache_max_age_hours: u16,
    pub transfer_history_days: u16,
    pub transfer_history_max_jobs: u32,
    pub log_retention_days: u16,
    pub log_max_bytes: u64,
    pub typed_confirm_object_threshold: u64,
    pub typed_confirm_bytes_threshold: u64,
    pub update_channel: UpdateChannel,
    pub automatic_update_check: bool,
}
```

### 51.2 Defaults and ranges

| Setting | Default | Allowed range |
|---|---:|---:|
| Concurrent jobs | 4 | 1–16 |
| Per-job part concurrency | 4 | 1–16 |
| Per-profile request limit | 8 | 1–32 |
| Multipart threshold | 64 MiB | 16 MiB–5 GiB |
| Initial part size | 16 MiB | 5 MiB–5 GiB |
| Retry limit | 5 | 0–10 |
| Retry base delay | 500 ms | 100–5,000 ms |
| Retry maximum delay | 30 s | 1–120 s |
| Progress frequency | 5 Hz | 1–10 Hz |
| Preview cache | 512 MiB | 64 MiB–10 GiB |
| Preview max age | 24 h | 1–168 h |
| Transfer history | 30 days | 1–365 days |
| History maximum jobs | 1,000 | 100–100,000 |
| Log retention | 14 days | 1–90 days |
| Log total | 100 MiB | 10 MiB–2 GiB |
| Typed confirmation objects | 100 | 1–1,000,000 |
| Typed confirmation bytes | 10 GiB | 1 MiB–1 PiB |

### 51.3 Part-size planner

```text
minimum_required_part_size = ceil(object_size / provider_max_parts)
planned_part_size = max(configured_initial_part_size, minimum_required_part_size, provider_min_part_size)
planned_part_size = round_up_to_mib(planned_part_size)
reject if planned_part_size > provider_max_part_size
```

The last part may be smaller than provider minimum where supported. Provider limits are read from capability configuration, with AWS defaults from current official limits.

### 51.4 Settings application

Settings that affect jobs are copied into `transfer_jobs.settings_snapshot_json`. Changing settings affects only new jobs. View preferences apply immediately. Cache quota reductions trigger asynchronous eviction.

### 51.5 Reset behavior

Reset restores defaults but preserves:

- Profiles and credentials
- Bookmarks and recents
- Transfer history
- Window layout unless `Reset layout` is separately selected

---

## 52. Database completion specification

### 52.1 Additional tables

```sql
CREATE TABLE credential_refs (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    os_target_name TEXT NOT NULL UNIQUE,
    reference_count INTEGER NOT NULL CHECK(reference_count >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE credential_cleanup (
    id TEXT PRIMARY KEY,
    os_target_name TEXT NOT NULL,
    reason TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE planning_checkpoints (
    transfer_id TEXT PRIMARY KEY REFERENCES transfer_jobs(id) ON DELETE CASCADE,
    mode TEXT NOT NULL,
    encrypted_continuation_token BLOB,
    page_number INTEGER NOT NULL DEFAULT 0,
    planned_items INTEGER NOT NULL DEFAULT 0,
    planned_bytes INTEGER NOT NULL DEFAULT 0,
    enumeration_complete INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE TABLE preview_cache (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    bucket_hash TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    object_identity_hash TEXT,
    local_path TEXT NOT NULL UNIQUE,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    last_accessed_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    in_use_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE key_mapping_manifests (
    transfer_id TEXT PRIMARY KEY REFERENCES transfer_jobs(id) ON DELETE CASCADE,
    manifest_path TEXT NOT NULL,
    created_at TEXT NOT NULL
);
```

### 52.2 Transfer item stage fields

`transfer_items` MUST include:

```text
stage
source_identity
planned_destination
collision_resolution
bytes_completed
attempt_count
last_error_code
copy_verified_at
delete_completed_at
cleanup_required
```

### 52.3 Startup recovery

In one startup transaction:

1. Mark jobs in Planning, Preparing, Running, Pausing, Retrying, or Cancelling as Interrupted.
2. Decrement leaked preview cache `in_use_count` values to zero.
3. Queue expired preview entries for deletion.
4. Load credential cleanup work.
5. Detect multipart uploads known to the database but not terminal.
6. Never automatically resume transfer data in the MVP.

### 52.4 Retention cleanup

Cleanup runs after startup and every six hours while the app is open. It deletes in bounded batches to avoid long SQLite locks.

---

## 53. Windows packaging and release specification

### 53.1 Supported environment

```text
OS: Windows 10 version 2004 build 19041 or later; Windows 11
Architecture: x86_64
Rust target: x86_64-pc-windows-msvc
Install scope: per-user
Default install location: Tauri/NSIS per-user application directory
```

### 53.2 Installer behavior

The NSIS installer:

- Installs without elevation under normal per-user flow.
- Creates Start Menu entry.
- Offers optional desktop shortcut.
- Registers uninstall entry.
- Checks WebView2 and uses bootstrapper download mode when missing/outdated.
- Does not delete profiles or application data on normal uninstall unless the user selects `Delete application data`.
- Does not reboot automatically.

### 53.3 Build reproducibility

Release pipeline pins:

- Rust toolchain in `rust-toolchain.toml`
- Cargo dependencies in `Cargo.lock`
- Node package manager and lockfile
- Tauri CLI major/minor version
- NSIS version on the Windows runner

Build metadata includes Git commit, build date, Rust version, frontend package version, and AWS SDK crate version.

### 53.4 Signing

Two independent signatures:

1. Authenticode signs EXE and installer.
2. Tauri updater signing key signs updater bundle and manifest entry.

Private keys are available only in protected release jobs. Pull-request workflows cannot access them.

### 53.5 Update manifest

```json
{
  "version": "1.0.1",
  "notes": "Security and reliability fixes",
  "pub_date": "2026-08-04T04:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<tauri-signature>",
      "url": "https://updates.example.invalid/stable/1.0.1/windows-x86_64.zip"
    }
  }
}
```

The real host is deployment configuration, not hard-coded domain logic. Stable and beta use different manifest URLs. HTTP update URLs are rejected.

### 53.6 Rollback

Automatic downgrade is not supported. A bad release is mitigated by:

- Halting manifest publication
- Publishing a higher patch version containing the rollback fix
- Retaining previous installers for manual support recovery

### 53.7 Privacy

MVP sends no analytics or crash reports. Network calls are limited to configured object-storage endpoints, update manifest/artifact endpoints, and URLs explicitly opened by the user.

---

## 54. Detailed acceptance criteria and test catalog

Each acceptance criterion is mandatory unless marked provider-dependent.

### 54.1 Profiles

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-PRO-001 | Create at least five profiles of mixed provider types and restart; all metadata persists | E2E |
| AC-PRO-002 | Static secret and session token are absent from SQLite, frontend logs, localStorage, diagnostics, and exported JSON | Security test |
| AC-PRO-003 | Duplicate creates a new UUID and honors shared-credential ref count | Integration |
| AC-PRO-004 | Favorite ordering persists across restart | E2E |
| AC-PRO-005 | Editing protected profile fields while a job is active is rejected | Integration |
| AC-PRO-006 | Credential-write success plus SQLite failure triggers secret compensation | Fault injection |
| AC-PRO-007 | Credential replacement leaves no unusable profile during injected cleanup failure | Fault injection |
| AC-PRO-008 | AWS, R2, MinIO, Wasabi preset validation and Custom preset form behavior match Section 42 | UI/integration |
| AC-PRO-009 | Default bucket opens after ListBuckets access denial | Provider integration |
| AC-PRO-010 | Root-prefix escape via UI and direct IPC is rejected | Security test |

### 54.2 Explorer

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-EXP-001 | ListObjectsV2 prefixes and objects render without duplicate folder markers | Integration |
| AC-EXP-002 | Pagination loads at least 2,500 objects in multiple pages without duplication or omission | Integration |
| AC-EXP-003 | Late results from a cancelled location request do not replace current location | Race test |
| AC-EXP-004 | List/Grid modes, columns, and tile behavior match Section 44 | E2E |
| AC-EXP-005 | Ctrl, Shift, Ctrl+A, keyboard navigation, F2, Delete, and Alt+Enter work on loaded entries | E2E |
| AC-EXP-006 | UI clearly states that selection/sort/filter apply to loaded entries while incomplete | UX test |
| AC-EXP-007 | Back/forward history caps at 100 and branch navigation discards forward entries | Unit/E2E |
| AC-EXP-008 | Bookmarks and profile favorites behave independently | E2E |
| AC-EXP-009 | Access-denied, empty, offline, partial-page, filter-empty, and cancelled states render required actions | Component/E2E |
| AC-EXP-010 | Context menu actions reflect type, selection count, and capabilities | Component/E2E |

### 54.3 Upload

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-UP-001 | File below threshold uses single PUT; file above threshold uses multipart | Integration |
| AC-UP-002 | Multipart planner increases part size to stay within provider maximum part count | Unit |
| AC-UP-003 | Four parallel parts transfer without loading the full file into memory | Performance |
| AC-UP-004 | Pause stops new parts and resume finishes in the same session | Integration |
| AC-UP-005 | Cancelling multipart triggers abort or records cleanup candidate | Integration/fault |
| AC-UP-006 | Folder upload preserves root by default and supports contents-only | E2E |
| AC-UP-007 | Empty folders create markers when enabled | Integration |
| AC-UP-008 | Symlink, junction, and reparse-point traversal is skipped by default | Security/filesystem |
| AC-UP-009 | File changed during upload fails with LOCAL_FILE_CHANGED | Fault injection |
| AC-UP-010 | Ask/Skip/Overwrite/Rename and Apply to all behave per job | E2E |
| AC-UP-011 | Content-Type, Content-Disposition, Cache-Control, and user metadata are applied | Integration |

### 54.4 Download

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-DL-001 | Download streams to `.partial` then atomically exposes final file | Integration |
| AC-DL-002 | Cancel deletes partial by default and retains it when configured | Integration |
| AC-DL-003 | Pause/resume uses Range only when identity matches | Integration |
| AC-DL-004 | Changed ETag disables resume and requires restart | Fault injection |
| AC-DL-005 | Path traversal keys cannot escape selected directory | Security |
| AC-DL-006 | Invalid characters, reserved names, trailing dot/space, and case collisions map reversibly | Unit/integration |
| AC-DL-007 | Recursive encoded download writes a correct key-map manifest | Integration |
| AC-DL-008 | Existing-file collision policies behave deterministically | E2E |
| AC-DL-009 | Disk-full error preserves existing destination and reports actionable failure | Fault injection |
| AC-DL-010 | Open destination folder works after completion | E2E |

### 54.5 Copy, move, rename, and delete

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-OP-001 | Single rename executes Copy -> Verify -> Delete | Integration |
| AC-OP-002 | Injected delete failure yields CleanupRequired without deleting destination | Fault injection |
| AC-OP-003 | Retry CleanupRequired retries source deletion only when destination still verifies | Integration |
| AC-OP-004 | Recursive operation reports item and byte progress and retains failures | Integration |
| AC-OP-005 | Destination nested under source is rejected | Unit/E2E |
| AC-OP-006 | Multipart copy is selected when single-copy limit/capability requires it | Integration |
| AC-OP-007 | Cross-profile operation is rejected with explicit unsupported message | E2E |
| AC-DEL-001 | Delete always requires confirmation | E2E |
| AC-DEL-002 | Above threshold requires typed confirmation | E2E |
| AC-DEL-003 | Batch delete respects provider chunk limits and reports individual errors | Integration |
| AC-DEL-004 | Root-prefix deletion guard cannot be bypassed through IPC | Security |
| AC-DEL-005 | Versioning/recoverability wording is conservative when capability is unknown | UX test |

### 54.6 Metadata, preview, and sharing

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-MET-001 | Properties show all available fields and Unknown for absent optional fields | Integration/UI |
| AC-MET-002 | Metadata edit performs self-copy replacement and verifies result | Integration |
| AC-MET-003 | CR/LF and excessive metadata values are rejected | Unit/security |
| AC-PRV-001 | Allowlisted formats preview; unsupported formats show Properties/Download | E2E |
| AC-PRV-002 | HTML and SVG are escaped as text and never execute | Security |
| AC-PRV-003 | Text preview truncates after 2 MiB and indicates truncation | Integration |
| AC-PRV-004 | Audio/video presigned playback expires and handle is revoked on close | Integration |
| AC-PRV-005 | Cache quota and age eviction work without deleting in-use files | Unit/integration |
| AC-PRV-006 | Share link defaults to 1 hour, cannot exceed 7 days, and never appears in logs/history | Security/E2E |

### 54.7 Transfer manager and settings

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-TRN-001 | State machine rejects invalid transitions | Unit |
| AC-TRN-002 | Queue follows priority then FIFO | Unit/integration |
| AC-TRN-003 | Four concurrent jobs keep UI responsive and respect semaphores | Performance/E2E |
| AC-TRN-004 | Retry uses classified errors, capped exponential backoff, and jitter | Unit/fault |
| AC-TRN-005 | Progress is ordered, coalesced to configured rate, and final state immediate | Integration |
| AC-TRN-006 | ETA hides when total or speed confidence is unavailable | Unit/UI |
| AC-TRN-007 | Shutdown choices produce correct terminal/interrupted states | E2E |
| AC-TRN-008 | History retention respects 30 days or 1,000 jobs and never deletes active jobs | Integration |
| AC-SET-001 | Every setting enforces documented range with field errors | Unit/E2E |
| AC-SET-002 | Active jobs retain captured settings after global changes | Integration |
| AC-SET-003 | Reset preserves profiles, credentials, bookmarks, and history | E2E |

### 54.8 Security and packaging

| ID | Acceptance criterion | Verification |
|---|---|---|
| AC-SEC-001 | Main webview loads bundled content only and CSP blocks remote script | Security |
| AC-SEC-002 | Capability files expose only required commands/plugins | Review/test |
| AC-SEC-003 | Direct malformed IPC requests fail Rust validation | Fuzz/security |
| AC-SEC-004 | Diagnostic archive passes seeded secret and presigned URL scan | Security |
| AC-SEC-005 | Plain HTTP requires explicit opt-in; TLS bypass does not exist | E2E/code review |
| AC-PKG-001 | NSIS per-user installer installs on clean Windows 10 build 19041 and Windows 11 | Installer test |
| AC-PKG-002 | Missing WebView2 triggers bootstrapper path and actionable failure offline | Installer test |
| AC-PKG-003 | Tampered updater bundle or manifest signature is rejected | Security |
| AC-PKG-004 | Updater refuses installation while transfers are active | E2E |
| AC-PKG-005 | Uninstall preserves app data unless delete-data option is selected | Installer test |

---

## 55. Requirement-to-design traceability

### 55.1 Traceability rules

Every MVP requirement MUST map to:

1. At least one design section.
2. At least one test or acceptance criterion.
3. An implementation phase.

The CI documentation check validates that every `FR-*` and `NFR-*` ID appears in a machine-readable traceability CSV stored in the repository.

### 55.2 Requirement group matrix

| Requirement group | Design sections | Acceptance groups | Phase |
|---|---|---|---|
| FR-PRO-* | 17, 42, 43, 50, 52 | AC-PRO-* | 1 |
| FR-EXP-* | 14, 19, 44, 50 | AC-EXP-* | 1 |
| FR-UP-* | 21, 22, 45, 48, 49, 50, 51 | AC-UP-* | 2–3 |
| FR-DL-* | 21, 23, 45, 48, 49, 50 | AC-DL-* | 2–3 |
| FR-OP-* | 20, 21, 48, 49 | AC-OP-* | 3 |
| FR-DEL-* | 20, 49, 50 | AC-DEL-* | 3 |
| FR-MET-* | 46, 50 | AC-MET-* | 4 |
| FR-PRV-* | 24, 47, 50, 51 | AC-PRV-* | 4 |
| FR-TRN-* | 21, 48, 49, 51, 52 | AC-TRN-* | 2–3 |
| FR-SET-* | 51, 52 | AC-SET-* | 0–4 |
| FR-PKG-* | 31, 34, 53 | AC-PKG-* | 4 |
| NFR-PERF-* | 21, 30, 48, 49 | AC-TRN-003 and performance tests | All |
| NFR-REL-* | 21–23, 48, 49, 52 | Fault-injection suites | All |
| NFR-SEC-* | 17, 28, 29, 45, 47, 50, 53 | AC-SEC-* | All |
| NFR-UX-* | 14, 44, 46, 47 | AC-EXP, AC-MET, AC-PRV | All |
| NFR-COMP-* | 18, 42 | AC-PRO-008 and provider smoke | 1–4 |
| NFR-MNT-* | 15, 26, 50, 55 | Contract and architecture tests | All |

### 55.3 MVP scope coverage

| MVP feature | Requirement coverage | Detailed design | Acceptance coverage | Status |
|---|---|---|---|---|
| Multiple profiles | FR-PRO-* | 42–43 | AC-PRO-* | Complete |
| AWS/R2/MinIO/Wasabi/Custom | FR-PRO-019, NFR-COMP-* | 42 | AC-PRO-008 | Complete |
| Secure credentials | FR-PRO-013–018, NFR-SEC-* | 17, 43, 50, 52 | AC-PRO-002, AC-SEC-* | Complete |
| Buckets/prefix browsing | FR-EXP-* | 19, 44, 50 | AC-EXP-* | Complete |
| List/grid and keyboard | FR-EXP-011–028 | 44 | AC-EXP-004–010 | Complete |
| Upload files/folders | FR-UP-* | 22, 45, 48–51 | AC-UP-* | Complete |
| Download files/folders | FR-DL-* | 23, 45, 48–50 | AC-DL-* | Complete |
| Copy/move/rename | FR-OP-* | 20, 48–50 | AC-OP-* | Complete |
| Recursive delete | FR-DEL-* | 20, 49–50 | AC-DEL-* | Complete |
| Metadata | FR-MET-* | 46 | AC-MET-* | Complete |
| Preview/share | FR-PRV-* | 47 | AC-PRV-* | Complete |
| Transfer queue | FR-TRN-* | 48–49 | AC-TRN-* | Complete |
| Settings | FR-SET-* | 51 | AC-SET-* | Complete |
| Windows installer/update | FR-PKG-* | 53 | AC-PKG-* | Complete |
| Logging/diagnostics | NFR-SEC-007 and Section 29 | 29, 53 | AC-SEC-004 | Complete |

---

## 56. Final implementation review

### 56.1 Review method

The final review performs four passes:

1. **Scope pass:** Every item in Section 5.1 maps to requirements, design, and acceptance tests.
2. **Consistency pass:** No design contradicts trust boundaries, S3 semantics, provider compatibility, or MVP deferrals.
3. **Implementability pass:** Commands, DTOs, state transitions, persistence, defaults, validation, and UI behavior are sufficiently defined to write code without a new product decision.
4. **Security pass:** Secret handling, path safety, capabilities, destructive confirmation, logging, preview, update, and installer boundaries are explicit.

### 56.2 Findings closed from baseline review

| Baseline gap | Resolution |
|---|---|
| Provider preset details | Section 42 |
| Favorite/duplicate/profile transaction behavior | Section 43 |
| Explorer list/grid/selection/keyboard/context states | Section 44 |
| Folder upload/download mapping and Windows names | Section 45 |
| Metadata contract | Section 46 |
| Preview allowlist/cache/media decision | Section 47 |
| Pause/resume matrix | Section 48 |
| Recursive planning strategy | Section 49 |
| Typed DTO and command authorization | Section 50 |
| Settings defaults/ranges | Section 51 |
| Missing persistence tables/recovery | Section 52 |
| Windows build/signing/update decisions | Section 53 |
| One-to-one acceptance coverage | Sections 54–55 |

### 56.3 Consistency conclusions

- Tauri commands remain request-response boundaries; ordered high-rate transfer updates use channels.
- Permanent credentials remain Rust-side and OS-store backed.
- Presigned URLs are limited to preview/share and treated as bearer secrets.
- S3 prefixes are not represented as atomic directories.
- Move and rename remain Copy -> Verify -> Delete.
- Cross-profile transfer and global recursive search remain outside MVP.
- TLS verification bypass remains unavailable.
- All active transfer behavior snapshots settings.
- Windows filename encoding is reversible and path-safe.
- Preview cannot execute active remote content.
- Installer and updater signatures are independent.

### 56.4 Final sign-off status

**SDD status: APPROVED FOR MVP IMPLEMENTATION.**

No open product, architecture, security, IPC, persistence, UX, transfer, provider, packaging, or acceptance-test decision remains for the defined MVP. Implementation may still discover defects or provider-specific behavior, but such findings are engineering changes governed by the decision log rather than missing SDD scope.

---

## 57. Implementation readiness checklist

Before repository implementation begins:

- [ ] Create Tauri 2 React TypeScript repository.
- [ ] Pin Rust, Node, package manager, Tauri CLI, and lockfiles.
- [ ] Create Rust domain modules independent of Tauri wrappers.
- [ ] Add SQLx migrations matching Sections 25 and 52.
- [ ] Implement `CredentialStore` Windows backend and fault-injection fake.
- [ ] Implement generated/validated Rust-TypeScript IPC schema workflow.
- [ ] Add Tauri capability files and CSP.
- [ ] Add provider preset fixture data and validation tests.
- [ ] Add MinIO test service to CI.
- [ ] Provision separate AWS and R2 smoke-test credentials with least privilege.
- [ ] Add seeded-secret scanning for logs and diagnostics.
- [ ] Add Windows build runner with NSIS and WebView2 installer tests.
- [ ] Configure signing only in protected release environment.
- [ ] Maintain traceability CSV for every requirement ID.

The checklist does not alter scope; it converts the approved design into repository tasks.

---

## 58. Official technical references

1. Tauri — Calling Rust from the Frontend  
   https://v2.tauri.app/develop/calling-rust/
2. Tauri — Capabilities  
   https://v2.tauri.app/security/capabilities/
3. Tauri — Windows Installer  
   https://v2.tauri.app/distribute/windows-installer/
4. Tauri — Stronghold plugin  
   https://v2.tauri.app/plugin/stronghold/
5. Tauri — Updater plugin  
   https://v2.tauri.app/plugin/updater/
6. AWS SDK for Rust — Credential providers  
   https://docs.aws.amazon.com/sdk-for-rust/latest/dg/credproviders.html
7. AWS SDK for Rust — Amazon S3 examples  
   https://docs.aws.amazon.com/sdk-for-rust/latest/dg/rust_s3_code_examples.html
8. AWS SDK for Rust — Presigned URLs  
   https://docs.aws.amazon.com/sdk-for-rust/latest/dg/presigned-urls.html
9. AWS SDK for Rust — Error handling  
   https://docs.aws.amazon.com/sdk-for-rust/latest/dg/error-handling.html
10. Amazon S3 — ListObjectsV2  
    https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html
11. Amazon S3 — Copying, moving, and renaming objects  
    https://docs.aws.amazon.com/AmazonS3/latest/userguide/copy-object.html
12. Amazon S3 — HeadBucket  
    https://docs.aws.amazon.com/AmazonS3/latest/API/API_HeadBucket.html
13. Tauri — Calling the Frontend from Rust (events and channels)  
    https://v2.tauri.app/develop/calling-frontend/
14. Tauri — Prerequisites  
    https://v2.tauri.app/start/prerequisites/
15. Amazon S3 — Multipart upload limits  
    https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
16. Amazon S3 — Multipart upload overview  
    https://docs.aws.amazon.com/AmazonS3/latest/userguide/mpuoverview.html
17. Cloudflare R2 — AWS SDK for Rust  
    https://developers.cloudflare.com/r2/examples/aws/aws-sdk-rust/
18. Cloudflare R2 — S3 API compatibility  
    https://developers.cloudflare.com/r2/api/s3/api/
19. MinIO — S3 API compatibility  
    https://docs.min.io/aistor/developers/s3-api-compatibility/
20. Microsoft — Credentials Management API  
    https://learn.microsoft.com/en-us/windows/win32/secauthn/credentials-management

---

**End of document**
