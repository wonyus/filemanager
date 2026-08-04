-- Durable Explorer bookmarks and recent locations.
-- Migration 0001 created these tables before the richer Explorer API existed;
-- this migration adds deterministic bookmark ordering and a uniqueness
-- constraint so recording a location updates its existing history entry.
ALTER TABLE bookmarks ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;

-- Older development databases may contain duplicate recent entries because
-- migration 0001 did not enforce uniqueness. Keep the newest row per location
-- before creating the unique index required by the upsert path.
DELETE FROM recent_locations
WHERE id NOT IN (
    SELECT MAX(id)
    FROM recent_locations
    GROUP BY profile_id, bucket, prefix
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_recent_locations_unique
    ON recent_locations(profile_id, bucket, prefix);
CREATE INDEX IF NOT EXISTS idx_bookmarks_profile_order
    ON bookmarks(profile_id, sort_order ASC, name COLLATE NOCASE ASC, id ASC);
