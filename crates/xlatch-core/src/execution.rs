//! Execution without access to the approval database or service secrets.
use crate::capability::{Execution, Job, MAX_BYTES, Manifest};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Execute a job already authorized by the broker under the executor's OS identity.
///
/// # Errors
/// Rejects changed executable content, output/schema violations, timeout and cancellation.
pub async fn execute(
    dir: &Path,
    job: &Job,
    manifest: &Manifest,
    cancellation: impl std::future::Future<Output = Result<()>>,
) -> Result<Value> {
    manifest.validate_host_binding()?;
    let result = match &manifest.execution {
        Execution::Compose { .. } => anyhow::bail!("composition must be scheduled by the broker"),
        Execution::Echo => job.input.clone(),
        Execution::SaveFile { directory } => crate::save_file::save(directory, &job.input)?,

        Execution::Command { .. } => run_command(dir, job, manifest, cancellation).await?,
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
    cancellation: impl std::future::Future<Output = Result<()>>,
) -> Result<Value> {
    let Execution::Command { program, args, .. } = &manifest.execution else {
        anyhow::bail!("expected a command execution binding");
    };
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
        result=cancellation=>{result?;anyhow::bail!("job cancelled")}
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
