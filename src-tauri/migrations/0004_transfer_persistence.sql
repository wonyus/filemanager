-- Durable transfer snapshots.  The request and job JSON are intentionally
-- redacted snapshots: credentials and destructive confirmation tokens never
-- cross into the transfer history database.
ALTER TABLE transfers ADD COLUMN request_json TEXT;
ALTER TABLE transfers ADD COLUMN job_json TEXT;

CREATE INDEX IF NOT EXISTS idx_transfers_updated_at
    ON transfers(updated_at DESC);
