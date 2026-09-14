//! HTTPS capability service with a private local control socket.

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
};
use clap::Args;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use xlatch_core::{
    auth::{Enrollment, SignedRequest},
    capability::MAX_BYTES,
    local, service,
    store::Store,
    worker,
};

#[derive(Debug, Clone, Args)]
pub struct Options {
    /// Administrator-owned protected service configuration.
    #[arg(long)]
    pub protected_config: Option<PathBuf>,
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
    let protected = crate::protected::load(cli.protected_config.as_deref(), &data_dir)?;
    let origins = crate::network::origins(cli.listen, cli.public_url.as_deref())?;
    let (mut store, lock) = open_store(&data_dir)?;
    let notifications = crate::notifications::prepare(&data_dir)?;
    if protected.is_some() {
        crate::protected::require_guard(&data_dir)?;
    }
    store.recover()?;
    let (tls, pin) = crate::tls::prepare(&data_dir, &origins).await?;
    let state = AppState {
        dir: data_dir.clone(),
        permits: Arc::new(tokio::sync::Semaphore::new(16)),
    };
    let app = Router::new()
        .route("/health", get(health))
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
    tasks.spawn(crate::tls::maintain(
        data_dir.clone(),
        cli.listen,
        cli.public_url.clone(),
        tls.clone(),
    ));
    #[cfg(unix)]
    if let Some(config) = &protected {
        tasks.spawn(local::serve_scoped(
            data_dir.clone(),
            config.control_dir.clone(),
            origins.clone(),
            pin,
            Some(config.executor_uid),
        ));
    } else {
        tasks.spawn(local::serve(data_dir.clone(), origins.clone(), pin));
    }
    #[cfg(not(unix))]
    anyhow::bail!("v0 local registration currently requires Unix; Windows support is tracked");
    if protected.is_none() {
        for _ in 0..cli.workers {
            tasks.spawn(worker::run(data_dir.clone()));
        }
    } else {
        tasks.spawn(crate::protected::reap_leases(data_dir.clone()));
        eprintln!("Protected service: execution runs in a separate unprivileged process.");
    }
    if let Some(notifications) = notifications {
        tasks.spawn(notifications.run(data_dir.clone()));
        eprintln!("External notifications enabled (generic job status only).");
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

async fn health(State(state): State<AppState>) -> ApiResult {
    crate::tls::health(&state.dir)
        .map(Json)
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "TLS identity unavailable"))
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

fn open_store(data_dir: &std::path::Path) -> Result<(Store, std::fs::File)> {
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("daemon.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another daemon already uses this data directory")?;
    Ok((Store::initialize(data_dir)?, lock))
}
