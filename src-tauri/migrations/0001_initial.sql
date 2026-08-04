CREATE TABLE IF NOT EXISTS connection_profiles (
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
    allow_insecure_http INTEGER NOT NULL DEFAULT 0 CHECK (allow_insecure_http IN (0, 1)),
    favorite INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    last_connected_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profile_capabilities (
    profile_id TEXT PRIMARY KEY NOT NULL,
    capabilities_json TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS recent_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id TEXT NOT NULL,
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL,
    opened_at TEXT NOT NULL,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS bookmarks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id TEXT NOT NULL,
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(profile_id, bucket, prefix),
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS transfers (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT,
    operation TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(profile_id) REFERENCES connection_profiles(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS transfer_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transfer_id TEXT NOT NULL,
    source_key TEXT,
    destination_key TEXT,
    state TEXT NOT NULL,
    bytes_total INTEGER,
    bytes_completed INTEGER NOT NULL DEFAULT 0,
    error_code TEXT,
    error_message TEXT,
    FOREIGN KEY(transfer_id) REFERENCES transfers(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_connection_profiles_favorite_name
    ON connection_profiles(favorite DESC, name COLLATE NOCASE ASC);
CREATE INDEX IF NOT EXISTS idx_recent_locations_profile_opened
    ON recent_locations(profile_id, opened_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfer_items_transfer
    ON transfer_items(transfer_id);
