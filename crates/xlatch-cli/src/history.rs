//! Local operator controls for optional routing history.
use anyhow::Result;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use xlatch_core::{history::HistoryPolicy, store::Store};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show capture policy. History is disabled by default.
    Status,
    /// Load an explicit JSON or TOML policy. Content mode can retain sensitive shared text.
    Configure { file: PathBuf },
    /// Export dataset as JSONL. Excludes operational inputs unless capture was enabled.
    Export {
        #[arg(long)]
        owner: Option<String>,
    },
    /// Delete dataset records, leaving operational jobs and retry identities intact.
    Purge {
        #[arg(long)]
        owner: Option<String>,
    },
}
pub fn dispatch(dir: &Path, command: Command) -> Result<()> {
    let store = Store::open(dir)?;
    match command {
        Command::Status => println!(
            "{}",
            serde_json::to_string_pretty(&store.history_policy()?)?
        ),
        Command::Configure { file } => {
            let raw = std::fs::read_to_string(&file)?;
            let policy: HistoryPolicy = if file.extension().is_some_and(|ext| ext == "toml") {
                toml::from_str(&raw)?
            } else {
                serde_json::from_str(&raw)?
            };
            store.set_history_policy(&policy)?;
            println!("{}", serde_json::to_string_pretty(&policy)?);
        }
        Command::Export { owner } => {
            for record in store.history(owner.as_deref())? {
                println!("{}", serde_json::to_string(&record)?);
            }
        }
        Command::Purge { owner } => println!(
            "{}",
            serde_json::json!({"deleted":store.purge_history(owner.as_deref())?})
        ),
    }
    Ok(())
}
