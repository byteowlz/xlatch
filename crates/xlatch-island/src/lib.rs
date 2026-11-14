//! Optional anchr integration: present xlatch job activity on the shared desktop
//! island through anchr's status-only local transport (`docs/transport.md` in anchr).
//!
//! Only capability titles and job states cross this seam: never shared content,
//! targets, results, file names or paths. Every activity is a lease, so a stopped
//! companion or xlatch server makes the island forget it; expiry is not success.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use xlatch_core::capability::Job;
use xlatch_desktop::Snapshot;

/// The anchr `source` claimed by xlatch. A claim, not an identity.
pub const SOURCE: &str = "xlatch";
/// Activity id for in-flight work.
pub const JOBS: &str = "jobs";
/// Activity id for the most recent outcome.
pub const RESULT: &str = "result";

const LABEL_CHARS: usize = 120;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE: usize = 16 * 1024;
/// anchr rejects socket paths longer than `sockaddr_un` allows.
const MAX_SOCKET_PATH: usize = 103;

/// One anchr status-transport message (version 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// Create or refresh a leased activity.
    Publish {
        /// Protocol version.
        version: u32,
        /// Claimed source app.
        source: &'static str,
        /// Activity id within the source.
        id: &'static str,
        /// Short label shown on the island.
        label: String,
        /// Optional secondary line.
        detail: Option<String>,
        /// Lease length.
        ttl_ms: u64,
    },
    /// End an activity explicitly.
    Withdraw {
        /// Protocol version.
        version: u32,
        /// Claimed source app.
        source: &'static str,
        /// Activity id within the source.
        id: &'static str,
    },
}

impl Frame {
    const fn publish(id: &'static str, label: String, detail: String, ttl_ms: u64) -> Self {
        Self::Publish {
            version: 1,
            source: SOURCE,
            id,
            label,
            detail: Some(detail),
            ttl_ms,
        }
    }

    const fn withdraw(id: &'static str) -> Self {
        Self::Withdraw {
            version: 1,
            source: SOURCE,
            id,
        }
    }
}

/// Turns successive job snapshots into island frames.
#[derive(Debug, Default)]
pub struct Tracker {
    /// Jobs seen queued or running, so their outcome can be announced once.
    active: HashSet<String>,
    /// Jobs whose outcome was already announced or predates the companion.
    settled: HashSet<String>,
    /// Whether the `jobs` activity is currently published.
    published: bool,
    /// Whether the first snapshot was seen; earlier outcomes are not announced.
    primed: bool,
    /// Seconds since the epoch when tracking started.
    started: i64,
}

impl Tracker {
    /// A tracker that announces outcomes only of jobs created at or after `started`.
    #[must_use]
    pub fn new(started: i64) -> Self {
        Self {
            started,
            ..Self::default()
        }
    }

