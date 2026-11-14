//! `xlatch-sessions`: the optional session-bridge command adapter (ADR 0008).
//! `list` and `send` read one JSON value on stdin and write one JSON value on
//! stdout, as xlatch command capabilities require. `manifests` is the operator
//! command that writes the capability manifests bound to this binary.
use std::io::Read as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use xlatch_sessions::{Providers, SendInput, manifests, sha256_hex};

/// Largest stdin request: prompt text plus an inline file of up to 4 `MiB`.
const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Send to live hc or herdr agent sessions (optional xlatch adapter)"
)]
struct Options {
    /// Absolute path of the hc (herdr-core) program to consult.
    #[arg(long)]
    hc: Option<PathBuf>,
    /// Absolute path of the legacy herdr program to consult.
    #[arg(long)]
    herdr: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// List live agent sessions (metadata only). Capability entry point.
    List,
    /// Send text and/or one file to a target from `list`. Capability entry point.
    Send,
    /// Write sessions.list.json and sessions.send.json bound to this binary.
    Manifests {
        /// Directory to write the two manifests into.
        #[arg(long)]
        out: PathBuf,
    },
}

fn request() -> Result<String> {
    let mut body = String::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_string(&mut body)
        .context("read request")?;
    ensure!(
        body.len() as u64 <= MAX_INPUT_BYTES,
        "request exceeds {MAX_INPUT_BYTES} bytes"
    );
    Ok(body)
}

fn main() -> Result<()> {
    let options = Options::parse();
    for program in [&options.hc, &options.herdr].into_iter().flatten() {
        ensure!(
            program.is_absolute(),
            "provider programs must be absolute paths"
        );
    }
    ensure!(
        options.hc.is_some() || options.herdr.is_some(),
        "enable at least one provider with --hc or --herdr"
    );
    let providers = Providers {
        hc: options.hc,
        herdr: options.herdr,
    };
    let reply = match options.command {
        Action::List => {
            // Drain the request so the executor's stdin write never sees a closed pipe.
            request()?;
            serde_json::to_value(providers.list())?
        }
        Action::Send => {
            let input: SendInput =
                serde_json::from_str(&request()?).context("invalid send request")?;
            serde_json::to_value(providers.send(&input)?)?
        }
        Action::Manifests { out } => {
            let adapter = std::env::current_exe()?
                .canonicalize()
                .context("locate this adapter")?;
            let sha256 = sha256_hex(&std::fs::read(&adapter)?);
            std::fs::create_dir_all(&out)?;
            let mut written = Vec::new();
            for (id, manifest) in manifests(&adapter, &sha256, &providers)? {
                let path = out.join(format!("{id}.json"));
                std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
                written.push(path);
            }
            serde_json::json!({"adapter": adapter, "sha256": sha256, "written": written})
        }
    };
    println!("{reply}");
    Ok(())
}
