ALTER TABLE connection_profiles ADD COLUMN favorite_order INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS credential_refs (
    secret_reference TEXT PRIMARY KEY NOT NULL,
    profile_count INTEGER NOT NULL DEFAULT 0 CHECK (profile_count >= 0),
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS credential_cleanup (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    secret_reference TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_error TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_credential_cleanup_pending
    ON credential_cleanup(attempt_count, created_at);

-- Backfill reference counts for profiles created by migration 0001.  Without
-- this repair, deleting the first upgraded profile could incorrectly treat a
-- shared secret as unreferenced and remove it from the OS vault.
INSERT INTO credential_refs (secret_reference, profile_count, updated_at)
SELECT secret_reference, COUNT(*), datetime('now')
FROM connection_profiles
WHERE secret_reference IS NOT NULL
GROUP BY secret_reference
ON CONFLICT(secret_reference) DO UPDATE SET
    profile_count = excluded.profile_count,
    updated_at = excluded.updated_at;

INSERT INTO credential_refs (secret_reference, profile_count, updated_at)
SELECT session_reference, COUNT(*), datetime('now')
FROM connection_profiles
WHERE session_reference IS NOT NULL
GROUP BY session_reference
ON CONFLICT(secret_reference) DO UPDATE SET
    profile_count = credential_refs.profile_count + excluded.profile_count,
    updated_at = excluded.updated_at;
