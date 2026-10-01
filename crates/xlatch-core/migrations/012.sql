BEGIN IMMEDIATE;
ALTER TABLE parked_items ADD COLUMN preparation_capability_id TEXT;
ALTER TABLE parked_items ADD COLUMN preparation_revision TEXT;
ALTER TABLE parked_items ADD COLUMN preparation_job_id TEXT REFERENCES jobs(id);
ALTER TABLE parked_items ADD COLUMN preparation_error TEXT;
PRAGMA user_version=12;
COMMIT;
