//! Desktop presentation data and transport. No GPUI dependency in the default build.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use xlatch_core::{
    capability::{Capability, Job, Request},
    local::Control,
};

/// A coherent refresh from the selected local daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Registered actions, including unapproved revisions.
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
    let capabilities = serde_json::from_value(call(
        directory,
        Control::Rpc {
            request: Request::Discover,
        },
    )?)?;
    let jobs = serde_json::from_value(call(
        directory,
        Control::Rpc {
            request: Request::Jobs,
        },
    )?)?;
    Ok(Snapshot { capabilities, jobs })
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
    fn malformed_input_never_becomes_an_invocation() -> Result<()> {
        ensure!(Draft::prepare(&capability()?, "not JSON", None).is_err());
        Ok(())
    }
}
