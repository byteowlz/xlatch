BEGIN IMMEDIATE;
CREATE TABLE parked_items(
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    label TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    input TEXT NOT NULL,
    upload_id TEXT REFERENCES uploads(id),
    created_at INTEGER NOT NULL
);
CREATE INDEX parked_items_owner ON parked_items(owner, created_at DESC);
PRAGMA user_version=11;
COMMIT;
