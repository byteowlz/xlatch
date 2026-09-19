//! Private `SQLite` storage and atomic capability/job transitions.

use crate::capability::{Capability, Event, Execution, Job, Manifest};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;

/// `SQLite` connection confined to a service call or worker operation.
#[derive(Debug)]
pub struct Store {
    pub(crate) conn: Connection,
}

/// Unix timestamp in seconds.
#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Store {
    /// Open an initialized database. Initialization belongs to the daemon.
    ///
    /// # Errors
    /// Returns filesystem/SQLite errors.
    pub fn open(dir: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            dir.join("xlatch.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn })
    }

    /// Create a private service directory and the v0 schema.
    ///
    /// # Errors
    /// Returns filesystem/SQLite errors, including unsupported future schemas.
    pub fn initialize(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let conn = Connection::open(dir.join("xlatch.sqlite3"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(version <= 6, "database was created by a newer xlatch");
        conn.pragma_update(None, "foreign_keys", true)?;
        if version == 0 {
            conn.execute_batch(include_str!("../migrations/001.sql"))?;
        }
        if version < 2 {
            conn.execute_batch(include_str!("../migrations/002.sql"))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        if version < 3 {
            conn.execute_batch(include_str!("../migrations/003.sql"))?;
        }
        if version < 4 {
            conn.execute_batch(include_str!("../migrations/004.sql"))?;
        }
        if version < 5 {
            conn.execute_batch(include_str!("../migrations/005.sql"))?;
        }
        if version < 6 {
            conn.execute_batch(include_str!("../migrations/006.sql"))?;
        }
        Ok(Self { conn })
    }

    /// Register a revision without granting execution permission.
    ///
    /// # Errors
    /// Returns validation or storage errors.
    pub fn register(&self, manifest: &Manifest) -> Result<Capability> {
        manifest.validate()?;
        let revision = manifest.revision()?;
        self.conn.execute("INSERT INTO capabilities(id,revision,manifest,status) VALUES(?1,?2,?3,'pending') ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,manifest=excluded.manifest,status=CASE WHEN revision=excluded.revision THEN status ELSE 'pending' END", params![manifest.id, revision, serde_json::to_string(&manifest)?])?;
        self.capability(&manifest.id)
    }

    /// Fetch the latest revision, including pending registrations.
    ///
    /// # Errors
    /// Returns missing-capability or storage errors.
    pub fn capability(&self, id: &str) -> Result<Capability> {
        let (body, revision, status): (String, String, String) = self
            .conn
            .query_row(
                "SELECT manifest,revision,status FROM capabilities WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .context("capability not found")?;
        Ok(Capability {
            manifest: serde_json::from_str(&body)?,
            revision,
            status,
        })
    }

    /// Activate exactly the reviewed revision.
    ///
    /// # Errors
    /// Rejects stale approvals and host commands without explicit consent.
    pub fn approve(
        &self,
        id: &str,
        revision: &str,
        allow_host_execution: bool,
    ) -> Result<Capability> {
        let capability = self.capability(id)?;
        ensure!(
            capability.revision == revision,
            "revision changed; review again"
        );
        ensure!(
            !matches!(capability.manifest.execution, Execution::Command { .. })
                || allow_host_execution,
            "command executes with the daemon user's privileges; approval requires --allow-host-execution"
        );
        capability.manifest.validate()?;
        capability.manifest.validate_host_binding()?;
        let count = self.conn.execute(
            "UPDATE capabilities SET status='active' WHERE id=?1 AND revision=?2",
            params![id, revision],
        )?;
        ensure!(count == 1, "revision changed during approval");
        self.capability(id)
    }

    /// Grant an approved revision to an already paired device.
    ///
    /// # Errors
    /// Rejects unknown or revoked devices and pending or stale capability revisions.
    pub fn grant(&self, device: &str, id: &str, revision: &str) -> Result<()> {
        let changed = self.conn.execute(
            "INSERT INTO grants(device_id,capability_id,revision) SELECT d.id,c.id,c.revision FROM devices d JOIN capabilities c ON c.id=?2 WHERE d.id=?1 AND d.revoked=0 AND d.enrollment_status='active' AND c.status='active' AND c.revision=?3 ON CONFLICT(device_id,capability_id) DO UPDATE SET revision=excluded.revision",
            params![device, id, revision],
        )?;
        ensure!(
            changed == 1,
            "grant requires an active device and an approved, current capability revision"
        );
        Ok(())
    }

    /// List registrations for a local operator or scoped device.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn discover(&self, owner: &str) -> Result<Vec<Capability>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM capabilities ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| self.capability(&id))
            .filter_map(|r| match r {
                Ok(cap)
                    if owner == "local"
                        || (cap.status == "active"
                            && self
                                .has_grant(owner, &cap.manifest.id, &cap.revision)
                                .unwrap_or(false)) =>
                {
                    Some(Ok(cap))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    pub(crate) fn has_grant(&self, owner: &str, id: &str, revision: &str) -> Result<bool> {
        if owner == "local" {
            return Ok(true);
        }
        Ok(self.conn.query_row("SELECT count(*) FROM grants g JOIN devices d ON d.id=g.device_id WHERE g.device_id=?1 AND g.capability_id=?2 AND g.revision=?3 AND d.revoked=0 AND d.enrollment_status='active'", params![owner,id,revision], |r| r.get::<_, i64>(0))? > 0)
    }

    /// Atomically check the contract, deduplicate, and enqueue execution.
    ///
    /// # Errors
    /// Rejects unapproved/unauthorized revisions, invalid input, and key conflicts.
    pub fn invoke(
        &mut self,
        owner: &str,
        id: &str,
        revision: &str,
        input: &Value,
        key: &str,
    ) -> Result<Job> {
        ensure!(
            !key.is_empty() && key.len() <= 120,
            "invalid idempotency key"
        );
        // The immediate transaction prevents registration/revocation racing acceptance.
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let outcome = self.enqueue(owner, id, revision, input, key);
        match outcome {
            Ok(job) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(job)
            }
            Err(error) => {
                self.conn.execute_batch("ROLLBACK")?;
                Err(error)
            }
        }
    }

    fn enqueue(
        &self,
        owner: &str,
        id: &str,
        revision: &str,
        input: &Value,
        key: &str,
    ) -> Result<Job> {
        let capability = self.capability(id)?;
        ensure!(
            capability.status == "active" && capability.revision == revision,
            "capability revision is not active"
        );
        ensure!(
            self.has_grant(owner, id, revision)?,
            "capability permission denied"
        );
        let validator = jsonschema::validator_for(&capability.manifest.input_schema)?;
        ensure!(
            validator.is_valid(input),
            "input does not match capability schema"
        );
        if let Some(job_id) = self
            .conn
            .query_row(
                "SELECT id FROM jobs WHERE owner=?1 AND idempotency_key=?2",
                params![owner, key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            let job = self.job(owner, &job_id)?;
            ensure!(
                job.capability_id == id && job.revision == revision && job.input == *input,
                "idempotency key already used with different input"
            );
            return Ok(job);
        }
        let queued: i64 = self.conn.query_row(
            "SELECT count(*) FROM jobs WHERE status IN ('queued','running')",
            [],
            |r| r.get(0),
        )?;
        ensure!(queued < 1000, "job queue is full");
        let job_id = uuid::Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO jobs(id,capability_id,revision,manifest,owner,status,input,idempotency_key,created_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8)", params![job_id,id,revision,serde_json::to_string(&capability.manifest)?,owner,serde_json::to_string(&input)?,key,now()])?;
        self.capture_history(&job_id, owner, &capability, input)?;
        self.event(&job_id, owner, "queued")?;
        self.job(owner, &job_id)
    }

    /// Fetch a job only for its owner, or the trusted local operator.
    ///
    /// # Errors
    /// Returns missing/unauthorized job errors.
    pub fn job(&self, owner: &str, id: &str) -> Result<Job> {
        let mut stmt = self.conn.prepare("SELECT id,capability_id,revision,owner,status,input,result,error,created_at FROM jobs WHERE id=?1 AND (owner=?2 OR ?2='local')")?;
        let tuple = stmt
            .query_row(params![id, owner], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .context("job not found")?;
        Ok(Job {
            id: tuple.0,
            capability_id: tuple.1,
            revision: tuple.2,
            owner: tuple.3,
            status: tuple.4,
            input: serde_json::from_str(&tuple.5)?,
            result: tuple.6.map(|s| serde_json::from_str(&s)).transpose()?,
            error: tuple.7,
            created_at: tuple.8,
        })
    }

    /// List at most 100 recent owned jobs.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn jobs(&self, owner: &str) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare("SELECT id FROM jobs WHERE owner=?1 OR ?1='local' ORDER BY created_at DESC,rowid DESC LIMIT 100")?;
        let ids = stmt
            .query_map([owner], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| {
                let mut job = self.job(owner, id)?;
                job.input = Value::Null;
                job.result = None;
                Ok(job)
            })
            .collect()
    }

    /// Persist cancellation; the worker checks it while the child runs.
    ///
    /// # Errors
    /// Returns unauthorized or storage errors.
    pub fn cancel(&self, owner: &str, id: &str) -> Result<Job> {
        let job = self.job(owner, id)?;
        let changed = self.conn.execute(
            "UPDATE jobs SET status='cancelled' WHERE id=?1 AND status IN ('queued','running')",
            [id],
        )?;
        if changed > 0 {
            self.event(id, &job.owner, "cancelled")?;
        }
        self.job(owner, id)
    }

    pub(crate) fn event(&self, id: &str, owner: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events(job_id,owner,status) VALUES(?1,?2,?3)",
            params![id, owner, status],
        )?;
        Ok(())
    }

    /// Fetch ordered events after a durable cursor.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn events(&self, owner: &str, after: i64) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare("SELECT sequence,job_id,status FROM events WHERE sequence>?1 AND (owner=?2 OR ?2='local') ORDER BY sequence LIMIT 100")?;
        Ok(stmt
            .query_map(params![after, owner], |r| {
                Ok(Event {
                    sequence: r.get(0)?,
                    job_id: r.get(1)?,
                    status: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
}
