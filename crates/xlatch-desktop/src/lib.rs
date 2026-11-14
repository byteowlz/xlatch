//! Desktop presentation data and transport. No graphical dependency in the default build.

pub mod palette;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use xlatch_core::{
    capability::{Capability, Job, Request},
    local::Control,
};

/// Explicit transport selection; paired clients never fall back to operator access.
#[derive(Debug, Clone)]
pub enum Connection {
    /// Same-user local operator socket.
    Local(std::path::PathBuf),
    /// Paired HTTPS client config and adjacent private device credentials.
    Paired(std::path::PathBuf),
}

impl Connection {
    /// Selected path, for connection settings.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Local(path) | Self::Paired(path) => path,
        }
    }

    /// Whether authority is limited to the paired device's grants.
    #[must_use]
    pub const fn is_paired(&self) -> bool {
        matches!(self, Self::Paired(_))
    }

    /// Execute through the selected transport without changing its authority.
    ///
    /// # Errors
    /// Rejects operator operations on paired connections and transport/server errors.
    pub fn call(&self, request: Control) -> Result<Value> {
        match self {
            Self::Local(directory) => call(directory, request),
            Self::Paired(config) => {
                let Control::Rpc { request } = request else {
                    anyhow::bail!("paired clients cannot use local operator operations");
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let client = xlatch_client::Client::load(config)?;
                runtime.block_on(client.call(&serde_json::to_value(request)?))
            }
        }
    }

    /// Refresh visible actions and owned activity from the same authority.
    ///
    /// # Errors
    /// Returns transport, grant and malformed response errors.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let capabilities = serde_json::from_value(self.call(Control::Rpc {
            request: Request::Discover,
        })?)?;
        let jobs = serde_json::from_value(self.call(Control::Rpc {
            request: Request::Jobs,
        })?)?;
        Ok(Snapshot { capabilities, jobs })
    }
}

/// A coherent refresh from the selected local daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Visible actions; paired transports return only active granted revisions.
    pub capabilities: Vec<Capability>,
    /// Recent jobs without result bodies.
    pub jobs: Vec<Job>,
}

/// Execute a bounded local API call from a background thread.
///
/// # Errors
/// Returns transport, timeout, or server authorization errors.
pub fn call(directory: &Path, request: Control) -> Result<Value> {
    #[cfg(unix)]
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(25),
                xlatch_core::local::call(directory, request),
            )
            .await
            .context("xlatch did not respond within 25 seconds")?
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, request);
        anyhow::bail!(
            "Local control transport is not implemented on this platform. Paired HTTPS desktop access is required."
        )
    }
}

/// Load the registry and activity without touching daemon storage directly.
///
/// # Errors
/// Returns connection, permission, or malformed response errors.
pub fn snapshot(directory: &Path) -> Result<Snapshot> {
    Connection::Local(directory.to_path_buf()).snapshot()
}

/// A reviewed invocation retaining its identity across uncertain replies.
#[derive(Debug, Clone)]
pub struct Draft {
    /// Exact action contract being invoked.
    pub capability: Capability,
    /// JSON payload visible in the composer.
    pub input: Value,
    /// Stable idempotency key; retries must reuse it.
    pub id: String,
}

impl Draft {
    /// Prepare an active revision, preserving an unchanged previous draft's key.
    ///
    /// # Errors
    /// Rejects pending actions and malformed JSON.
    pub fn prepare(capability: &Capability, input: &str, previous: Option<&Self>) -> Result<Self> {
        ensure!(
            capability.status == "active",
            "This revision still needs approval."
        );
        let input: Value = serde_json::from_str(input).context("Input must be valid JSON")?;
        if let Some(previous) = previous
            && previous.capability == *capability
            && previous.input == input
        {
            return Ok(previous.clone());
        }
        Ok(Self {
            capability: capability.clone(),
            input,
            id: uuid::Uuid::new_v4().to_string(),
        })
    }

    /// Build the transport-neutral invocation, never resolving to a newer revision.
    #[must_use]
    pub fn request(&self) -> Control {
        Control::Rpc {
            request: Request::Invoke {
                capability_id: self.capability.manifest.id.clone(),
                revision: self.capability.revision.clone(),
                input: self.input.clone(),
                idempotency_key: self.id.clone(),
            },
        }
    }
}

/// Find actions by every space-separated term in their title, ID or description.
#[must_use]
pub fn matches(capability: &Capability, query: &str) -> bool {
    let haystack = format!(
        "{} {} {}",
        capability.manifest.id, capability.manifest.title, capability.manifest.description
    )
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|term| haystack.contains(term))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capability() -> Result<Capability> {
        let manifest =
            serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
        Ok(Capability {
            manifest,
            revision: "a".repeat(64),
            status: "active".into(),
        })
    }
    #[test]
    fn uncertain_retry_keeps_key_but_changed_revision_does_not() -> Result<()> {
        let mut action = capability()?;
        let draft = Draft::prepare(&action, r#"{"text":"hello"}"#, None)?;
        let retry = Draft::prepare(&action, r#"{ "text": "hello" }"#, Some(&draft))?;
        ensure!(draft.id == retry.id, "retry changed identity");
        action.revision = "b".repeat(64);
        ensure!(
            draft.id != Draft::prepare(&action, r#"{"text":"hello"}"#, Some(&draft))?.id,
            "new revision reused identity"
        );
        action.status = "pending".into();
        ensure!(Draft::prepare(&action, "{}", None).is_err());
        Ok(())
    }
    #[test]
    fn paired_connections_never_fall_back_to_operator_authority() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let connection = Connection::Paired(directory.path().join("missing.toml"));
        ensure!(connection.is_paired());
        let error = connection
            .call(Control::Approve {
                id: "echo".into(),
                revision: "a".repeat(64),
                allow_host_execution: false,
            })
            .err()
            .context("paired connection accepted approval")?;
        ensure!(
            error
                .to_string()
                .contains("cannot use local operator operations")
        );
        ensure!(
            connection.snapshot().is_err(),
            "missing pairing fell back to local control"
        );
        Ok(())
    }

    #[test]
    fn malformed_input_never_becomes_an_invocation() -> Result<()> {
        ensure!(Draft::prepare(&capability()?, "not JSON", None).is_err());
        Ok(())
    }
}
