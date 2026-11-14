//! Synthetic pinned HTTPS proofs against the real authorization/job substrate.
use anyhow::{Context as _, Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use xlatch_client::{Client, file_input, text_input};
use xlatch_core::{
    auth::{Enrollment, SignedRequest},
    capability::{Manifest, digest},
    service,
    store::Store,
};

#[derive(Clone)]
struct TestState {
    directory: PathBuf,
    key_pin: String,
    posts: Arc<AtomicUsize>,
    unavailable: Arc<AtomicBool>,
    redirect: Arc<AtomicBool>,
    traps: Arc<AtomicUsize>,
    fail_chunk_once: Arc<AtomicBool>,
    lose_invoke_ack: Arc<AtomicBool>,
    chunks: Arc<AtomicUsize>,
}

struct Server {
    _directory: tempfile::TempDir,
    state: TestState,
    url: String,
    pin: String,
    revision: String,
    key: rcgen::KeyPair,
    tls: axum_server::tls_rustls::RustlsConfig,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    worker: tokio::task::JoinHandle<Result<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        self.worker.abort();
    }
}

macro_rules! equal {
    ($left:expr, $right:expr $(,)?) => {
        ensure!(
            $left == $right,
            "expected full equality: {:?} != {:?}",
            $left,
            $right
        )
    };
}

type ApiResult = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;

fn failure(error: &anyhow::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": error.to_string()})),
    )
}

async fn health(State(state): State<TestState>) -> Response {
    if state.redirect.load(Ordering::SeqCst) {
        return axum::response::Redirect::temporary("/trap").into_response();
    }
    Json(json!({"name":"xlatch","version":1,"key_pin":state.key_pin})).into_response()
}

async fn pair(State(state): State<TestState>, Json(enrollment): Json<Enrollment>) -> ApiResult {
    state.posts.fetch_add(1, Ordering::SeqCst);
    let value = Store::open(&state.directory)
        .and_then(|mut store| store.enroll(enrollment))
        .and_then(|device| Ok(serde_json::to_value(device)?))
        .map_err(|error| failure(&error))?;
    Ok(Json(value))
}

async fn rpc(State(state): State<TestState>, Json(envelope): Json<SignedRequest>) -> ApiResult {
    state.posts.fetch_add(1, Ordering::SeqCst);
    if state.unavailable.load(Ordering::SeqCst) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"fixture unavailable"})),
        ));
    }
    let request: Value =
        serde_json::from_str(&envelope.payload).map_err(|error| failure(&error.into()))?;
    let chunk = request["request"]["action"] == "chunk";
    let value = (|| {
        let mut store = Store::open(&state.directory)?;
        let request = store.authenticate(&envelope)?;
        service::dispatch(&mut store, &envelope.device_id, request)
    })()
    .map_err(|error| failure(&error))?;
    if request["op"] == "invoke" && state.lose_invoke_ack.swap(false, Ordering::SeqCst) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"fixture lost invocation acknowledgement"})),
        ));
    }
    if chunk {
        state.chunks.fetch_add(1, Ordering::SeqCst);
        if state.fail_chunk_once.swap(false, Ordering::SeqCst) {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"fixture lost chunk acknowledgement"})),
            ));
        }
    }
    Ok(Json(value))
}

