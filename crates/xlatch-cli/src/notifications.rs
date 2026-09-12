//! Optional native ntfy or Apprise delivery, independent of capability execution.

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use xlatch_core::{capability::digest, paths::default_config_dir, store::Store};

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Provider {
    #[default]
    Apprise,
    Ntfy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    provider: Provider,
    /// Full Apprise API /notify/KEY URL or ntfy topic URL.
    endpoint: String,
    #[serde(default)]
    tag: String,
    /// Optional environment variable holding the API's bearer credential.
    token_env: Option<String>,
}

pub struct Delivery {
    config: Config,
    client: reqwest::Client,
    token: Option<String>,
    destination: String,
    state: Connection,
}

fn validate_endpoint(value: &str, provider: Provider) -> Result<()> {
    let url = url::Url::parse(value).context("invalid notification endpoint")?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && local),
        "Notifications require HTTPS except on loopback"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.query().is_none(),
        "Notification endpoint must not contain userinfo, query or fragment"
    );
    match provider {
        Provider::Apprise => ensure!(
            url.path().starts_with("/notify/") && url.path().len() > 8,
            "use the Apprise API /notify/KEY endpoint"
        ),
        Provider::Ntfy => {
            let topic = url.path().trim_start_matches('/');
            ensure!(
                !topic.is_empty()
                    && topic
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
                "ntfy endpoint must contain a single topic using letters, digits, underscores or hyphens"
            );
        }
    }
    Ok(())
}

pub fn prepare(dir: &Path) -> Result<Option<Delivery>> {
    let path = default_config_dir()?.join("notifications.toml");
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading notifications.toml"),
    };
    let config: Config = toml::from_str(&body).map_err(|_| {
        anyhow::anyhow!(
            "invalid notifications.toml; expected provider, endpoint, tag and optional token_env"
        )
    })?;
    validate_endpoint(&config.endpoint, config.provider)?;
    ensure!(
        matches!(config.provider, Provider::Apprise) || config.tag.is_empty(),
        "tag is only supported for Apprise"
    );
    let token = config
        .token_env
        .as_ref()
        .map(|name| {
            std::env::var(name).context("Notification token environment variable is missing")
        })
        .transpose()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?;
    let identity = match config.provider {
        Provider::Apprise => format!("{}\n{}", config.endpoint, config.tag),
        Provider::Ntfy => format!("ntfy\n{}", config.endpoint),
    };
    let destination = digest(identity.as_bytes());
    let state = Connection::open(dir.join("notifications.sqlite3"))?;
    state.busy_timeout(Duration::from_secs(5))?;
    state.execute_batch("CREATE TABLE IF NOT EXISTS cursors(destination TEXT PRIMARY KEY, sequence INTEGER NOT NULL);")?;
    // Enabling a destination starts with future events, never historical job content.
    let jobs = Connection::open_with_flags(
        dir.join("xlatch.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let latest: i64 =
        jobs.query_row("SELECT coalesce(max(sequence),0) FROM events", [], |row| {
            row.get(0)
        })?;
    state.execute(
        "INSERT OR IGNORE INTO cursors VALUES(?1,?2)",
        params![destination, latest],
    )?;
    Ok(Some(Delivery {
        config,
        client,
        token,
        destination,
        state,
    }))
}

fn payload(status: &str, tag: &str) -> Option<Value> {
    let (body, kind) = match status {
        "succeeded" => (
            "A job completed. Open xlatch to view the result.",
            "success",
        ),
        "failed" => ("A job failed. Open xlatch for details.", "failure"),
        "cancelled" => ("A job was cancelled.", "info"),
        _ => return None,
    };
    Some(json!({"title":"xlatch", "body":body, "type":kind, "format":"text", "tag":tag}))
}

impl Delivery {
    async fn step(&mut self, dir: &Path) -> Result<()> {
        let cursor: i64 = self.state.query_row(
            "SELECT sequence FROM cursors WHERE destination=?1",
            [&self.destination],
            |row| row.get(0),
        )?;
        let events = Store::open(dir)?.events("local", cursor)?;
        for event in events {
            if let Some(body) = payload(&event.status, &self.config.tag) {
                let mut request = match self.config.provider {
                    Provider::Apprise => self.client.post(&self.config.endpoint).json(&body),
                    Provider::Ntfy => self
                        .client
                        .post(&self.config.endpoint)
                        .header("Title", "xlatch")
                        .header("Content-Type", "text/plain; charset=utf-8")
                        .body(
                            body["body"]
                                .as_str()
                                .context("missing notification text")?
                                .to_owned(),
                        ),
                };
                if let Some(token) = &self.token {
                    request = request.bearer_auth(token);
                }
                // Never surface URLs, response bodies or credentials in logs.
                let response = request
                    .send()
                    .await
                    .map_err(|_| anyhow::anyhow!("Notification connection failed"))?;
                ensure!(
                    response.status().is_success(),
                    "Notification provider rejected notification (HTTP {})",
                    response.status().as_u16()
                );
            }
            let tx = self.state.transaction()?;
            tx.execute(
                "UPDATE cursors SET sequence=?2 WHERE destination=?1",
                params![self.destination, event.sequence],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub async fn run(mut self, dir: PathBuf) -> Result<()> {
        let mut delay = 2;
        loop {
            if let Err(error) = self.step(&dir).await {
                // Retry only notifications, never the underlying job. An uncertain HTTP
                // outcome can produce a duplicate: delivery providers have no shared idempotency contract.
                log::warn!("Notification delivery paused: {error}; retrying in {delay}s");
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(300);
            } else {
                delay = 2;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_policy() {
        let cases = [
            (
                Provider::Apprise,
                "http://127.0.0.1:8000/notify/xlatch",
                true,
            ),
            (
                Provider::Apprise,
                "https://push.example.com/notify/xlatch",
                true,
            ),
            (Provider::Apprise, "https://example.com/notify/", false),
            (Provider::Apprise, "http://example.com/notify/xlatch", false),
            (
                Provider::Apprise,
                "https://user:secret@example.com/notify/xlatch",
                false,
            ),
            (
                Provider::Apprise,
                "https://example.com/notify/xlatch?token=secret",
                false,
            ),
            (Provider::Ntfy, "https://ntfy.sh/xlatch-alerts", true),
            (Provider::Ntfy, "http://127.0.0.1:8000/topic", true),
            (Provider::Ntfy, "https://ntfy.sh/", false),
            (Provider::Ntfy, "https://ntfy.sh/topic/json", false),
            (Provider::Ntfy, "https://ntfy.sh/topic?auth=secret", false),
            (Provider::Ntfy, "http://ntfy.sh/topic", false),
        ];
        for (provider, endpoint, expected) in cases {
            assert_eq!(validate_endpoint(endpoint, provider).is_ok(), expected);
        }
    }

    #[test]
    fn only_terminal_events_have_generic_payloads() {
        assert_eq!(payload("running", "phone"), None);
        assert_eq!(
            payload("succeeded", "phone"),
            Some(
                json!({"title":"xlatch","body":"A job completed. Open xlatch to view the result.","type":"success","format":"text","tag":"phone"})
            )
        );
    }
}
