//! Local operator CLI for `CrossLatch`.

mod compose;
mod destination;
mod history;
mod network;
mod notifications;
mod pairing;
mod protected;
mod protected_install;
mod server;
mod service_install;
mod tls;

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
    /// Directory of a protected service's local control socket.
    #[arg(long, global = true)]
    control_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Combine existing actions into a new pending share target.
    Compose(compose::Options),
    /// Configure and export optional server-side routing history (local operator only).
    History {
        #[command(subcommand)]
        command: history::Command,
    },
    /// Show a QR for explicitly trusting a rotated server certificate on paired phones.
    Identity,
    /// Run the unprivileged executor for a protected service.
    Executor {
        /// Working directory for approved commands (never the service data directory).
        #[arg(long)]
        work_dir: PathBuf,
    },
    /// Issue a one-time code for enabling phone-approved enrollment in the app.
    EnrollmentBootstrap {
        /// Existing paired device that will become the first approver.
        device: String,
    },
    /// Run or manage the background user service.
    Service {
        /// Manage the system service under its dedicated account.
        #[arg(long, global = true)]
        protected: bool,
        #[command(subcommand)]
        command: service_install::Command,
    },
    /// Configure server-side save destinations.
    Destination {
        #[command(subcommand)]
        command: destination::Command,
    },
    /// Submit a manifest for approval.
    Register {
        manifest: PathBuf,
        /// Embed a PNG or self-contained SVG as a portable action icon.
        #[arg(long)]
        icon: Option<PathBuf>,
    },
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
    let control_dir = cli.control_dir.unwrap_or_else(|| data_dir.clone());
    let mut wait_for = None;
    let control = match cli.command {
        Command::Compose(options) => return compose::dispatch(&control_dir, options).await,
        Command::History { command } => return history::dispatch(&data_dir, command),
        Command::Identity => return pairing::identity(&control_dir, cli.json).await,
        Command::Executor { work_dir } => {
            return protected::run_executor(control_dir, work_dir).await;
        }
        Command::EnrollmentBootstrap { device } => {
            return pairing::enrollment_bootstrap(&control_dir, device, cli.json).await;
        }
        Command::Service { command, protected } => {
            return service_install::dispatch(data_dir, command, protected).await;
        }
        Command::Destination { command } => {
            return destination::dispatch(&control_dir, command).await;
        }
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "xlatch", &mut std::io::stdout());
            return Ok(());
        }
        Command::Register { manifest, icon } => {
            let mut manifest: Manifest = serde_json::from_slice(&std::fs::read(manifest)?)?;
            if let Some(path) = icon {
                manifest.icon = Some(xlatch_core::icon::Icon::from_file(&path)?);
            }
            Control::Register { manifest }
        }
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
            return pairing::run(&control_dir, capabilities, output, cli.json).await;
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
                    idempotency_key: key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
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
    let value = local::call(&control_dir, control).await?;
    finish_output(&control_dir, value, None, wait_for).await
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
