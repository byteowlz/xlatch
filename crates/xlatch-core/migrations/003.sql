BEGIN IMMEDIATE;
CREATE TABLE executor_leases(job_id TEXT PRIMARY KEY REFERENCES jobs(id), token_hash TEXT NOT NULL, expires_at INTEGER NOT NULL);
PRAGMA user_version=3;
COMMIT;
