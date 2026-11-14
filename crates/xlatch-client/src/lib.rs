//! Paired desktop client shared by CLI and native presentation.
//! Device grants and ownership are always enforced by the remote broker.
mod target;
mod tls;
mod upload;

pub use upload::PreparedFile;

use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use xlatch_wire_auth::{CROSSLATCH_V1, Enrollment, MAX_PAYLOAD_BYTES, SignedRequest};

/// Non-secret client configuration. The adjacent device.json is a credential.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Per-request timeout in seconds (1–300).
    #[schemars(range(min = 1, max = 300))]
    pub timeout_seconds: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout_seconds: 25,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Identity {
    urls: Vec<String>,
    pin: String,
    key_pin: Option<String>,
    device_id: Option<String>,
    seed: String,
}

/// A paired client; Debug intentionally never exposes signing material.
pub struct Client {
    identity: Identity,
    http: reqwest::Client,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("device_id", &self.identity.device_id)
            .finish_non_exhaustive()
    }
}

/// Standard XDG configuration path, with a home-directory fallback.
///
/// # Errors
/// Returns an error when no user directory can be resolved.
pub fn default_config() -> Result<PathBuf> {
    Ok(std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("cannot resolve client config directory")?
        .join("xlatch-client/config.toml"))
}

/// Write defaults only when absent, without enrolling or contacting a server.
///
/// # Errors
/// Returns filesystem errors.
pub fn initialize(path: &Path) -> Result<()> {
    let directory = path.parent().context("config needs a parent directory")?;
    std::fs::create_dir_all(directory)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => file.write_all(toml::to_string_pretty(&Settings::default())?.as_bytes())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn settings(path: &Path) -> Result<Settings> {
    let settings: Settings = config::Config::builder()
        .add_source(config::File::from(path))
        .build()?
        .try_deserialize()?;
    ensure!(
        (1..=300).contains(&settings.timeout_seconds),
        "timeout_seconds must be 1–300"
    );
    Ok(settings)
}

fn identity_path(path: &Path) -> Result<PathBuf> {
    let stem = path
        .file_stem()
        .context("invalid config path")?
        .to_string_lossy();
    Ok(path.with_file_name(format!("{stem}.device.json")))
}

fn private_read(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "credential must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            std::fs::metadata(path)?
                .permissions()
                .mode()
                .trailing_zeros()
                >= 6,
            "credential permissions must be 0600"
        );
    }
    std::fs::read(path).map_err(Into::into)
}

fn save_identity(path: &Path, identity: &Identity) -> Result<()> {
    let directory = path
        .parent()
        .context("credential needs a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(&serde_json::to_vec(identity)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

fn lock(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path.with_extension("lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).context("another client is changing this pairing")?;
    Ok(file)
}

/// Validate an HTTPS origin without credentials, paths, queries or fragments.
///
/// # Errors
/// Rejects origins that could leak signed requests or change endpoint semantics.
pub fn origin(value: &str) -> Result<String> {
    let url = url::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "expected an HTTPS origin without credentials, path, query or fragment"
    );
    Ok(url.as_str().trim_end_matches('/').into())
}

fn timestamp() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?)
}

fn signing_key(seed: &str) -> Result<SigningKey> {
    let bytes: [u8; 32] = STANDARD
        .decode(seed)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid signing key length"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

async fn response(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_PAYLOAD_BYTES,
            "server response exceeds 8 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    ensure!(
        status.is_success(),
        "server rejected request (HTTP {}): {}",
        status.as_u16(),
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid response".into())
    );
    Ok(serde_json::from_slice(&bytes)?)
}

impl Client {
    /// Read an existing pairing. Never starts a daemon or changes credentials.
    ///
    /// # Errors
    /// Rejects absent, unsafe or incomplete credentials and malformed config.
    pub fn load(path: &Path) -> Result<Self> {
        let settings = settings(path)?;
        let identity: Identity = serde_json::from_slice(&private_read(&identity_path(path)?)?)?;
        ensure!(
            identity.device_id.as_ref().is_some_and(|id| !id.is_empty()),
            "pairing outcome is uncertain; obtain a fresh ticket and explicitly forget this local identity before pairing again"
        );
        ensure!(
            !identity.urls.is_empty() && identity.urls.len() <= 8,
            "invalid server origins"
        );
        for url in &identity.urls {
            origin(url)?;
        }
        signing_key(&identity.seed)?;
        let http = tls::http(
            &identity.pin,
            identity.key_pin.clone(),
            settings.timeout_seconds,
        )?;
        Ok(Self { identity, http })
    }