impl Server {
    async fn start(host: &str) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let directory = tempfile::tempdir()?;
        let store = Store::initialize(directory.path())?;
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
        let registered = store.register(&manifest)?;
        store.approve("echo", &registered.revision, false)?;
        let mut hidden = manifest;
        hidden.id = "not-granted".into();
        let hidden = store.register(&hidden)?;
        store.approve(&hidden.manifest.id, &hidden.revision, false)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("https://127.0.0.1:{}", listener.local_addr()?.port());
        let key = rcgen::KeyPair::generate()?;
        let cert = rcgen::CertificateParams::new(vec![host.into()])?.self_signed(&key)?;
        let pin = digest(cert.der());
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert.pem().into_bytes(),
            key.serialize_pem().into_bytes(),
        )
        .await?;
        let state = TestState {
            directory: directory.path().into(),
            key_pin: digest(key.public_key_raw()),
            posts: Arc::default(),
            unavailable: Arc::default(),
            redirect: Arc::default(),
            traps: Arc::default(),
            fail_chunk_once: Arc::default(),
            lose_invoke_ack: Arc::default(),
            chunks: Arc::default(),
        };
        let app = Router::new()
            .route("/health", get(health))
            .route("/v1/pair", post(pair))
            .route("/v1/rpc", post(rpc))
            .route(
                "/trap",
                get(|State(state): State<TestState>| async move {
                    state.traps.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"name":"xlatch","version":1}))
                }),
            )
            .with_state(state.clone());
        let task = tokio::spawn(
            axum_server::from_tcp_rustls(listener, tls.clone())?.serve(app.into_make_service()),
        );
        let worker = tokio::spawn(xlatch_core::worker::run(directory.path().into()));
        Ok(Self {
            _directory: directory,
            state,
            url,
            pin,
            revision: registered.revision,
            key,
            tls,
            task,
            worker,
        })
    }

    fn prompted_ticket(&self) -> Result<(Value, String)> {
        let store = Store::open(&self.state.directory)?;
        let mut manifest = store.capability("echo")?.manifest;
        manifest.id = "context.echo".into();
        manifest.file_input = Some(xlatch_core::uploads::FileInput::Path);
        manifest.accepts = vec!["application/octet-stream".into()];
        manifest.input_schema = json!({"type":"object","required":["file","mime_type","text"],
            "additionalProperties":false,"properties":{"file":{"type":"object"},
            "mime_type":{"const":"application/octet-stream"},"text":{"type":"string","minLength":1}}});
        manifest.output_schema = json!(true);
        let capability = store.register(&manifest)?;
        store.approve(&manifest.id, &capability.revision, false)?;
        let ticket = store.pairing_ticket(self.url.clone(), self.pin.clone(), &[manifest.id])?;
        Ok((serde_json::to_value(ticket)?, capability.revision))
    }

    fn ticket(&self) -> Result<Value> {
        Ok(serde_json::to_value(
            Store::open(&self.state.directory)?.pairing_ticket(
                self.url.clone(),
                self.pin.clone(),
                &["echo".into()],
            )?,
        )?)
    }
}

#[tokio::test]
async fn real_pair_grants_text_file_jobs_idempotency_and_revocation() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    let paired = Client::pair(&config, &server.ticket()?, "Synthetic desktop").await?;
    let client = Client::load(&config)?;
    let discovered = client.call(&json!({"op":"discover"})).await?;
    let expected = Store::open(&server.state.directory)?.capability("echo")?;
    equal!(discovered, json!([expected]));
    let request = json!({"op":"invoke","capability_id":"echo","revision":server.revision,
        "input":text_input("Desktop fixture", "text/plain"),"idempotency_key":"desktop-fixture-text"});
    let first = client.call(&request).await?;
    let second = client.call(&request).await?;
    equal!(first["id"], second["id"]);
    let mut job = first;
    for _ in 0..40 {
        job = client.call(&json!({"op":"job","id":job["id"]})).await?;
        if job["status"] == "succeeded" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    equal!(job["result"], text_input("Desktop fixture", "text/plain"));
    let file = directory.path().join("fixture.txt");
    std::fs::write(&file, "file fixture")?;
    let file_payload = file_input(&file, "text/plain")?;
    equal!(
        file_payload,
        json!({"file":{"name":"fixture.txt","mime_type":"text/plain","data_base64":STANDARD.encode("file fixture")},"mime_type":"text/plain"}),
    );
    client
        .call(
            &json!({"op":"invoke","capability_id":"echo","revision":server.revision,
        "input":file_payload,"idempotency_key":"desktop-fixture-file"}),
        )
        .await?;
    let mut stale = request.clone();
    stale["revision"] = json!("0".repeat(64));
    stale["idempotency_key"] = json!("stale");
    ensure!(
        client.call(&stale).await.is_err(),
        "stale revision accepted"
    );
    let mut denied = request;
    denied["capability_id"] = json!("not-granted");
    denied["idempotency_key"] = json!("not-granted");
    ensure!(
        client.call(&denied).await.is_err(),
        "ungranted action accepted"
    );
    let other_config = directory.path().join("other.toml");
    Client::pair(&other_config, &server.ticket()?, "Other desktop").await?;
    let other = Client::load(&other_config)?;
    ensure!(
        other
            .call(&json!({"op":"job","id":job["id"]}))
            .await
            .is_err(),
        "cross-device job disclosed"
    );
    ensure!(
        Client::pair(&config, &server.ticket()?, "Replacement")
            .await
            .is_err(),
        "existing pairing replaced"
    );
    Store::open(&server.state.directory)?.revoke(paired["id"].as_str().context("missing id")?)?;
    ensure!(
        client.call(&json!({"op":"discover"})).await.is_err(),
        "revoked device authenticated"
    );
    Ok(())
}

