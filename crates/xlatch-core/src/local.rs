//! Bounded JSON-lines control transport in a private Unix socket directory.

use crate::{
    capability::{MAX_BYTES, Manifest, Request},
    service,
    store::Store,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Local operator requests, protected by the private directory and Unix peer identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    /// Propose a capability; this does not activate it.
    Register {
        /// Manifest to validate.
        manifest: Manifest,
    },
    /// Explicitly approve a reviewed revision.
    Approve {
        /// Capability identifier.
        id: String,
        /// Reviewed digest.
        revision: String,
        /// Acknowledge trusted host execution.
        allow_host_execution: bool,
    },
    /// Issue a short-lived QR bootstrap.
    Pair {
        /// Exact capability ids to grant.
        capabilities: Vec<String>,
    },
    /// List enrolled devices.
    Devices,
    /// Revoke a device and cancel outstanding work.
    Revoke {
        /// Device identifier.
        id: String,
    },
    /// Execute a core request as the local operator.
    Rpc {
        /// Transport-neutral request.
        request: Request,
    },
}

/// Serve local control requests. Local agents run as the trusted OS user in v0.
///
/// # Errors
/// Returns socket setup or accept errors.
#[cfg(unix)]
pub async fn serve(dir: std::path::PathBuf, urls: Vec<String>, pin: String) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let socket = dir.join("control.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = tokio::net::UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        let permit = permits.clone().acquire_owned().await?;
        let (stream, _) = listener.accept().await?;
        ensure!(
            stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
            "unexpected local peer"
        );
        let (dir, urls, pin) = (dir.clone(), urls.clone(), pin.clone());
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                handle(stream, &dir, urls, pin),
            )
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("local request timed out")))
            {
                log::warn!("local control: {error}");
            }
        });
    }
}

#[cfg(unix)]
async fn handle(
    stream: tokio::net::UnixStream,
    dir: &Path,
    urls: Vec<String>,
    pin: String,
) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut buffer = Vec::new();
    BufReader::new(read.take((MAX_BYTES + 1) as u64))
        .read_until(b'\n', &mut buffer)
        .await?;
    ensure!(
        buffer.len() <= MAX_BYTES && buffer.last() == Some(&b'\n'),
        "invalid request size or framing"
    );
    let outcome = (|| {
        let control: Control = serde_json::from_slice(&buffer)?;
        let mut store = Store::open(dir)?;
        match control {
            Control::Register { manifest } => Ok(serde_json::to_value(store.register(&manifest)?)?),
            Control::Approve {
                id,
                revision,
                allow_host_execution,
            } => Ok(serde_json::to_value(store.approve(
                &id,
                &revision,
                allow_host_execution,
            )?)?),
            Control::Pair { capabilities } => {
                let mut ticket = store.pairing_ticket(
                    urls.first().context("no pairing addresses")?.clone(),
                    pin,
                    &capabilities,
                )?;
                ticket.urls = urls.into_iter().skip(1).collect();
                Ok(serde_json::to_value(ticket)?)
            }
            Control::Devices => Ok(serde_json::to_value(store.devices()?)?),
            Control::Revoke { id } => {
                store.revoke(&id)?;
                Ok(json!({"revoked":id}))
            }
            Control::Rpc { request } => service::dispatch(&mut store, "local", request),
        }
    })();
    let response = match outcome {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    };
    write.write_all(&serde_json::to_vec(&response)?).await?;
    write.write_all(b"\n").await?;
    Ok(())
}

/// Send one local control request without opening the database directly.
///
/// # Errors
/// Returns connection, framing, or server errors.
#[cfg(unix)]
pub async fn call(dir: &Path, control: Control) -> Result<Value> {
    let mut stream = tokio::net::UnixStream::connect(dir.join("control.sock"))
        .await
        .context("cannot reach xlatch; start xlatch service run first")?;
    let body = serde_json::to_vec(&control)?;
    ensure!(body.len() < MAX_BYTES, "request exceeds 8 MiB");
    stream.write_all(&body).await?;
    stream.write_all(b"\n").await?;
    let mut buffer = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        BufReader::new(stream.take((MAX_BYTES + 1) as u64)).read_until(b'\n', &mut buffer),
    )
    .await??;
    ensure!(buffer.len() <= MAX_BYTES, "response exceeds 8 MiB");
    let response: Value = serde_json::from_slice(&buffer)?;
    ensure!(
        response["ok"] == true,
        "{}",
        response["error"]
            .as_str()
            .unwrap_or("control request failed")
    );
    Ok(response["value"].clone())
}
