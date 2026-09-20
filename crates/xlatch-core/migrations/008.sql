BEGIN IMMEDIATE;
CREATE TABLE transient_compositions(parent_id TEXT PRIMARY KEY REFERENCES jobs(id));
PRAGMA user_version=8;
COMMIT;
