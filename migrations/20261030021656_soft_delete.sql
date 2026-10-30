-- Add migration script here
ALTER TABLE posts ADD COLUMN deleted_at TEXT;
