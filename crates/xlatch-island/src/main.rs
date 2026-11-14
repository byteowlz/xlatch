//! `xlatch-island`: optional companion that shows xlatch job activity on the
//! anchr desktop island. Runs as the user, in the foreground; stop it with Ctrl+C.
//! Leases make the island forget stopped work on their own.
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use xlatch_desktop::Connection;
use xlatch_island::{Tracker, send, socket_path};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Show xlatch job activity on the anchr desktop island (optional)"
)]
struct Options {
    /// Directory of the local xlatch control socket (default: xlatch's data directory).
    #[arg(long, conflicts_with = "client_config")]
    control_dir: Option<PathBuf>,
    /// Use a paired HTTPS identity; shows only this device's own jobs.
    #[arg(long)]
    client_config: Option<PathBuf>,
    /// anchr host socket (default: `ANCHR_SOCKET`, then anchr's XDG location).
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Seconds between refreshes (default 3 locally, 10 when paired).
    #[arg(long)]
    interval: Option<u64>,
    /// Print frames as JSON lines instead of sending them.
    #[arg(long)]
    dry_run: bool,
    #[command(subcommand)]
    command: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Keep the island current until stopped.
    Run,
    /// Refresh once: print the frames and send them unless --dry-run.
    Once,
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

fn main() -> Result<()> {
    let options = Options::parse();
    let connection = match (&options.client_config, &options.control_dir) {
        (Some(config), _) => Connection::Paired(config.clone()),
        (None, Some(dir)) => Connection::Local(dir.clone()),
        (None, None) => Connection::Local(xlatch_core::paths::default_data_dir()?),
    };
    let interval = options
        .interval
        .unwrap_or_else(|| if connection.is_paired() { 10 } else { 3 });
    ensure!(
        (1..=300).contains(&interval),
        "--interval must be 1–300 seconds"
    );
    // Long enough to survive one slow refresh, short enough to fade a stopped companion.
    let lease_ms = interval * 3_000 + 2_000;
    let socket = socket_path(options.socket.clone())?;
    let mut tracker = Tracker::new(now()?);
    let mut log = Log::default();
    loop {
        let frames = match connection.snapshot() {
            Ok(snapshot) => {
                log.clear("xlatch");
                tracker.update(&snapshot, lease_ms)
            }
            Err(error) => {
                log.report("xlatch", &format!("{error:#}"));
                tracker.lost()
            }
        };
        let once = matches!(options.command, Action::Once);
        for frame in &frames {
            if options.dry_run || once {
                println!("{}", serde_json::to_string(frame).context("encode frame")?);
            }
            if options.dry_run {
                continue;
            }
            // Best effort: anchr is optional and may start later.
            match send(&socket, frame) {
                Ok(()) => log.clear("anchr"),
                Err(error) => log.report("anchr", &format!("{error:#}")),
            }
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

/// Reports each distinct problem once, and its recovery, instead of every refresh.
#[derive(Default)]
struct Log(std::collections::HashMap<&'static str, String>);

impl Log {
    fn report(&mut self, what: &'static str, message: &str) {
        if self.0.get(what).is_none_or(|last| last != message) {
            eprintln!("xlatch-island: {what}: {message}");
            self.0.insert(what, message.to_owned());
        }
    }

    fn clear(&mut self, what: &'static str) {
        if self.0.remove(what).is_some() {
            eprintln!("xlatch-island: {what}: reachable again");
        }
    }
}
