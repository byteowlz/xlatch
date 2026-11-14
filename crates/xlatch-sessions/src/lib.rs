//! Optional session bridge (ADR 0008): list live agent sessions held by hc
//! (`herdr-core`) or the legacy herdr server, and send text or one file to a
//! target that is re-validated against the live list on every send.
//!
//! The bridge never creates, interrupts or reads sessions, never forces input
//! into a blocked program and never targets plain shells.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// Largest prompt text accepted, matching the paired client's prompt bound.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Detected "agents" that are shells: never targets. This also hides hc's
/// attach mirrors of legacy herdr panes, which hc reports as `sh`.
const SHELLS: &[&str] = &[
    "", "sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu", "pwsh", "elvish", "xonsh",
    "shell",
];

/// Terminal managers the bridge may consult. Paths are fixed by the operator's
/// capability manifest, never by a request.
#[derive(Debug, Clone, Default)]
pub struct Providers {
    /// Absolute path of the `hc` program.
    pub hc: Option<PathBuf>,
    /// Absolute path of the legacy `herdr` program.
    pub herdr: Option<PathBuf>,
}

/// One live agent session, metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    /// Stable target id: `<provider>:<address>`.
    pub id: String,
    /// `hc` or `herdr`.
    pub provider: String,
    /// Provider-native address (`t7`, `machine/t7`, `w2:p88`).
    pub address: String,
    /// Detected agent, e.g. `claude`, `codex`, `pi`.
    pub agent: String,
    /// Provider status when known (`idle`, `working`, `blocked`, `done`).
    pub status: Option<String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// Human title or location.
    pub title: Option<String>,
}

/// A provider that could not be listed; other providers still are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unavailable {
    /// `hc` or `herdr`.
    pub provider: String,
    /// Bounded reason.
    pub error: String,
}

/// Result of `list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    /// Live agent sessions.
    pub sessions: Vec<Session>,
    /// Providers that failed to answer.
    pub unavailable: Vec<Unavailable>,
}

/// One shared file: an executor-staged artifact (`file_input: "path"`) or a
/// small inline file. Other fields are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileRef {
    /// Private path staged by the executor for this job only.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Original file name, kept so the landed file keeps its extension.
    #[serde(default)]
    pub name: Option<String>,
    /// Inline bytes for small files the client did not upload separately.
    #[serde(default)]
    pub data_base64: Option<String>,
}

impl FileRef {
    /// A staged file at `path`.
    #[must_use]
    pub fn staged(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            ..Self::default()
        }
    }

    /// A local path for this file, writing inline bytes into `scratch` first.
    fn materialize(&self, scratch: &Path) -> Result<PathBuf> {
        if let Some(path) = &self.path {
            return Ok(path.clone());
        }
        let data = self
            .data_base64
            .as_deref()
            .context("file has neither a staged path nor inline data")?;
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
            .context("inline file is not valid base64")?;
        let name = self
            .name
            .as_deref()
            .and_then(|n| Path::new(n).file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.starts_with('.'))
            .unwrap_or("shared-file");
        let path = scratch.join(name);
        std::fs::write(&path, bytes).context("stage inline file")?;
        Ok(path)
    }
}

/// Input of `send`.
#[derive(Debug, Clone, Deserialize)]
pub struct SendInput {
    /// Target id from `list`: `<provider>:<address>`.
    pub target: String,
    /// Prompt text; may be empty only when a file is attached.
    #[serde(default)]
    pub text: String,
    /// Optional single file, delivered beside the session (hc only).
    #[serde(default)]
    pub file: Option<FileRef>,
}

/// Receipt of a send. The text itself stays in the xlatch job input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Receipt {
    /// `hc` or `herdr`.
    pub provider: String,
    /// Target id that received the prompt.
    pub target: String,
    /// Agent detected at send time.
    pub agent: String,
    /// SHA-256 of the exact text submitted (including any attachment note).
    pub text_sha256: String,
    /// Where an attached file landed, when there was one.
    pub file_path: Option<String>,
}

