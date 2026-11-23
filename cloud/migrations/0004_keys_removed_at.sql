-- When the user explicitly removed their keys from the account (cleared again by the next key upload).
ALTER TABLE users ADD COLUMN keys_removed_at INTEGER;
