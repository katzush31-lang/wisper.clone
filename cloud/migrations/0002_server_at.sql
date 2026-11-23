-- Sync cursors use the server's clock: server_at = Worker time when the row was last written.
ALTER TABLE items ADD COLUMN server_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats ADD COLUMN server_at INTEGER NOT NULL DEFAULT 0;
UPDATE items SET server_at = updated_at;
UPDATE stats SET server_at = COALESCE(created_at, 0);
CREATE INDEX idx_items_user_server ON items(user_id, server_at);
CREATE INDEX idx_stats_user_server ON stats(user_id, server_at);
