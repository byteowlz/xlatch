//! Minimal desktop paired-client CLI. No daemon or graphical runtime required.
use anyhow::{Context as _, Result, ensure};
use clap::{CommandFactory as _, Parser, Subcommand};
use serde_json::{Value, json};
use std::{path::PathBuf, process::ExitCode};
use xlatch_client::{Client, initialize, read_bounded, text_input};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Pair with xlatch and send content using only this device's grants"
)]
struct Options {
    /// Client config; each config has its own adjacent private device identity.
    #[arg(long, global = true, env = "XLATCH_CLIENT_CONFIG")]
    config: Option<PathBuf>,
    /// One JSON envelope on stdout, including failures.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Enroll once using an out-of-band pairing JSON file, or - for stdin.
    Pair {
        ticket: PathBuf,
        #[arg(long, default_value = "Desktop")]
        name: String,
    },
    /// Inspect enrollment state, including pending approval and comparison payload.
    Status,
    /// Discover only active actions granted to this paired device.
    List,
    /// Send text/stdin/file/JSON to an exact granted revision.
    Send(Send),
    /// List this device's recent jobs.
    Jobs,
    /// Retrieve a job and its result.
    Job { id: String },
    /// Cancel an owned unfinished job.
    Cancel { id: String },
    /// Remove local credentials, not the remote device or its jobs.
    Forget {
        #[arg(long, required = true)]
        yes: bool,
    },
    /// Print the JSON Schema for non-secret config.toml.
    ConfigSchema,
    /// Print the versioned JSON output envelope schema; no connection required.
    OutputSchema,
    /// Generate shell completions.
    Completions { shell: clap_complete::Shell },
}

#[derive(Debug, clap::Args)]
#[command(group(clap::ArgGroup::new("content").required(true).args(["text", "stdin", "file", "input"])))]
struct Send {
    id: String,
    /// Exact digest returned by list; never silently selects a newer revision.
    #[arg(long)]
    revision: String,
    /// Stable invocation identity. Reuse only for the same target and content.
    #[arg(long)]
    key: String,
    #[arg(long)]
    text: Option<String>,
    /// Read UTF-8 text from stdin.
    #[arg(long)]
    stdin: bool,
    /// Regular exported file; large inputs use resumable chunked uploads.
    #[arg(long, requires = "mime_type")]
    file: Option<PathBuf>,
    /// Instruction accompanying the file; the target must accept file plus text.
    #[arg(long, requires = "file", conflicts_with = "prompt_file")]
    prompt: Option<String>,
    /// Read the instruction from a UTF-8 file, or - for stdin (at most 64 `KiB`).
    #[arg(long, requires = "file")]
    prompt_file: Option<PathBuf>,
    /// Raw JSON input file, or - for stdin.
    #[arg(long)]
    input: Option<PathBuf>,
    /// MIME type for text or files. Files require this option.
    #[arg(long)]
    mime_type: Option<String>,
    #[arg(long)]
    wait: bool,
    /// Overall wait limit; a timeout does not cancel the remote job.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
}

