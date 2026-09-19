BEGIN IMMEDIATE;
CREATE TABLE approver_reviews(id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), payload TEXT NOT NULL, expires_at INTEGER NOT NULL);
PRAGMA user_version=6;
COMMIT;
