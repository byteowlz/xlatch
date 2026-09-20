//! Leased execution requests for a separate, unprivileged worker process.

use crate::{
    capability::{Job, Manifest, digest},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Internal worker protocol, available only to the configured executor OS identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutorRequest {
    /// Lease the next authorized job.
    Claim,
    /// Return a result for a job leased to this executor.
    Complete {
        /// Job id.
        id: String,
        /// Unpredictable lease secret returned by claim.
        lease: String,
        /// Exactly one result or error must be present.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_result"
        )]
        result: Option<Value>,
        /// Bounded diagnostic; not notification content.
        error: Option<String>,
    },
    /// Check cancellation while executing a leased job.
    Status {
        /// Job id.
        id: String,
        /// Lease secret.
        lease: String,
    },
}

/// Authorized work handed to the unprivileged executor, without approval-store access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Work {
    /// Durable job and input.
    pub job: Job,
    /// Exact manifest authorized for the job.
    pub manifest: Manifest,
    /// Lease secret bound to this job.
    pub lease: String,
}

impl Store {
    fn lease_work(&mut self) -> Result<Option<Work>> {
        let lease = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let Some((job, manifest)) = self.claim(Some(&lease))? else {
            return Ok(None);
        };
        if let Err(error) = self.authorize_job(&job) {
            self.finish(&job, Err(error))?;
            return Ok(None);
        }
        Ok(Some(Work {
            job,
            manifest,
            lease,
        }))
    }

    fn leased_job(&self, id: &str, lease: &str) -> Result<Job> {
        ensure!(lease.len() == 64, "invalid executor lease");
        let valid: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM executor_leases WHERE job_id=?1 AND token_hash=?2 AND expires_at>=?3)", params![id,digest(lease.as_bytes()),now()], |r| r.get(0))?;
        ensure!(valid, "unknown or expired executor lease");
        self.job("local", id)
    }

    fn complete_work(
        &mut self,
        id: &str,
        lease: &str,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<Value> {
        let job = self.leased_job(id, lease)?;
        ensure!(
            matches!(job.status.as_str(), "running" | "cancelled"),
            "job already finished"
        );
        let outcome = match (result, error) {
            (Some(value), None) => {
                ensure!(
                    serde_json::to_vec(&value)?.len() <= 6 * 1024 * 1024,
                    "executor result exceeds 6 MiB"
                );
                let body: String =
                    self.conn
                        .query_row("SELECT manifest FROM jobs WHERE id=?1", [id], |r| r.get(0))?;
                let manifest: Manifest = serde_json::from_str(&body)?;
                ensure!(
                    jsonschema::validator_for(&manifest.output_schema)?.is_valid(&value),
                    "executor result does not match output schema"
                );
                Ok(value)
            }
            (None, Some(error)) => {
                ensure!(error.len() <= 4000, "executor error too long");
                Err(anyhow::anyhow!(error))
            }
            _ => anyhow::bail!("provide exactly one executor result or error"),
        };
        self.finish(&job, outcome)?;
        self.conn
            .execute("DELETE FROM executor_leases WHERE job_id=?1", [id])?;
        Ok(json!({"recorded":true}))
    }

    /// Fail expired leases rather than repeating potentially irreversible effects.
    ///
    /// # Errors
    /// Returns storage errors.
    pub fn expire_executor_leases(&mut self) -> Result<()> {
        self.fail_interrupted(
            Some(now()),
            "Executor lease expired; review side effects before retrying",
        )
    }
}

/// Dispatch after the transport has verified the configured executor OS identity.
///
/// # Errors
/// Returns lease, permission, schema, or persistence errors.
pub fn dispatch(store: &mut Store, request: ExecutorRequest) -> Result<Value> {
    match request {
        ExecutorRequest::Claim => {
            store.expire_executor_leases()?;
            Ok(serde_json::to_value(store.lease_work()?)?)
        }
        ExecutorRequest::Status { id, lease } => {
            let job = store.leased_job(&id, &lease)?;
            store.authorize_job(&job)?;
            Ok(json!({"status":job.status}))
        }
        ExecutorRequest::Complete {
            id,
            lease,
            result,
            error,
        } => store.complete_work(&id, &lease, result, error),
    }
}

/// Run without opening any server database. Commands inherit this process's unprivileged identity.
///
/// # Errors
/// Returns transport or work-directory errors; execution failures become job results.
#[cfg(unix)]
pub async fn run(control_dir: std::path::PathBuf, work_dir: std::path::PathBuf) -> Result<()> {
    std::fs::create_dir_all(&work_dir)?;
    loop {
        let value = call(&control_dir, ExecutorRequest::Claim).await?;
        let work: Option<Work> = serde_json::from_value(value)?;
        if let Some(work) = work {
            let binding = match &work.manifest.execution {
                crate::capability::Execution::Command { program, .. } => {
                    crate::host_trust::command(program)
                }
                _ => Ok(()),
            };
            let outcome = match binding {
                Err(error) => Err(error),
                Ok(()) => {
                    crate::execution::execute(
                        &work_dir,
                        &work.job,
                        &work.manifest,
                        cancellation(&control_dir, &work),
                    )
                    .await
                }
            };
            let (result, error) = match outcome {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(format!("{error:#}").chars().take(900).collect())),
            };
            call(
                &control_dir,
                ExecutorRequest::Complete {
                    id: work.job.id,
                    lease: work.lease,
                    result,
                    error,
                },
            )
            .await?;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }
}

#[cfg(unix)]
async fn call(dir: &std::path::Path, request: ExecutorRequest) -> Result<Value> {
    crate::local::call(dir, crate::local::Control::Executor { request }).await
}

#[cfg(unix)]
async fn cancellation(dir: &std::path::Path, work: &Work) -> Result<()> {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let state = call(
            dir,
            ExecutorRequest::Status {
                id: work.job.id.clone(),
                lease: work.lease.clone(),
            },
        )
        .await?;
        let status = state["status"]
            .as_str()
            .context("missing executor job status")?;
        if status != "running" {
            return Ok(());
        }
    }
}

// A successful JSON null is distinct from an omitted result on a failed job.
fn present_result<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(decoder).map(Some)
}