#[tokio::test]
async fn pins_hostnames_redirects_and_single_post_fail_closed() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    let mut wrong = server.ticket()?;
    wrong["pin"] = json!("0".repeat(64));
    ensure!(Client::pair(&config, &wrong, "Wrong pin").await.is_err());
    equal!(server.state.posts.load(Ordering::SeqCst), 0);
    server.state.redirect.store(true, Ordering::SeqCst);
    ensure!(
        Client::pair(&config, &server.ticket()?, "Redirect")
            .await
            .is_err()
    );
    equal!(server.state.traps.load(Ordering::SeqCst), 0);
    equal!(server.state.posts.load(Ordering::SeqCst), 0);
    server.state.redirect.store(false, Ordering::SeqCst);
    // An unavailable candidate is probed, not sent the enrollment body.
    let mut fallback = server.ticket()?;
    fallback["url"] = json!("https://127.0.0.1:1");
    fallback["urls"] = json!([server.url]);
    Client::pair(&config, &fallback, "Fallback").await?;
    let client = Client::load(&config)?;
    server.state.unavailable.store(true, Ordering::SeqCst);
    let before = server.state.posts.load(Ordering::SeqCst);
    ensure!(client.call(&json!({"op":"discover"})).await.is_err());
    equal!(server.state.posts.load(Ordering::SeqCst), before + 1);
    let wrong_host = Server::start("wrong.invalid").await?;
    ensure!(
        Client::pair(
            &directory.path().join("hostname.toml"),
            &wrong_host.ticket()?,
            "Wrong host"
        )
        .await
        .is_err()
    );
    equal!(wrong_host.state.posts.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn certificate_renewal_keeps_key_but_replacement_is_rejected() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    Client::pair(&config, &server.ticket()?, "Renewal").await?;
    let renewed =
        rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?.self_signed(&server.key)?;
    ensure!(digest(renewed.der()) != server.pin);
    server
        .tls
        .reload_from_pem(
            renewed.pem().into_bytes(),
            server.key.serialize_pem().into_bytes(),
        )
        .await?;
    Client::load(&config)?
        .call(&json!({"op":"discover"}))
        .await?;
    let replacement = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
    server
        .tls
        .reload_from_pem(
            replacement.cert.pem().into_bytes(),
            replacement.signing_key.serialize_pem().into_bytes(),
        )
        .await?;
    ensure!(
        Client::load(&config)?
            .call(&json!({"op":"discover"}))
            .await
            .is_err(),
        "replacement TLS key trusted"
    );
    Ok(())
}

