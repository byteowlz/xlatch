BEGIN IMMEDIATE;
CREATE TABLE history_policy(singleton INTEGER PRIMARY KEY CHECK(singleton=1), config TEXT NOT NULL);
CREATE TABLE routing_history(job_id TEXT PRIMARY KEY REFERENCES jobs(id), owner TEXT NOT NULL, created_at INTEGER NOT NULL, record TEXT NOT NULL);
CREATE INDEX routing_history_age ON routing_history(created_at,job_id);
PRAGMA user_version=5;
COMMIT;