/// Shipped capability templates (`examples/capabilities`), rendered by
/// [`manifests`]; never registered by default.
pub const TEMPLATES: [(&str, &str); 2] = [
    (
        "list",
        include_str!("../../../examples/capabilities/sessions-list.json"),
    ),
    (
        "send",
        include_str!("../../../examples/capabilities/sessions-send.json"),
    ),
];

/// The two capability manifests bound to this adapter at `adapter` (its
/// SHA-256 `adapter_sha256`) and to the enabled provider programs.
///
/// # Errors
/// Fails for relative paths, no enabled provider or a malformed template.
pub fn manifests(
    adapter: &Path,
    adapter_sha256: &str,
    providers: &Providers,
) -> Result<Vec<(String, Value)>> {
    ensure!(adapter.is_absolute(), "adapter path must be absolute");
    let mut fixed = Vec::new();
    for (flag, program) in [("--hc", &providers.hc), ("--herdr", &providers.herdr)] {
        if let Some(program) = program {
            ensure!(program.is_absolute(), "{flag} must be an absolute path");
            fixed.push(flag.to_owned());
            fixed.push(
                program
                    .to_str()
                    .context("provider path is not UTF-8")?
                    .to_owned(),
            );
        }
    }
    ensure!(!fixed.is_empty(), "enable at least one provider");
    TEMPLATES
        .iter()
        .map(|(verb, template)| {
            let mut manifest: Value = serde_json::from_str(template)?;
            let mut args = fixed.clone();
            args.push((*verb).to_owned());
            manifest["execution"] = serde_json::json!({
                "kind": "command",
                "program": adapter.to_str().context("adapter path is not UTF-8")?,
                "args": args,
                "sha256": adapter_sha256,
            });
            let id = manifest["id"].as_str().context("template id")?.to_owned();
            Ok((id, manifest))
        })
        .collect()
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

impl Providers {
    /// All live agent sessions from every configured provider.
    #[must_use]
    pub fn list(&self) -> Listing {
        let mut listing = Listing {
            sessions: Vec::new(),
            unavailable: Vec::new(),
        };
        for (provider, result) in [
            ("hc", self.hc.as_deref().map(hc_sessions)),
            ("herdr", self.herdr.as_deref().map(herdr_sessions)),
        ] {
            match result {
                Some(Ok(mut sessions)) => listing.sessions.append(&mut sessions),
                Some(Err(error)) => listing.unavailable.push(Unavailable {
                    provider: provider.into(),
                    error: bounded(&format!("{error:#}")),
                }),
                None => {}
            }
        }
        listing
    }

    /// Sends `input` to its target after re-validating it against the live list.
    ///
    /// # Errors
    /// Rejects unknown, stale, shell or blocked targets, oversized or empty
    /// prompts, files for providers that cannot place them, and provider failures.
    pub fn send(&self, input: &SendInput) -> Result<Receipt> {
        ensure!(
            input.text.len() <= MAX_TEXT_BYTES,
            "text exceeds {MAX_TEXT_BYTES} bytes"
        );
        ensure!(
            !input.text.trim().is_empty() || input.file.is_some(),
            "nothing to send: text is empty and no file is attached"
        );
        let (provider, address) = input
            .target
            .split_once(':')
            .context("target must be <provider>:<address> from sessions.list")?;
        let (program, sessions) = match provider {
            "hc" => {
                let program = self.hc.as_deref().context("hc is not enabled")?;
                (program, hc_sessions(program)?)
            }
            "herdr" => {
                let program = self.herdr.as_deref().context("herdr is not enabled")?;
                (program, herdr_sessions(program)?)
            }
            other => bail!("unknown provider {other:?}"),
        };
        let session = sessions
            .into_iter()
            .find(|s| s.address == address)
            .with_context(|| format!("{} is not a live agent session", input.target))?;
        ensure!(
            session.status.as_deref() != Some("blocked"),
            "{} is waiting on a dialog; nothing was sent",
            input.target
        );
        let (text, file_path) = match (provider, &input.file) {
            ("hc", Some(file)) => {
                // Private scratch for inline bytes; removed when this send returns.
                let scratch = tempfile::tempdir().context("create scratch directory")?;
                let local = file.materialize(scratch.path())?;
                let landed = run(
                    program,
                    &[
                        "put",
                        address,
                        local.to_str().context("file path is not UTF-8")?,
                        "--json",
                    ],
                )?;
                let path = landed["path"]
                    .as_str()
                    .context("hc put did not report a path")?
                    .to_owned();
                let note = format!("Attached file: {path}");
                let text = if input.text.trim().is_empty() {
                    note
                } else {
                    format!("{}\n\n{note}", input.text)
                };
                (text, Some(path))
            }
            (_, Some(_)) => bail!("the legacy herdr provider cannot deliver files; use hc"),
            (_, None) => (input.text.clone(), None),
        };
        if provider == "hc" {
            run(program, &["send", address, &text, "--json"])?;
        } else {
            run(program, &["agent", "prompt", address, &text])?;
        }
        Ok(Receipt {
            provider: provider.into(),
            target: input.target.clone(),
            agent: session.agent,
            text_sha256: hex(&Sha256::digest(text.as_bytes())),
            file_path,
        })
    }
}

fn is_shell(agent: &str) -> bool {
    SHELLS.contains(&agent)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn hc_sessions(program: &Path) -> Result<Vec<Session>> {
    let listing = run(program, &["ls", "--json"])?;
    let terminals = listing["terminals"]
        .as_array()
        .context("hc ls returned no terminal list")?;
    Ok(terminals
        .iter()
        .filter(|t| t["state"].as_str() == Some("live"))
        .filter_map(|t| {
            let address = text(t, "address")?;
            let agent = text(t, "agent").unwrap_or_default();
            (!is_shell(&agent)).then(|| Session {
                id: format!("hc:{address}"),
                provider: "hc".into(),
                address,
                agent,
                status: text(t, "status"),
                cwd: text(t, "cwd"),
                title: text(t, "name").or_else(|| text(t, "where")),
            })
        })
        .collect())
}

fn herdr_sessions(program: &Path) -> Result<Vec<Session>> {
    let listing = run(program, &["agent", "list"])?;
    let agents = listing["result"]["agents"]
        .as_array()
        .context("herdr agent list returned no agents")?;
    Ok(agents
        .iter()
        .filter_map(|a| {
            let address = text(a, "pane_id")?;
            let agent = text(a, "agent").unwrap_or_default();
            (!is_shell(&agent)).then(|| Session {
                id: format!("herdr:{address}"),
                provider: "herdr".into(),
                address,
                agent,
                status: text(a, "agent_status"),
                cwd: text(a, "cwd"),
                title: text(a, "terminal_title_stripped"),
            })
        })
        .collect())
}

/// Runs a provider with the user's `HOME` (command execution clears the
/// environment) and parses its single JSON reply.
fn run(program: &Path, args: &[&str]) -> Result<Value> {
    let home = dirs::home_dir().context("cannot determine the user's home directory")?;
    let output = Command::new(program)
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("start {}", program.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reply: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    if !output.status.success() {
        let reason = reply
            .as_ref()
            .and_then(|r| r["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    stdout.trim(),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        bail!(
            "{} {} failed ({}): {}",
            program.display(),
            args.first().copied().unwrap_or_default(),
            output.status,
            bounded(&reason)
        );
    }
    // A successful exit is the delivery signal; replies that matter (lists,
    // landed paths) are checked by their callers.
    let reply = reply.unwrap_or(Value::Null);
    if let Some(message) = reply["error"]["message"].as_str() {
        bail!("{}", bounded(message));
    }
    Ok(reply)
}

fn bounded(text: &str) -> String {
    text.trim().chars().take(1000).collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
