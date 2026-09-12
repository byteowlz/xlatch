BEGIN IMMEDIATE;
ALTER TABLE devices ADD COLUMN enrollment_status TEXT NOT NULL DEFAULT 'active' CHECK(enrollment_status IN ('active','pending','rejected'));
CREATE TABLE enrollment_policy(singleton INTEGER PRIMARY KEY CHECK(singleton=1), server_id TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 0);
INSERT INTO enrollment_policy(singleton,server_id) VALUES(1,lower(hex(randomblob(32))));
CREATE TABLE enrollment_bootstrap(device_id TEXT PRIMARY KEY REFERENCES devices(id), token_hash TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE approvers(device_id TEXT PRIMARY KEY REFERENCES devices(id), public_key TEXT NOT NULL);
CREATE TABLE pending_enrollments(device_id TEXT PRIMARY KEY REFERENCES devices(id), payload TEXT NOT NULL, expires_at INTEGER NOT NULL, decision TEXT);
PRAGMA user_version=2;
COMMIT;
