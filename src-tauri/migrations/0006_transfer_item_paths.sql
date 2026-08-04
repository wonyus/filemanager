-- Keep local filesystem identities separate from remote object keys so the
-- transfer details view can reconstruct both sides without guesswork.
ALTER TABLE transfer_items ADD COLUMN local_path TEXT;

CREATE INDEX IF NOT EXISTS idx_transfer_items_transfer_item_identity
    ON transfer_items(transfer_id, source_key, destination_key, local_path);