#[tokio::main]
async fn main() -> ExitCode {
    let machine = std::env::args_os().any(|argument| argument == "--json");
    let options = match Options::try_parse() {
        Ok(options) => options,
        Err(error) => {
            if machine && error.use_stderr() {
                println!(
                    "{}",
                    json!({"schema_version":1,"ok":false,"error":{"code":"invalid_arguments","message":error.to_string()}})
                );
            } else {
                let _ = error.print();
            }
            return if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    match dispatch(options).await {
        Ok(Some(value)) => {
            if machine {
                println!("{}", json!({"schema_version":1,"ok":true,"value":value}));
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                );
            }
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            if machine {
                println!(
                    "{}",
                    json!({"schema_version":1,"ok":false,"error":{"code":"client_error","message":format!("{error:#}")}})
                );
            } else {
                eprintln!("xlatch-client: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn input(path: &std::path::Path) -> Result<Value> {
    let bytes = if path == std::path::Path::new("-") {
        read_bounded(std::io::stdin().lock(), 8 * 1024 * 1024)?
    } else {
        read_bounded(std::fs::File::open(path)?, 8 * 1024 * 1024)?
    };
    Ok(serde_json::from_slice(&bytes)?)
}

async fn dispatch(options: Options) -> Result<Option<Value>> {
    if let Command::Completions { shell } = options.command {
        clap_complete::generate(
            shell,
            &mut Options::command(),
            "xlatch-client",
            &mut std::io::stdout(),
        );
        return Ok(None);
    }
    if matches!(options.command, Command::OutputSchema) {
        return Ok(Some(xlatch_client::output_schema()));
    }
    if matches!(options.command, Command::ConfigSchema) {
        return Ok(Some(serde_json::to_value(schemars::schema_for!(
            xlatch_client::Settings
        ))?));
    }
    let path = options
        .config
        .map_or_else(xlatch_client::default_config, Ok)?;
    initialize(&path)?;
    if let Command::Pair { ticket, name } = &options.command {
        return Ok(Some(Client::pair(&path, &input(ticket)?, name).await?));
    }
    if let Command::Forget { .. } = options.command {
        xlatch_client::forget(&path)?;
        return Ok(Some(json!({"forgotten":true,"remote_revoked":false})));
    }
    let client = Client::load(&path)?;
    let request = match options.command {
        Command::Status => json!({"op":"enrollment","request":{"action":"status"}}),
        Command::List => json!({"op":"discover"}),
        Command::Jobs => json!({"op":"jobs"}),
        Command::Job { id } => json!({"op":"job","id":id}),
        Command::Cancel { id } => json!({"op":"cancel","id":id}),
        Command::Send(send) => return Ok(Some(send_content(&client, send).await?)),
        _ => anyhow::bail!("unsupported command"),
    };
    Ok(Some(client.call(&request).await?))
}

async fn validate_target(client: &Client, send: &Send, mime: &str) -> Result<Value> {
    let large = send
        .file
        .as_ref()
        .map(std::fs::metadata)
        .transpose()?
        .is_some_and(|metadata| metadata.len() > 4 * 1024 * 1024);
    client
        .target(
            &send.id,
            &send.revision,
            send.input.is_none().then_some(mime),
            large,
        )
        .await
}

fn prompt(send: &Send) -> Result<Option<String>> {
    let value = if let Some(path) = &send.prompt_file {
        let bytes = if path == std::path::Path::new("-") {
            read_bounded(std::io::stdin().lock(), 64 * 1024)?
        } else {
            read_bounded(std::fs::File::open(path)?, 64 * 1024)?
        };
        Some(String::from_utf8(bytes)?)
    } else {
        send.prompt.clone()
    };
    ensure!(
        value
            .as_deref()
            .is_none_or(|prompt| !prompt.trim().is_empty() && prompt.len() <= 64 * 1024),
        "prompt must contain 1–65536 bytes"
    );
    Ok(value)
}

async fn send_content(client: &Client, send: Send) -> Result<Value> {
    ensure!(
        !send.key.trim().is_empty(),
        "invocation key must not be empty"
    );
    ensure!(
        send.revision.len() == 64 && send.revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "revision must be the exact SHA-256 from list"
    );
    let prompt = prompt(&send)?;
    let mime = send.mime_type.as_deref().unwrap_or("text/plain");
    let target = validate_target(client, &send, mime).await?;
    let file = send.file.as_deref();
    let mut prepared = file
        .filter(|_| prompt.is_some())
        .map(|file| client.prepare_file(file, mime, &send.key))
        .transpose()?;
    let mut payload = if let Some(file) = &prepared {
        file.input().clone()
    } else if let Some(file) = file {
        client.file_input(file, mime, &send.key).await?
    } else if let Some(path) = &send.input {
        input(path)?
    } else {
        let text = if send.stdin {
            String::from_utf8(read_bounded(std::io::stdin().lock(), 4 * 1024 * 1024)?)?
        } else {
            send.text.context("missing content")?
        };
        text_input(&text, mime)
    };
    if let Some(prompt) = &prompt {
        payload = xlatch_client::file_with_prompt(&target, payload, prompt)?;
    }
    if let Some(prepared) = &mut prepared {
        prepared.upload(client).await?;
    }
    let request = json!({"op":"invoke","capability_id":send.id,"revision":send.revision,"input":payload,"idempotency_key":send.key});
    let mut job = client.call(&request).await?;
    if send.wait {
        let id = job["id"]
            .as_str()
            .context("server omitted job id")?
            .to_owned();
        let wait = async {
            while matches!(job["status"].as_str(), Some("queued" | "running")) {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                job = client.call(&json!({"op":"job","id":id})).await?;
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::time::timeout(std::time::Duration::from_secs(send.timeout), wait).await
            .with_context(|| format!("wait timed out; job {id} was not cancelled; retrieve it or retry with the same --key"))??;
    }
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_arguments_keep_file_only_compatibility() -> Result<()> {
        let common = [
            "xlatch-client",
            "send",
            "fixture",
            "--revision",
            "digest",
            "--key",
            "intent",
        ];
        for arguments in [
            vec!["--file", "bundle.ctx", "--mime-type", "application/zip"],
            vec![
                "--file",
                "bundle.ctx",
                "--mime-type",
                "application/zip",
                "--prompt",
                "Review",
            ],
            vec![
                "--file",
                "bundle.ctx",
                "--mime-type",
                "application/zip",
                "--prompt-file",
                "-",
            ],
        ] {
            Options::try_parse_from(common.into_iter().chain(arguments))?;
        }
        for arguments in [
            vec!["--text", "text", "--prompt", "Review"],
            vec!["--file", "bundle.ctx", "--prompt", "Review"],
            vec![
                "--file",
                "bundle.ctx",
                "--mime-type",
                "application/zip",
                "--prompt",
                "Review",
                "--prompt-file",
                "-",
            ],
        ] {
            ensure!(Options::try_parse_from(common.into_iter().chain(arguments)).is_err());
        }
        Ok(())
    }
}