    /// Enroll with a single-use out-of-band JSON ticket. Never replaces a pairing.
    /// A candidate key is saved before POST; an uncertain POST is never replayed.
    ///
    /// # Errors
    /// Returns validation, TLS, server or persistence errors. Pending enrollment
    /// is successful pairing, not a grant; check enrollment status separately.
    pub async fn pair(path: &Path, ticket: &Value, name: &str) -> Result<Value> {
        initialize(path)?;
        let _lock = lock(path)?;
        let credential = identity_path(path)?;
        ensure!(
            !credential.exists(),
            "a local identity already exists; use another --config or explicitly forget it"
        );
        ensure!(ticket["version"] == 1, "unsupported ticket version");
        ensure!(
            ticket["expires_at"]
                .as_i64()
                .context("missing ticket expiry")?
                > timestamp()?,
            "pairing ticket expired"
        );
        let mut urls = vec![origin(
            ticket["url"].as_str().context("missing server URL")?,
        )?];
        if let Some(alternates) = ticket["urls"].as_array() {
            ensure!(alternates.len() <= 8, "too many server origins");
            for url in alternates {
                let url = origin(url.as_str().context("invalid origin")?)?;
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
        ensure!(urls.len() <= 8, "too many server origins");
        let mut seed = [0; 32];
        getrandom::fill(&mut seed)
            .map_err(|error| anyhow::anyhow!("system random source failed: {error}"))?;
        let key = SigningKey::from_bytes(&seed);
        let mut enrollment = Enrollment {
            token: ticket["token"]
                .as_str()
                .context("missing ticket token")?
                .into(),
            name: name.into(),
            public_key: STANDARD.encode(key.verifying_key().as_bytes()),
            signature: String::new(),
        };
        enrollment.signature = STANDARD.encode(
            key.sign(&enrollment.signing_bytes_for(CROSSLATCH_V1))
                .to_bytes(),
        );
        enrollment.verify()?;
        let mut identity = Identity {
            urls,
            pin: ticket["pin"].as_str().context("missing pin")?.into(),
            key_pin: None,
            device_id: None,
            seed: STANDARD.encode(seed),
        };
        let http = tls::http(&identity.pin, None, settings(path)?.timeout_seconds)?;
        let client = Self { identity, http };
        let (url, health) = client.reachable().await?;
        identity = client.identity;
        if let Some(pin) = health["key_pin"].as_str() {
            tls::validate_pin(pin)?;
            identity.key_pin = Some(pin.into());
        }
        // No secrets or token are persisted until validation and read-only discovery pass.
        save_identity(&credential, &identity)?;
        let paired = response(
            client
                .http
                .post(format!("{url}/v1/pair"))
                .json(&enrollment)
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("pairing outcome uncertain; do not automatically resubmit")?,
        )
        .await?;
        identity.device_id = Some(
            paired["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("missing paired device id")?
                .into(),
        );
        save_identity(&credential, &identity)?;
        Ok(paired)
    }

    async fn reachable(&self) -> Result<(String, Value)> {
        for url in &self.identity.urls {
            let reply = self
                .http
                .get(format!("{url}/health"))
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await;
            if let Ok(reply) = reply
                && let Ok(health) = response(reply).await
                && health["name"] == "xlatch"
                && health["version"] == 1
            {
                return Ok((url.clone(), health));
            }
        }
        anyhow::bail!(
            "no pinned xlatch origin reachable; check network, hostname, certificate and device clock"
        )
    }

    /// Send exactly one signed POST after read-only pinned health discovery.
    /// Retrying an invocation requires the same application idempotency key.
    ///
    /// # Errors
    /// Returns bounded transport, authentication, grant and server errors.
    pub async fn call(&self, request: &Value) -> Result<Value> {
        let payload = serde_json::to_string(request)?;
        ensure!(
            payload.len() <= MAX_PAYLOAD_BYTES,
            "payload exceeds request limit"
        );
        let (url, _) = self.reachable().await?;
        let mut envelope = SignedRequest {
            device_id: self
                .identity
                .device_id
                .clone()
                .context("device not paired")?,
            timestamp: timestamp()?,
            nonce: uuid::Uuid::new_v4().to_string(),
            payload,
            signature: String::new(),
        };
        envelope.signature = STANDARD.encode(
            signing_key(&self.identity.seed)?
                .sign(&envelope.signing_bytes())
                .to_bytes(),
        );
        ensure!(
            serde_json::to_vec(&envelope)?.len() <= MAX_PAYLOAD_BYTES,
            "signed request exceeds 8 MiB"
        );
        response(
            self.http
                .post(format!("{url}/v1/rpc"))
                .json(&envelope)
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("request outcome uncertain; retain the same invocation key")?,
        )
        .await
    }
}

/// Forget local credentials only; this does NOT revoke the remote device.
///
/// # Errors
/// Returns filesystem or concurrent-pairing errors.
pub fn forget(path: &Path) -> Result<()> {
    let _lock = lock(path)?;
    std::fs::remove_file(identity_path(path)?)?;
    Ok(())
}

/// JSON Schema for version 1 CLI envelopes. Domain values retain the broker's
/// existing capability, enrollment and job contracts.
#[must_use]
pub fn output_schema() -> Value {
    json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "title":"xlatch-client output v1",
        "oneOf":[
            {"type":"object","required":["schema_version","ok","value"],"additionalProperties":false,
             "properties":{"schema_version":{"const":1},"ok":{"const":true},"value":{}}},
            {"type":"object","required":["schema_version","ok","error"],"additionalProperties":false,
             "properties":{"schema_version":{"const":1},"ok":{"const":false},"error":{
                "type":"object","required":["code","message"],"additionalProperties":false,
                "properties":{"code":{"enum":["invalid_arguments","client_error"]},"message":{"type":"string"}}
             }}}
        ]
    })
}

/// Attach an explicit instruction to native file input and validate the exact
/// combined input against the selected reviewed contract, before upload.
///
/// # Errors
/// Rejects blank/oversized prompts, non-file input, save-only actions that would
/// discard the prompt, and contracts that do not accept the full combined input.
pub fn file_with_prompt(target: &Value, mut input: Value, prompt: &str) -> Result<Value> {
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 64 * 1024,
        "prompt must contain 1–65536 bytes"
    );
    ensure!(
        input["file"].is_object(),
        "a file is required with a prompt"
    );
    ensure!(
        target["manifest"]["execution"]["kind"] != "save_file",
        "a save-only action would discard the prompt; choose a reviewed session/context action"
    );
    input
        .as_object_mut()
        .context("invalid native input")?
        .insert("text".into(), json!(prompt));
    let validator = jsonschema::validator_for(&target["manifest"]["input_schema"])
        .map_err(|_| anyhow::anyhow!("invalid target input schema"))?;
    ensure!(
        validator.is_valid(&input),
        "target schema does not accept this file with its prompt"
    );
    Ok(input)
}

/// Build the native text-share input.
#[must_use]
pub fn text_input(text: &str, mime_type: &str) -> Value {
    json!({"text": text, "mime_type": mime_type})
}

/// Build a small native file-share input. Large resumable uploads are not
/// silently attempted; this minimal client currently supports files up to 4 `MiB`.
///
/// # Errors
/// Rejects oversized or non-regular files and unreadable names/content.
pub fn file_input(path: &Path, mime_type: &str) -> Result<Value> {
    let metadata = std::fs::metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 4 * 1024 * 1024,
        "send a regular file of at most 4 MiB"
    );
    let bytes = crate::read_bounded(std::fs::File::open(path)?, 4 * 1024 * 1024)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid file name")?;
    Ok(
        json!({"file":{"name":name,"mime_type":mime_type,"data_base64":STANDARD.encode(bytes)},"mime_type":mime_type}),
    )
}

/// Read a bounded stream, including stdin.
///
/// # Errors
/// Rejects I/O failures or more than the requested byte limit.
pub fn read_bounded(reader: impl std::io::Read, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "input exceeds {limit} bytes");
    Ok(bytes)
}