    /// Frames for the latest snapshot. `lease_ms` covers a few refresh intervals.
    #[must_use]
    pub fn update(&mut self, snapshot: &Snapshot, lease_ms: u64) -> Vec<Frame> {
        let titles: HashMap<&str, &str> = snapshot
            .capabilities
            .iter()
            .map(|c| (c.manifest.id.as_str(), c.manifest.title.as_str()))
            .collect();
        let title = |job: &Job| {
            clean(
                titles
                    .get(job.capability_id.as_str())
                    .copied()
                    .unwrap_or("xlatch action"),
            )
        };
        let mut frames = Vec::new();

        // Newest outcome of a job that was in flight or started after us.
        let outcome = snapshot
            .jobs
            .iter()
            .filter(|j| is_terminal(&j.status) && !self.settled.contains(&j.id))
            .filter(|j| {
                self.active.contains(&j.id) || (self.primed && j.created_at >= self.started)
            })
            .max_by_key(|j| j.created_at);
        if let Some(job) = outcome {
            let (word, ttl) = match job.status.as_str() {
                "succeeded" => ("done", 6_000),
                "failed" => ("failed", 12_000),
                _ => ("cancelled", 6_000),
            };
            frames.push(Frame::publish(
                RESULT,
                title(job),
                format!("xlatch · {word}"),
                ttl,
            ));
        }
        for job in &snapshot.jobs {
            if is_terminal(&job.status) && (!self.primed || self.active.contains(&job.id)) {
                self.settled.insert(job.id.clone());
            }
        }
        if let Some(job) = outcome {
            self.settled.insert(job.id.clone());
        }

        let mut active: Vec<&Job> = snapshot
            .jobs
            .iter()
            .filter(|j| matches!(j.status.as_str(), "queued" | "running"))
            .collect();
        active.sort_by_key(|j| (j.status == "running", j.created_at));
        if let Some(primary) = active.last() {
            let state = if primary.status == "running" {
                "running"
            } else {
                "queued"
            };
            let detail = match active.len() - 1 {
                0 => format!("xlatch · {state}"),
                more => format!("xlatch · {state} · {more} more"),
            };
            frames.push(Frame::publish(JOBS, title(primary), detail, lease_ms));
            self.published = true;
        } else if self.published {
            frames.push(Frame::withdraw(JOBS));
            self.published = false;
        }

        let present: HashSet<&str> = snapshot.jobs.iter().map(|j| j.id.as_str()).collect();
        self.active = active.iter().map(|j| j.id.clone()).collect();
        self.settled.retain(|id| present.contains(id.as_str()));
        self.primed = true;
        frames
    }

    /// Frames when xlatch cannot be reached: withdraw in-flight work, since its
    /// state is unknown. Outcomes are not guessed.
    pub fn lost(&mut self) -> Vec<Frame> {
        self.active.clear();
        if std::mem::take(&mut self.published) {
            vec![Frame::withdraw(JOBS)]
        } else {
            Vec::new()
        }
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

/// A bounded single-line label: no control characters, never blank.
fn clean(text: &str) -> String {
    let single: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed: String = single.trim().chars().take(LABEL_CHARS).collect();
    if trimmed.is_empty() {
        "xlatch action".into()
    } else {
        trimmed
    }
}

/// anchr's host socket: `explicit`, then `ANCHR_SOCKET`, `$XDG_RUNTIME_DIR`,
/// `$XDG_STATE_HOME`, then `~/.local/state`, each under `anchr/host.sock`.
///
/// # Errors
/// Fails for relative or overlong paths, or when no location can be derived.
pub fn socket_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let env_dir = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let path = explicit
        .or_else(|| env_dir("ANCHR_SOCKET"))
        .or_else(|| env_dir("XDG_RUNTIME_DIR").map(|d| d.join("anchr/host.sock")))
        .or_else(|| env_dir("XDG_STATE_HOME").map(|d| d.join("anchr/host.sock")))
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state/anchr/host.sock")))
        .context("cannot locate the anchr socket")?;
    ensure!(path.is_absolute(), "the anchr socket path must be absolute");
    ensure!(
        path.as_os_str().len() <= MAX_SOCKET_PATH,
        "the anchr socket path exceeds {MAX_SOCKET_PATH} bytes"
    );
    Ok(path)
}

/// Sends one frame and checks anchr's `{"ok":true}` reply.
///
/// # Errors
/// Fails when the host is not running, times out or rejects the frame.
pub fn send(socket: &Path, frame: &Frame) -> Result<()> {
    let mut stream = UnixStream::connect(socket).context("anchr is not running")?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut payload = serde_json::to_vec(frame)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()?;
    let mut reply = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).context("read anchr reply")?;
        if read == 0 {
            break;
        }
        reply.extend_from_slice(chunk.get(..read).context("read anchr reply")?);
        ensure!(reply.len() <= MAX_RESPONSE, "anchr reply too large");
        if reply.contains(&b'\n') {
            break;
        }
    }
    let reply: Value = serde_json::from_slice(&reply).context("invalid anchr reply")?;
    if reply["ok"].as_bool() != Some(true) {
        bail!(
            "anchr rejected the frame: {}",
            reply["error"]["code"].as_str().unwrap_or("unknown")
        );
    }
    Ok(())
}