#[tokio::test]
async fn large_cli_file_resumes_lost_ack_and_retries_the_same_job() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let incoming = server.state.directory.join("incoming");
    std::fs::create_dir(&incoming)?;
    let incoming = std::fs::canonicalize(incoming)?;
    let store = Store::open(&server.state.directory)?;
    let mut manifest = store.capability("echo")?.manifest;
    manifest.id = "save.large".into();
    manifest.execution = xlatch_core::capability::Execution::SaveFile {
        directory: incoming.to_string_lossy().into_owned(),
    };
    let registered = store.register(&manifest)?;
    store.approve("save.large", &registered.revision, false)?;
    let ticket = serde_json::to_value(store.pairing_ticket(
        server.url.clone(),
        server.pin.clone(),
        &["save.large".into()],
    )?)?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    Client::pair(&config, &ticket, "Large file CLI").await?;
    let file = directory.path().join("recording.ctx");
    let content = vec![0x5a; 4 * 1024 * 1024 + 123];
    std::fs::write(&file, &content)?;
    let run = || {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_xlatch-client"));
        command
            .arg("--config")
            .arg(&config)
            .args([
                "--json",
                "send",
                "save.large",
                "--revision",
                &registered.revision,
                "--key",
                "large-fixture",
                "--file",
            ])
            .arg(&file)
            .args(["--mime-type", "application/octet-stream", "--wait"]);
        command
    };
    server.state.fail_chunk_once.store(true, Ordering::SeqCst);
    let uncertain = run().output().await?;
    ensure!(
        !uncertain.status.success(),
        "lost acknowledgement did not interrupt transfer"
    );
    equal!(server.state.chunks.load(Ordering::SeqCst), 1);
    let accepted = run().output().await?;
    ensure!(
        accepted.status.success(),
        "large send failed: {}",
        String::from_utf8_lossy(&accepted.stdout)
    );
    let receipt: Value = serde_json::from_slice(&accepted.stdout)?;
    equal!(receipt["value"]["status"], "succeeded");
    equal!(receipt["value"]["result"]["bytes"], content.len() as u64);
    equal!(std::fs::read(incoming.join("recording.ctx"))?, content);
    equal!(server.state.chunks.load(Ordering::SeqCst), 5);
    let retry = run().output().await?;
    ensure!(retry.status.success());
    let repeated: Value = serde_json::from_slice(&retry.stdout)?;
    equal!(repeated, receipt);
    equal!(std::fs::read_dir(&incoming)?.count(), 1);
    // Changing bytes with the same key may upload a distinct artifact but must
    // never silently return/replace the earlier job as if its input matched.
    std::fs::write(&file, vec![0x6b; content.len()])?;
    let changed = run().output().await?;
    ensure!(
        !changed.status.success(),
        "changed bytes reused a job identity"
    );
    equal!(std::fs::read_dir(&incoming)?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn prompted_cli_freezes_input_and_reconciles_lost_invoke_ack() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let (ticket, revision) = server.prompted_ticket()?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    Client::pair(&config, &ticket, "Prompt fixture").await?;
    let source = directory.path().join("context.bin");
    std::fs::write(&source, vec![0x5a; 4 * 1024 * 1024 + 17])?;
    let prompt = directory.path().join("prompt.txt");
    std::fs::write(
        &prompt,
        "Inspect this explicit context, do not dispatch anything.",
    )?;
    let run = || {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_xlatch-client"));
        command
            .arg("--config")
            .arg(&config)
            .args([
                "--json",
                "send",
                "context.echo",
                "--revision",
                &revision,
                "--key",
                "prompted-fixture",
                "--file",
            ])
            .arg(&source)
            .args(["--mime-type", "application/octet-stream", "--prompt-file"])
            .arg(&prompt)
            .arg("--wait");
        command
    };
    server.state.lose_invoke_ack.store(true, Ordering::SeqCst);
    let uncertain = run().output().await?;
    ensure!(!uncertain.status.success());
    let retry = run().output().await?;
    ensure!(
        retry.status.success(),
        "retry failed: {}",
        String::from_utf8_lossy(&retry.stdout)
    );
    let accepted: Value = serde_json::from_slice(&retry.stdout)?;
    equal!(accepted["value"]["status"], "succeeded");
    equal!(
        accepted["value"]["result"]["text"],
        std::fs::read_to_string(&prompt)?
    );
    equal!(server.state.chunks.load(Ordering::SeqCst), 5);
    let repeat = run().output().await?;
    equal!(serde_json::from_slice::<Value>(&repeat.stdout)?, accepted);
    std::fs::write(
        &prompt,
        "A new instruction must not mutate the accepted intent.",
    )?;
    let changed = run().output().await?;
    ensure!(
        !changed.status.success(),
        "changed prompt reused an accepted job"
    );
    let jobs = Client::load(&config)?.call(&json!({"op":"jobs"})).await?;
    equal!(jobs.as_array().context("missing jobs")?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn local_preparation_is_frozen_and_unsupported_prompt_never_uploads() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let (ticket, revision) = server.prompted_ticket()?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    let paired = Client::pair(&config, &ticket, "Preflight fixture").await?;
    let client = Client::load(&config)?;
    let source = directory.path().join("context.bin");
    for size in [7, 4 * 1024 * 1024 + 7] {
        std::fs::write(&source, vec![0x61; size])?;
        let posts = server.state.posts.load(Ordering::SeqCst);
        let mut prepared = client.prepare_file(&source, "application/octet-stream", "frozen")?;
        equal!(server.state.posts.load(Ordering::SeqCst), posts);
        let first = prepared.input().clone();
        std::fs::write(&source, vec![0x62; size])?;
        prepared.upload(&client).await?;
        equal!(prepared.input(), &first);
        let changed = client.prepare_file(&source, "application/octet-stream", "frozen")?;
        ensure!(
            changed.input() != prepared.input(),
            "changed bytes reused frozen identity"
        );
    }
    // Same reviewed capability/revision, but revoke its prompt schema. A new
    // revision is required and the CLI must reject it before beginning upload.
    let store = Store::open(&server.state.directory)?;
    let mut manifest = store.capability("context.echo")?.manifest;
    manifest.input_schema["properties"]
        .as_object_mut()
        .context("properties")?
        .remove("text");
    manifest.input_schema["required"] = json!(["file", "mime_type"]);
    let changed = store.register(&manifest)?;
    store.approve(&manifest.id, &changed.revision, false)?;
    ensure!(changed.revision != revision);
    store.grant(
        paired["id"].as_str().context("missing device")?,
        &manifest.id,
        &changed.revision,
    )?;
    let posts = server.state.posts.load(Ordering::SeqCst);
    let rejected = tokio::process::Command::new(env!("CARGO_BIN_EXE_xlatch-client"))
        .arg("--config")
        .arg(&config)
        .args([
            "--json",
            "send",
            "context.echo",
            "--revision",
            &changed.revision,
            "--key",
            "unsupported",
            "--file",
        ])
        .arg(&source)
        .args([
            "--mime-type",
            "application/octet-stream",
            "--prompt",
            "Inspect the bundle",
        ])
        .output()
        .await?;
    ensure!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stdout)?;
    ensure!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("target schema")),
        "unexpected preflight error: {error}"
    );
    equal!(server.state.posts.load(Ordering::SeqCst), posts + 1);
    Ok(())
}

