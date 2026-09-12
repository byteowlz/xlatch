//! Bounded durable job execution. Interrupted work fails rather than replaying side effects.

use crate::{
    capability::{Execution, Job, MAX_BYTES, Manifest, digest},
    store::Store,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

impl Store {
    /// Mark previously running jobs failed after obtaining the daemon's exclusive lock.
    ///
    /// # Errors
    /// Returns `SQLite` errors.
    pub fn recover(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'failed' FROM jobs WHERE status='running'",[])?;
        tx.execute("UPDATE jobs SET status='failed',error='Daemon restarted during execution; review side effects before retrying' WHERE status='running'",[])?;
        tx.commit()?;
        Ok(())
    }

    fn claim(&mut self) -> Result<Option<(Job, Manifest)>> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let selected:Option<(String,String)>=tx.query_row("SELECT id,manifest FROM jobs WHERE status='queued' ORDER BY created_at,rowid LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((id, body)) = selected else {
            return Ok(None);
        };
        tx.execute("UPDATE jobs SET status='running' WHERE id=?1", [&id])?;
        tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'running' FROM jobs WHERE id=?1",[&id])?;
        tx.commit()?;
        Ok(Some((
            self.job("local", &id)?,
            serde_json::from_str(&body)?,
        )))
    }

    fn finish(&mut self, job: &Job, outcome: Result<Value>) -> Result<()> {
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
        if let Some((job, manifest)) = store.claim()? {
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
    let result = match &manifest.execution {
        Execution::Echo => job.input.clone(),
        Execution::SaveFile { directory } => crate::save_file::save(directory, &job.input)?,

        Execution::Command {
            program,
            args,
            sha256,
        } => {
            ensure!(
                digest(&std::fs::read(program)?) == *sha256,
                "executable changed since approval"
            );
            run_command(dir, job, manifest, program, args).await?
        }
    };
    ensure!(
        serde_json::to_vec(&result)?.len() <= 6 * 1024 * 1024,
        "result exceeds 6 MiB"
    );
    ensure!(
        jsonschema::validator_for(&manifest.output_schema)?.is_valid(&result),
        "output does not match capability schema"
    );
    Ok(result)
}

async fn bounded_read(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(bytes.len() <= MAX_BYTES, "program output exceeds 8 MiB");
    Ok(bytes)
}

async fn run_command(
    dir: &Path,
    job: &Job,
    manifest: &Manifest,
    program: &str,
    args: &[String],
) -> Result<Value> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("start approved program")?;
    #[cfg(unix)]
    let _group = ProcessGroup(child.id().context("missing child process id")? as i32);
    let mut stdin = child.stdin.take().context("missing stdin")?;
    let stdout = child.stdout.take().context("missing stdout")?;
    let stderr = child.stderr.take().context("missing stderr")?;
    let input = serde_json::to_vec(&job.input)?;
    let work = async {
        let write = async {
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, anyhow::Error>(())
        };
        let ((), out, err, status) =
            tokio::try_join!(write, bounded_read(stdout), bounded_read(stderr), async {
                Ok::<_, anyhow::Error>(child.wait().await?)
            })?;
        ensure!(
            status.success(),
            "program failed ({status}): {}",
            String::from_utf8_lossy(&err)
                .chars()
                .take(1000)
                .collect::<String>()
        );
        serde_json::from_slice(&out).context("program must return one JSON value on stdout")
    };
    tokio::select! {
        result=tokio::time::timeout(Duration::from_secs(manifest.timeout_seconds),work)=>result.context("job timed out")?,
        result=wait_for_cancellation(dir,&job.id)=>{result?;anyhow::bail!("job cancelled")}
    }
}

async fn wait_for_cancellation(dir: &Path, id: &str) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if Store::open(dir)?.job("local", id)?.status == "cancelled" {
            return Ok(());
        }
    }
}

#[cfg(unix)]
struct ProcessGroup(i32);

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Descendants must not outlive cancellation or an output-limit failure.
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(self.0),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}
