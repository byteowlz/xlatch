//! HTTPS capability service with a private local control socket.

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
};
use axum_server::tls_rustls::RustlsConfig;
use clap::Args;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use xlatch_core::{
    auth::{Enrollment, SignedRequest},
    capability::{MAX_BYTES, digest},
    local, service,
    store::Store,
    worker,
};

#[derive(Debug, Clone, Args)]
pub struct Options {
    /// Bind interface; use 0.0.0.0:7443 to accept paired LAN clients.
    #[arg(long, default_value = "0.0.0.0:7443")]
    pub listen: SocketAddr,
    /// Override the listening port (also used in discovered pairing addresses).
    #[arg(long)]
    pub port: Option<u16>,
    /// Reachable HTTPS origin embedded in enrollment QR codes.
    #[arg(long)]
    pub public_url: Option<String>,
    #[arg(long,default_value_t=2,value_parser=clap::value_parser!(u16).range(1..=16))]
    pub workers: u16,
}

#[derive(Clone)]
struct AppState {
    dir: PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

pub async fn run(data_dir: PathBuf, mut cli: Options) -> Result<()> {
    if let Some(port) = cli.port {
        cli.listen.set_port(port);
    }
    let mut origins = crate::network::origins(cli.listen, cli.public_url.as_deref())?;
    let mut store = Store::initialize(&data_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("daemon.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another daemon already uses this data directory")?;
    store.recover()?;
    let (tls, pin) = tls_config(
        &data_dir,
        &mut origins,
        store.devices()?.iter().any(|device| !device.revoked),
    )
    .await?;
    let state = AppState {
        dir: data_dir.clone(),
        permits: Arc::new(tokio::sync::Semaphore::new(16)),
    };
    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"name":"xlatch","version":1})) }),
        )
        .route("/v1/pair", post(pair))
        .route("/v1/rpc", post(rpc))
        .layer(DefaultBodyLimit::max(MAX_BYTES))
        .layer(tower::limit::ConcurrencyLimitLayer::new(32))
        .with_state(state);
    let listener = std::net::TcpListener::bind(cli.listen)
        .with_context(|| format!("cannot listen on {}", cli.listen))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let mut tasks = tokio::task::JoinSet::new();
    #[cfg(unix)]
    tasks.spawn(local::serve(data_dir.clone(), origins.clone(), pin));
    #[cfg(not(unix))]
    anyhow::bail!("v0 local registration currently requires Unix; Windows support is tracked");
    for _ in 0..cli.workers {
        tasks.spawn(worker::run(data_dir.clone()));
    }
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(listener, tls)?
        .handle(handle.clone())
        .serve(app.into_make_service());
    eprintln!("xlatch listening on https://{address}");
    eprintln!("Pairing addresses: {}", origins.join(", "));
    eprintln!("Data directory: {}", data_dir.display());
    if address.ip().is_loopback() {
        eprintln!(
            "Local connections only. For phone access, set --listen and --public-url to a reachable interface and HTTPS origin."
        );
    }
    eprintln!("Run `xlatch pair` in another terminal. Press Ctrl-C to stop.");
    tokio::pin!(server);
    tokio::select! {
        result=&mut server=>result?,
        result=shutdown_signal()=>{result?;handle.graceful_shutdown(Some(Duration::from_secs(3)));server.await?;},
        result=tasks.join_next()=>{return Err(anyhow::anyhow!("background service stopped: {result:?}"));}
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(lock);
    Ok(())
}

async fn tls_config(
    dir: &std::path::Path,
    origins: &mut Vec<String>,
    has_devices: bool,
) -> Result<(RustlsConfig, String)> {
    let cert_path = dir.join("server.pem");
    let key_path = dir.join("server-key.pem");
    let der_path = dir.join("server.der");
    ensure!(
        cert_path.exists() == key_path.exists() && cert_path.exists() == der_path.exists(),
        "incomplete TLS identity; restore the certificate/key/DER set"
    );
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut regenerate = !der_path.exists();
    if !regenerate {
        let der = std::fs::read(&der_path)?;
        let supported = origins
            .iter()
            .filter(|origin| crate::network::certificate_covers(&der, origin))
            .cloned()
            .collect::<Vec<_>>();
        if supported.len() != origins.len() {
            if has_devices {
                ensure!(
                    !supported.is_empty(),
                    "the existing paired certificate covers none of the current addresses; restore the prior network address or revoke devices before re-pairing"
                );
                eprintln!(
                    "Keeping the paired server identity; newly discovered addresses need certificate migration before use."
                );
                *origins = supported;
            } else {
                regenerate = true;
            }
        }
    }
    if regenerate {
        let hosts = origins
            .iter()
            .map(|origin| {
                Ok(url::Url::parse(origin)?
                    .host_str()
                    .context("missing host")?
                    .trim_matches(['[', ']'])
                    .to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(hosts)?;
        std::fs::write(&key_path, signing_key.serialize_pem())?;
        std::fs::write(&cert_path, cert.pem())?;
        std::fs::write(&der_path, cert.der())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let config = RustlsConfig::from_pem_file(cert_path, key_path).await?;
    Ok((config, digest(&std::fs::read(der_path)?)))
}

fn api_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error":message})))
}

async fn pair(State(state): State<AppState>, Json(request): Json<Enrollment>) -> ApiResult {
    let _permit = state
        .permits
        .try_acquire()
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "server busy"))?;
    tokio::task::spawn_blocking(move || {
        Store::open(&state.dir)?
            .enroll(request)
            .map(|v| Json(json!(v)))
    })
    .await
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "request failed"))?
    .map_err(|_| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "Pairing failed. Generate a new QR code and try again.",
        )
    })
}

async fn rpc(State(state): State<AppState>, Json(request): Json<SignedRequest>) -> ApiResult {
    let _permit = state
        .permits
        .try_acquire()
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "server busy"))?;
    tokio::task::spawn_blocking(move || {
        let mut store = Store::open(&state.dir)
            .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable"))?;
        let payload = store.authenticate(&request).map_err(|_| {
            api_error(
                StatusCode::UNAUTHORIZED,
                "Device authentication failed. Check your clock or pair again.",
            )
        })?;
        service::dispatch(&mut store, &request.device_id, payload)
            .map(Json)
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, &e.to_string()))
    })
    .await
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "request failed"))?
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