#[test]
fn prompted_input_requires_explicit_supported_contract() -> Result<()> {
    let input = json!({"file":{"name":"fixture.bin","data_base64":"YQ==","mime_type":"application/octet-stream"},"mime_type":"application/octet-stream"});
    let mut target = json!({"manifest":{"execution":{"kind":"echo"},"input_schema":{
        "type":"object","required":["file","mime_type","text"],"additionalProperties":false,
        "properties":{"file":{"type":"object"},"mime_type":{"type":"string"},"text":{"type":"string"}}}}});
    let mut expected = input.clone();
    expected["text"] = json!("Review this");
    let actual = xlatch_client::file_with_prompt(&target, input.clone(), "Review this")?;
    equal!(actual, expected);
    ensure!(xlatch_client::file_with_prompt(&target, input.clone(), " ").is_err());
    ensure!(xlatch_client::file_with_prompt(&target, input.clone(), &"x".repeat(65537)).is_err());
    ensure!(
        xlatch_client::file_with_prompt(&target, json!({"text":"not a file"}), "Review").is_err()
    );
    target["manifest"]["execution"]["kind"] = json!("save_file");
    ensure!(xlatch_client::file_with_prompt(&target, input.clone(), "Review").is_err());
    target["manifest"]["execution"]["kind"] = json!("echo");
    target["manifest"]["input_schema"]["properties"]
        .as_object_mut()
        .context("properties")?
        .remove("text");
    ensure!(xlatch_client::file_with_prompt(&target, input, "Review").is_err());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn credentials_reject_broad_permissions_and_symlinks() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    Client::pair(&config, &server.ticket()?, "Credential fixture").await?;
    let credential = directory.path().join("config.device.json");
    equal!(
        std::fs::metadata(&credential)?.permissions().mode() & 0o777,
        0o600
    );
    let identity: Value = serde_json::from_slice(&std::fs::read(&credential)?)?;
    let debug = format!("{:?}", Client::load(&config)?);
    ensure!(
        !debug.contains(identity["seed"].as_str().context("missing seed")?),
        "debug leaked private key"
    );
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o644))?;
    ensure!(
        Client::load(&config).is_err(),
        "world-readable signing key accepted"
    );
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600))?;
    let moved = directory.path().join("moved.json");
    std::fs::rename(&credential, &moved)?;
    symlink(&moved, &credential)?;
    ensure!(
        Client::load(&config).is_err(),
        "symlinked signing key accepted"
    );
    Ok(())
}

