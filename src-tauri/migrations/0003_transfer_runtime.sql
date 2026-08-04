-- Phase 2 transfer runtime.  Migration 0001 contains the initial lightweight
-- transfer tables; these additive columns preserve existing history while
-- providing the durable fields required by the scheduler and retry UI.
ALTER TABLE transfers ADD COLUMN source_json TEXT;
ALTER TABLE transfers ADD COLUMN destination_json TEXT;
ALTER TABLE transfers ADD COLUMN status TEXT NOT NULL DEFAULT 'queued';
ALTER TABLE transfers ADD COLUMN collision_policy TEXT NOT NULL DEFAULT 'ask';
ALTER TABLE transfers ADD COLUMN total_bytes INTEGER;
ALTER TABLE transfers ADD COLUMN transferred_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfers ADD COLUMN total_items INTEGER;
ALTER TABLE transfers ADD COLUMN completed_items INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfers ADD COLUMN failed_items INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfers ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfers ADD COLUMN speed_bps INTEGER;
ALTER TABLE transfers ADD COLUMN eta_seconds INTEGER;
ALTER TABLE transfers ADD COLUMN public_error_json TEXT;
ALTER TABLE transfers ADD COLUMN settings_snapshot_json TEXT;
ALTER TABLE transfers ADD COLUMN profile_snapshot_json TEXT;
ALTER TABLE transfers ADD COLUMN started_at TEXT;
ALTER TABLE transfers ADD COLUMN finished_at TEXT;

-- Migration 0001 called this field `state`; preserve it when the richer
-- `status` projection is introduced so existing history is never reopened as
-- a queued job after upgrade.
UPDATE transfers
SET status = CASE state
    WHEN 'running' THEN 'running'
    WHEN 'completed' THEN 'completed'
    WHEN 'failed' THEN 'failed'
    WHEN 'cancelled' THEN 'cancelled'
    WHEN 'canceling' THEN 'cancelling'
    WHEN 'paused' THEN 'paused'
    ELSE status
END
WHERE state IS NOT NULL;

ALTER TABLE transfer_items ADD COLUMN stage TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE transfer_items ADD COLUMN size_bytes INTEGER;
ALTER TABLE transfer_items ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfer_items ADD COLUMN public_error_json TEXT;
ALTER TABLE transfer_items ADD COLUMN planned_destination TEXT;
ALTER TABLE transfer_items ADD COLUMN collision_resolution TEXT;
ALTER TABLE transfer_items ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE transfer_items ADD COLUMN last_error_code TEXT;
ALTER TABLE transfer_items ADD COLUMN copy_verified_at TEXT;
ALTER TABLE transfer_items ADD COLUMN delete_completed_at TEXT;
ALTER TABLE transfer_items ADD COLUMN cleanup_required INTEGER NOT NULL DEFAULT 0 CHECK (cleanup_required IN (0, 1));

CREATE TABLE IF NOT EXISTS multipart_uploads (
    transfer_id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT,
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    upload_id TEXT NOT NULL,
    part_size INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(transfer_id) REFERENCES transfers(id) ON DELETE CASCADE,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS multipart_parts (
    transfer_id TEXT NOT NULL,
    part_number INTEGER NOT NULL,
    etag TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    completed_at TEXT NOT NULL,
    PRIMARY KEY(transfer_id, part_number),
    FOREIGN KEY(transfer_id) REFERENCES multipart_uploads(transfer_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS planning_checkpoints (
    transfer_id TEXT PRIMARY KEY NOT NULL,
    mode TEXT NOT NULL,
    encrypted_continuation_token BLOB,
    page_number INTEGER NOT NULL DEFAULT 0,
    planned_items INTEGER NOT NULL DEFAULT 0,
    planned_bytes INTEGER NOT NULL DEFAULT 0,
    enumeration_complete INTEGER NOT NULL DEFAULT 0 CHECK (enumeration_complete IN (0, 1)),
    updated_at TEXT NOT NULL,
    FOREIGN KEY(transfer_id) REFERENCES transfers(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS key_mapping_manifests (
    transfer_id TEXT PRIMARY KEY NOT NULL,
    manifest_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(transfer_id) REFERENCES transfers(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_transfers_status_created
    ON transfers(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfers_profile_created
    ON transfers(profile_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfer_items_status
    ON transfer_items(transfer_id, state);
