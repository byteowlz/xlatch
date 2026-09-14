//! Persistent TLS key with renewable certificates and authenticated address advertisements.
use anyhow::{Context, Result, ensure};
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::{net::SocketAddr, path::Path, time::Duration};
use xlatch_core::capability::digest;

const FILE: &str = "tls-identity.json";
const LIFETIME_DAYS: i64 = 90;

#[derive(Serialize, Deserialize)]
struct Identity {
    pem: String,
    der: Vec<u8>,
    urls: Vec<String>,
    pin: String,
    key_pin: String,
    renew_at: i64,
}

impl Identity {
    fn issue(key: &rcgen::KeyPair, urls: Vec<String>) -> Result<Self> {
        let hosts = urls
            .iter()
            .map(|origin| {
                Ok(url::Url::parse(origin)?
                    .host_str()
                    .context("missing TLS host")?
                    .trim_matches(['[', ']'])
                    .to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        let now = time::OffsetDateTime::now_utc();
        let mut params = rcgen::CertificateParams::new(hosts)?;
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(LIFETIME_DAYS);
        let cert = params.self_signed(key)?;
        Ok(Self {
            pem: cert.pem(),
            der: cert.der().to_vec(),
            urls,
            pin: digest(cert.der()),
            key_pin: digest(key.public_key_raw()),
            renew_at: (now + time::Duration::days(60)).unix_timestamp(),
        })
    }
    fn save(&self, dir: &Path) -> Result<()> {
        let pending = dir.join("tls-identity.json.pending");
        std::fs::write(&pending, serde_json::to_vec(self)?)?;
        std::fs::rename(pending, dir.join(FILE))?;
        // Compatibility exports; the JSON snapshot is authoritative after migration.
        std::fs::write(dir.join("server.pem"), &self.pem)?;
        std::fs::write(dir.join("server.der"), &self.der)?;
        Ok(())
    }
}

pub async fn prepare(dir: &Path, urls: &[String]) -> Result<(RustlsConfig, String)> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key_path = dir.join("server-key.pem");
    if !key_path.exists() {
        ensure!(
            !dir.join(FILE).exists() && !dir.join("server.pem").exists(),
            "TLS key missing; restore the server identity"
        );
        let key = rcgen::KeyPair::generate()?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&key_path)?
            .write_all(key.serialize_pem().as_bytes())?;
    }
    let pem = std::fs::read_to_string(key_path)?;
    let key = rcgen::KeyPair::from_pem(&pem)?;
    let identity = if dir.join(FILE).exists() {
        let stored: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
        ensure!(
            stored.key_pin == digest(key.public_key_raw()),
            "TLS key changed; restore the server identity"
        );
        stored
    } else if dir.join("server.der").exists() {
        // Preserve the old certificate so installed clients can learn the stable key first.
        let der = std::fs::read(dir.join("server.der"))?;
        Identity {
            pem: std::fs::read_to_string(dir.join("server.pem"))?,
            pin: digest(&der),
            der,
            urls: urls.to_vec(),
            key_pin: digest(key.public_key_raw()),
            renew_at: (time::OffsetDateTime::now_utc() + time::Duration::days(60)).unix_timestamp(),
        }
    } else {
        Identity::issue(&key, urls.to_vec())?
    };
    let config = RustlsConfig::from_pem(identity.pem.as_bytes().to_vec(), pem.into_bytes()).await?;
    identity.save(dir)?;
    refresh(dir, urls, &config).await?;
    let current: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
    Ok((config, current.pin))
}

async fn refresh(dir: &Path, urls: &[String], config: &RustlsConfig) -> Result<()> {
    let mut identity: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
    let needs_certificate = identity.renew_at <= time::OffsetDateTime::now_utc().unix_timestamp()
        || urls
            .iter()
            .any(|url| !crate::network::certificate_covers(&identity.der, url));
    if needs_certificate {
        let pem = std::fs::read_to_string(dir.join("server-key.pem"))?;
        let key = rcgen::KeyPair::from_pem(&pem)?;
        ensure!(
            identity.key_pin == digest(key.public_key_raw()),
            "TLS key changed during renewal"
        );
        identity = Identity::issue(&key, urls.to_vec())?;
        config
            .reload_from_pem(identity.pem.as_bytes().to_vec(), pem.into_bytes())
            .await?;
        identity.save(dir)?;
        eprintln!("Renewed TLS certificate with the existing server key.");
    } else if identity.urls != urls {
        identity.urls = urls.to_vec();
        identity.save(dir)?;
    }
    Ok(())
}

pub async fn maintain(
    dir: std::path::PathBuf,
    listen: SocketAddr,
    explicit: Option<String>,
    config: RustlsConfig,
) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_mins(1)).await;
        match crate::network::origins(listen, explicit.as_deref()) {
            Ok(urls) => refresh(&dir, &urls, &config).await?,
            Err(error) => log::warn!("Network discovery unavailable: {error}"),
        }
    }
}

pub fn health(dir: &Path) -> Result<serde_json::Value> {
    let identity: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
    Ok(
        serde_json::json!({"name":"xlatch", "version":1, "urls":identity.urls, "key_pin":identity.key_pin}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn existing_certificate_is_kept_for_key_pin_upgrade() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("xlatch-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let legacy = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
        std::fs::write(
            dir.join("server-key.pem"),
            legacy.signing_key.serialize_pem(),
        )?;
        std::fs::write(dir.join("server.pem"), legacy.cert.pem())?;
        std::fs::write(dir.join("server.der"), legacy.cert.der())?;
        let urls = vec!["https://127.0.0.1:7443".to_owned()];
        let (_, pin) = prepare(&dir, &urls).await?;
        ensure!(
            pin == digest(legacy.cert.der()),
            "upgrade changed the QR-pinned certificate"
        );
        ensure!(
            health(&dir)?["key_pin"] == digest(legacy.signing_key.public_key_raw()),
            "upgrade advertised a different key"
        );
        let (_, restarted_pin) = prepare(&dir, &urls).await?;
        ensure!(pin == restarted_pin, "restart changed the certificate");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn renewal_preserves_key_and_updates_certificate_and_routes() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("xlatch-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let before = vec!["https://127.0.0.1:7443".to_owned()];
        let (config, pin) = prepare(&dir, &before).await?;
        let initial = health(&dir)?;
        let after = vec!["https://100.64.0.1:7443".to_owned()];
        refresh(&dir, &after, &config).await?;
        let renewed: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
        ensure!(
            renewed.pin != pin && renewed.urls == after,
            "certificate not renewed"
        );
        ensure!(
            initial["key_pin"] == renewed.key_pin,
            "renewal changed trusted key"
        );
        ensure!(
            crate::network::certificate_covers(&renewed.der, &after[0]),
            "new address not certified"
        );
        let mut due: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
        due.renew_at = 0;
        due.save(&dir)?;
        refresh(&dir, &after, &config).await?;
        let scheduled: Identity = serde_json::from_slice(&std::fs::read(dir.join(FILE))?)?;
        ensure!(
            scheduled.renew_at > 0 && scheduled.key_pin == renewed.key_pin,
            "scheduled renewal failed"
        );
        std::fs::write(
            dir.join("server-key.pem"),
            rcgen::KeyPair::generate()?.serialize_pem(),
        )?;
        ensure!(
            prepare(&dir, &after).await.is_err(),
            "replacement key accepted silently"
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
