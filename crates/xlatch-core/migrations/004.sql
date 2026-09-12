BEGIN IMMEDIATE;
CREATE TABLE capability_approvals(
 id TEXT PRIMARY KEY,
 owner TEXT NOT NULL REFERENCES devices(id),
 payload TEXT NOT NULL,
 expires_at INTEGER NOT NULL,
 decision TEXT,
 signature TEXT,
 decided_at INTEGER
);
PRAGMA user_version=4;
COMMIT;
