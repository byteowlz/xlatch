//! Bounded durable job execution. Interrupted work fails rather than replaying side effects.

use crate::{
    capability::{Job, Manifest},
    store::Store,
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

impl Store {
    /// Mark previously running jobs failed after obtaining the daemon's exclusive lock.
    ///
    /// # Errors
    /// Returns `SQLite` errors.
    pub fn recover(&mut self) -> Result<()> {
        self.fail_interrupted(
            None,
            "Daemon restarted during execution; review side effects before retrying",
        )
    }

    pub(crate) fn fail_interrupted(
        &mut self,
        expired_before: Option<i64>,
        reason: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'failed' FROM jobs WHERE status='running' AND (?1 IS NULL OR id IN (SELECT job_id FROM executor_leases WHERE expires_at<?1))",[expired_before])?;
        tx.execute("UPDATE jobs SET status='failed',error=?2 WHERE status='running' AND (?1 IS NULL OR id IN (SELECT job_id FROM executor_leases WHERE expires_at<?1))",params![expired_before,reason])?;
        tx.execute(
            "DELETE FROM executor_leases WHERE ?1 IS NULL OR expires_at<?1",
            [expired_before],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn claim(&mut self, lease: Option<&str>) -> Result<Option<(Job, Manifest)>> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let selected:Option<(String,String)>=tx.query_row("SELECT id,manifest FROM jobs WHERE status='queued' ORDER BY created_at,rowid LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((id, body)) = selected else {
            return Ok(None);
        };
        tx.execute("UPDATE jobs SET status='running' WHERE id=?1", [&id])?;
        tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'running' FROM jobs WHERE id=?1",[&id])?;
        let manifest: Manifest = serde_json::from_str(&body)?;
        if let Some(lease) = lease {
            tx.execute(
                "INSERT INTO executor_leases VALUES(?1,?2,?3)",
                params![
                    id,
                    crate::capability::digest(lease.as_bytes()),
                    crate::store::now() + i64::try_from(manifest.timeout_seconds)? + 30
                ],
            )?;
        }
        tx.commit()?;
        Ok(Some((self.job("local", &id)?, manifest)))
    }

    pub(crate) fn finish(&mut self, job: &Job, outcome: Result<Value>) -> Result<()> {
        let (status, result, error) = match outcome {
            Ok(value) => ("succeeded", Some(serde_json::to_string(&value)?), None),
            Err(error) => (
                "failed",
                None,
                Some(format!("{error:#}").chars().take(4000).collect::<String>()),
            ),
        };
        let tx = self.conn.transaction()?;
        if tx.execute(
            "UPDATE jobs SET status=?2,result=?3,error=?4 WHERE id=?1 AND status='running'",
            params![job.id, status, result, error],
        )? > 0
        {
            tx.execute(
                "INSERT INTO events(job_id,owner,status) VALUES(?1,?2,?3)",
                params![job.id, job.owner, status],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Run a single worker loop. Start a bounded number of these per daemon.
///
/// # Errors
/// Returns storage failures; job failures are persisted and do not stop the worker.
pub async fn run(dir: PathBuf) -> Result<()> {
    loop {
        let mut store = Store::open(&dir)?;
        if let Some((job, manifest)) = store.claim(None)? {
            let outcome = execute(&dir, &job, &manifest).await;
            store.finish(&job, outcome)?;
        } else {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}

async fn execute(dir: &Path, job: &Job, manifest: &Manifest) -> Result<Value> {
    let store = Store::open(dir)?;
    let current = store.capability(&job.capability_id)?;
    ensure!(
        current.status == "active"
            && current.revision == job.revision
            && store.has_grant(&job.owner, &job.capability_id, &job.revision)?,
        "approval or permission changed before execution"
    );
    crate::execution::execute(dir, job, manifest, wait_for_cancellation(dir, &job.id)).await
}

async fn wait_for_cancellation(dir: &Path, id: &str) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if Store::open(dir)?.job("local", id)?.status == "cancelled" {
            return Ok(());
        }
    }
}