#[tokio::test]
async fn pending_enrollment_retains_identity_without_grants() -> Result<()> {
    use p256::ecdsa::signature::Signer as _;
    use xlatch_core::enrollment::{EnrollmentRequest, enable_bytes};
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let owner_config = directory.path().join("owner.toml");
    let owner = Client::pair(&owner_config, &server.ticket()?, "Fixture approver").await?;
    let owner = owner["id"].as_str().context("missing approver id")?;
    let mut store = Store::open(&server.state.directory)?;
    let token = store.enrollment_bootstrap(owner)?["token"]
        .as_str()
        .context("missing bootstrap")?
        .to_owned();
    let status = service::dispatch(
        &mut store,
        owner,
        xlatch_core::capability::Request::Enrollment {
            request: EnrollmentRequest::Status,
        },
    )?;
    let key = p256::ecdsa::SigningKey::from_slice(&[42; 32])?;
    let public_key = STANDARD.encode(key.verifying_key().to_encoded_point(false).as_bytes());
    let signature: p256::ecdsa::Signature = key.sign(&enable_bytes(
        status["server_id"].as_str().context("missing server")?,
        owner,
        &token,
        &public_key,
    ));
    service::dispatch(
        &mut store,
        owner,
        xlatch_core::capability::Request::Enrollment {
            request: EnrollmentRequest::Enable {
                token,
                public_key,
                signature: STANDARD.encode(signature.to_der().as_bytes()),
            },
        },
    )?;
    let config = directory.path().join("pending.toml");
    let paired = Client::pair(&config, &server.ticket()?, "Pending desktop").await?;
    equal!(paired["enrollment_status"], "pending");
    let client = Client::load(&config)?;
    let status = client
        .call(&json!({"op":"enrollment","request":{"action":"status"}}))
        .await?;
    equal!(status["device_status"], "pending");
    ensure!(
        status["pending_payload"].as_str().is_some(),
        "comparison payload missing"
    );
    ensure!(
        client.call(&json!({"op":"discover"})).await.is_err(),
        "pending enrollment received grants"
    );
    Ok(())
}

