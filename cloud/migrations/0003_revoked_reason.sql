-- Why a device was revoked: 'reuse' (refresh token replayed) or 'removed' (logout, removed, device cap).
ALTER TABLE devices ADD COLUMN revoked_reason TEXT;
UPDATE devices SET revoked_reason = 'removed' WHERE revoked = 1;
