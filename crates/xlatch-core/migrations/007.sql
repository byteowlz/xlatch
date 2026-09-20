BEGIN IMMEDIATE;
CREATE TABLE composition_steps(parent_id TEXT NOT NULL REFERENCES jobs(id), position INTEGER NOT NULL, child_id TEXT NOT NULL UNIQUE REFERENCES jobs(id), PRIMARY KEY(parent_id,position));
PRAGMA user_version=7;
COMMIT;
