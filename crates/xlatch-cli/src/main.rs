//! Local operator CLI for `CrossLatch`.

mod network;
mod pairing;
mod server;
mod service_install;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;
use xlatch_core::{
    capability::{Manifest, Request},
    local::{self, Control},
    paths::default_data_dir,
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Register, approve, pair and invoke CrossLatch capabilities"
)]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run or manage the background user service.
    Service {
        #[command(subcommand)]
        command: service_install::Command,
    },
    /// Submit a manifest for approval.
    Register { manifest: PathBuf },
    /// Approve exactly the revision printed by register/list.
    Approve {
        id: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        allow_host_execution: bool,
    },
    /// List capabilities, including pending registrations.
    List,
    /// Issue a five-minute QR code granting these active capabilities.
    #[command(
        after_help = "Run `xlatch list` to find approved capability IDs.\nExamples:\n  xlatch pair --capability echo --qr pair.svg\n  xlatch pair --capabilities echo,audio-wav --qr pair.svg\nOnly the named capabilities are granted. Use the same --data-dir as the daemon."
    )]
    Pair {
        /// Approved capability ID; repeat this option or separate IDs with commas.
        #[arg(
            long = "capability",
            visible_alias = "capabilities",
            value_name = "ID",
            value_delimiter = ','
        )]
        capabilities: Vec<String>,
        #[arg(long)]
        qr: Option<PathBuf>,
    },
    /// Grant an approved action revision to a paired device without pairing again.
    Grant {
        device: String,
        id: String,
        #[arg(long)]
        revision: String,
    },
    /// List paired devices.
    Devices,
    /// Revoke device access and cancel its jobs.
    Revoke { id: String },
    /// Enqueue a JSON input. --wait waits on the same durable job.
    Invoke {
        id: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    /// List recent jobs without large input/result bodies.
    Jobs,
    /// Retrieve one job including its result.
    Job { id: String },
    /// Cancel outstanding work.
    Cancel { id: String },
    /// Fetch job changes after an event cursor.
    Events {
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
    /// Generate shell completions.
    Completions { shell: clap_complete::Shell },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let data_dir = cli.data_dir.map_or_else(default_data_dir, Ok)?;
    let mut wait_for = None;
    let control = match cli.command {
        Command::Service { command } => {
            return service_install::dispatch(data_dir, command).await;
        }
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "xlatch", &mut std::io::stdout());
            return Ok(());
        }
        Command::Register { manifest } => Control::Register {
            manifest: serde_json::from_slice::<Manifest>(&std::fs::read(manifest)?)?,
        },
        Command::Approve {
            id,
            revision,
            allow_host_execution,
        } => Control::Approve {
            id,
            revision,
            allow_host_execution,
        },
        Command::Pair {
            capabilities,
            qr: output,
        } => {
            return pairing::run(&data_dir, capabilities, output, cli.json).await;
        }
        Command::Grant {
            device,
            id,
            revision,
        } => Control::Grant {
            device,
            id,
            revision,
        },
        Command::Devices => Control::Devices,
        Command::Revoke { id } => Control::Revoke { id },
        Command::List => Control::Rpc {
            request: Request::Discover,
        },
        Command::Invoke {
            id,
            revision,
            input,
            key,
            wait,
            timeout,
        } => {
            if wait {
                wait_for = Some(timeout);
            }
            Control::Rpc {
                request: Request::Invoke {
                    capability_id: id,
                    revision,
                    input: serde_json::from_slice(&std::fs::read(input)?)?,
                    idempotency_key: key.unwrap_or_else(|| {
                        format!(
                            "cli-{}-{}",
                            std::process::id(),
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_nanos())
                        )
                    }),
                },
            }
        }
        Command::Jobs => Control::Rpc {
            request: Request::Jobs,
        },
        Command::Job { id } => Control::Rpc {
            request: Request::Job { id },
        },
        Command::Cancel { id } => Control::Rpc {
            request: Request::Cancel { id },
        },
        Command::Events { after } => Control::Rpc {
            request: Request::Events { after },
        },
    };
    let value = local::call(&data_dir, control).await?;
    finish_output(&data_dir, value, None, wait_for).await
}

async fn finish_output(
    dir: &std::path::Path,
    mut value: Value,
    qr: Option<PathBuf>,
    wait_for: Option<u64>,
) -> Result<()> {
    if let Some(path) = qr {
        let encoded = serde_json::to_string(&value)?;
        let code = qrcode::QrCode::new(encoded.as_bytes())?;
        std::fs::write(
            path,
            code.render::<qrcode::render::svg::Color<'_>>()
                .min_dimensions(512, 512)
                .build(),
        )?;
    }
    if let Some(seconds) = wait_for {
        let id = value["id"].as_str().context("missing job id")?.to_string();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
        while matches!(value["status"].as_str(), Some("queued" | "running"))
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            value = local::call(
                dir,
                Control::Rpc {
                    request: Request::Job { id: id.clone() },
                },
            )
            .await?;
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    if value.get("status").and_then(Value::as_str) == Some("failed") {
        anyhow::bail!("job failed");
    }
    Ok(())
}