#[test]
fn input_config_and_credentials_are_bounded_and_private() -> Result<()> {
    for invalid in [
        "http://localhost",
        "https://user:secret@localhost",
        "https://localhost/path",
        "https://localhost/?query",
        "https://localhost/#fragment",
    ] {
        ensure!(xlatch_client::origin(invalid).is_err());
    }
    equal!(
        xlatch_client::origin("https://localhost:7443/")?,
        "https://localhost:7443",
    );
    ensure!(xlatch_client::read_bounded(&b"12345"[..], 4).is_err());
    equal!(xlatch_client::read_bounded(&b"1234"[..], 4)?, b"1234");
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    xlatch_client::initialize(&config)?;
    let contents = std::fs::read_to_string(&config)?;
    xlatch_client::initialize(&config)?;
    equal!(std::fs::read_to_string(&config)?, contents);
    ensure!(Client::load(&config).is_err());
    let large = directory.path().join("large.bin");
    let file = std::fs::File::create(&large)?;
    file.set_len(4 * 1024 * 1024 + 1)?;
    ensure!(file_input(&large, "application/octet-stream").is_err());
    Ok(())
}

#[tokio::test]
async fn cli_and_library_share_identity_jobs_and_machine_errors() -> Result<()> {
    let server = Server::start("127.0.0.1").await?;
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("config.toml");
    let binary = env!("CARGO_BIN_EXE_xlatch-client");
    let ticket = directory.path().join("ticket.json");
    std::fs::write(&ticket, serde_json::to_vec(&server.ticket()?)?)?;
    let paired = tokio::process::Command::new(binary)
        .arg("--config")
        .arg(&config)
        .args(["--json", "pair"])
        .arg(&ticket)
        .args(["--name", "CLI fixture"])
        .output()
        .await?;
    ensure!(paired.status.success(), "CLI pairing failed");
    let paired: Value = serde_json::from_slice(&paired.stdout)?;
    equal!(paired["value"]["enrollment_status"], "active");
    let output = tokio::process::Command::new(binary)
        .arg("--config")
        .arg(&config)
        .args([
            "--json",
            "send",
            "echo",
            "--revision",
            &server.revision,
            "--key",
            "cli-fixture",
            "--text",
            "CLI fixture",
            "--wait",
        ])
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    equal!(reply["ok"], true);
    ensure!(jsonschema::validator_for(&xlatch_client::output_schema())?.is_valid(&reply));
    equal!(
        reply["value"]["result"],
        text_input("CLI fixture", "text/plain"),
    );
    let library = Client::load(&config)?
        .call(&json!({"op":"job","id":reply["value"]["id"]}))
        .await?;
    equal!(library, reply["value"]);
    let invalid = tokio::process::Command::new(binary)
        .args(["--json", "send", "echo"])
        .output()
        .await?;
    ensure!(!invalid.status.success());
    let error: Value = serde_json::from_slice(&invalid.stdout)?;
    equal!(error["error"]["code"], "invalid_arguments");
    ensure!(jsonschema::validator_for(&xlatch_client::output_schema())?.is_valid(&error));
    let missing = tokio::process::Command::new(binary)
        .arg("--config")
        .arg(directory.path().join("absent.toml"))
        .args(["--json", "list"])
        .output()
        .await?;
    ensure!(!missing.status.success());
    let error: Value = serde_json::from_slice(&missing.stdout)?;
    equal!(error["error"]["code"], "client_error");
    ensure!(jsonschema::validator_for(&xlatch_client::output_schema())?.is_valid(&error));
    Ok(())
}
