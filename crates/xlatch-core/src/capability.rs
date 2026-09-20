//! Transport-neutral contracts shared by humans, agents, and adapters.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Maximum encoded request or result size (8 `MiB`).
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Immutable, reviewed capability contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Stable publisher-selected identifier.
    pub id: String,
    /// Human-readable action name.
    pub title: String,
    /// What this action does and its side effects.
    pub description: String,
    /// MIME types the share client can submit.
    pub accepts: Vec<String>,
    /// JSON Schema for invocation input.
    pub input_schema: Value,
    /// JSON Schema for successful results.
    pub output_schema: Value,
    /// Explicit implementation, never a shell string.
    pub execution: Execution,
    /// Upper bound on execution time.
    pub timeout_seconds: u64,
}

/// Execution binding. Commands are explicitly trusted host programs, not sandboxed code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Execution {
    /// Run reviewed independent capabilities in order.
    Compose {
        /// Exact leaf contracts and their input wiring.
        steps: Vec<crate::composition::Step>,
    },
    /// Return the submitted JSON without side effects.
    Echo,
    /// Save shared content within a fixed, operator-approved directory.
    SaveFile {
        /// Canonical absolute directory; never supplied by the mobile request.
        directory: String,
    },
    /// Run an approved absolute program with fixed arguments and JSON on stdin.
    Command {
        /// Absolute executable path.
        program: String,
        /// Fixed argument vector. User input is supplied exclusively over stdin.
        args: Vec<String>,
        /// SHA-256 of the executable, checked at registration and before each run.
        sha256: String,
    },
}

impl Manifest {
    /// Validate a bounded, self-contained manifest.
    ///
    /// # Errors
    /// Returns an error for invalid identifiers, schemas, or execution bindings.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 80
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "invalid capability id"
        );
        ensure!(
            !self.title.trim().is_empty() && self.title.len() <= 120,
            "title must contain 1–120 bytes"
        );
        ensure!(self.description.len() <= 4000, "description too long");
        ensure!(
            !self.accepts.is_empty() && self.accepts.len() <= 16,
            "declare 1–16 MIME types"
        );
        ensure!(
            self.accepts
                .iter()
                .all(|mime| mime.len() <= 120 && mime.contains('/')),
            "invalid MIME type"
        );
        ensure!(
            (1..=3600).contains(&self.timeout_seconds),
            "timeout must be 1–3600 seconds"
        );
        if let Execution::Compose { steps } = &self.execution {
            crate::composition::validate(self, steps)?;
        }
        validate_schema(&self.input_schema)?;
        validate_schema(&self.output_schema)?;
        if let Execution::SaveFile { directory } = &self.execution {
            ensure!(
                std::path::Path::new(directory).is_absolute(),
                "save destination must be absolute"
            );
            ensure!(
                !std::path::Path::new(directory)
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
                "save destination must not traverse parents"
            );
        }
        if let Execution::Command {
            program,
            args,
            sha256,
        } = &self.execution
        {
            ensure!(
                std::path::Path::new(program).is_absolute(),
                "program must be absolute"
            );
            ensure!(
                args.len() <= 64 && args.iter().all(|a| a.len() <= 4096),
                "too many/large arguments"
            );
            ensure!(
                sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "expected executable SHA-256"
            );
        }
        Ok(())
    }

    /// Inspect host files only as a trusted operator or unprivileged executor.
    /// Registration in the protected broker must never call this method.
    ///
    /// # Errors
    /// Rejects non-canonical destinations, special files, and changed executables.
    pub fn validate_host_binding(&self) -> Result<()> {
        match &self.execution {
            Execution::SaveFile { directory } => ensure!(
                std::fs::canonicalize(directory)? == std::path::Path::new(directory),
                "save destination must be canonical"
            ),
            Execution::Command {
                program, sha256, ..
            } => {
                ensure!(
                    std::fs::metadata(program)?.is_file(),
                    "program must be a regular file"
                );
                ensure!(
                    digest(&std::fs::read(program)?) == *sha256,
                    "executable hash mismatch"
                );
            }
            Execution::Compose { steps } => {
                for step in steps {
                    step.manifest.validate_host_binding()?;
                }
            }
            Execution::Echo => {}
        }
        Ok(())
    }

    /// Whether approval delegates host command execution, directly or through composition.
    #[must_use]
    pub fn executes_commands(&self) -> bool {
        match &self.execution {
            Execution::Command { .. } => true,
            Execution::Compose { steps } => steps.iter().any(|s| s.manifest.executes_commands()),
            _ => false,
        }
    }

    /// Content digest to which approval is bound.
    ///
    /// # Errors
    /// Returns serialization errors.
    pub fn revision(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
}

fn validate_schema(schema: &Value) -> Result<()> {
    // Reject references entirely in v0: validation never retrieves remote schemas.
    fn has_reference(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                map.keys()
                    .any(|k| matches!(k.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef"))
                    || map.values().any(has_reference)
            }
            Value::Array(items) => items.iter().any(has_reference),
            _ => false,
        }
    }
    ensure!(
        !has_reference(schema),
        "v0 requires inline schemas without references"
    );
    jsonschema::validator_for(schema).map_err(|e| anyhow::anyhow!("invalid schema: {e}"))?;
    Ok(())
}

/// Lowercase SHA-256 used for revision binding and content integrity.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A registered manifest and its approval status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capability {
    /// Registered contract.
    pub manifest: Manifest,
    /// Content digest.
    pub revision: String,
    /// `pending` or `active`.
    pub status: String,
}

/// Durable execution record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Job {
    /// Stable job identifier.
    pub id: String,
    /// Capability identifier.
    pub capability_id: String,
    /// Approved revision executed by this job.
    pub revision: String,
    /// Requesting device or local operator.
    pub owner: String,
    /// `queued`, `running`, `succeeded`, `failed`, or `cancelled`.
    pub status: String,
    /// Validated input.
    pub input: Value,
    /// Structured output, including inline file artifacts when applicable.
    pub result: Option<Value>,
    /// Safe diagnostic for failed jobs.
    pub error: Option<String>,
    /// Unix creation time.
    pub created_at: i64,
}

/// State transition suitable for reconnect-safe polling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic cursor, retained across daemon restarts.
    pub sequence: i64,
    /// Changed job.
    pub job_id: String,
    /// New job status.
    pub status: String,
}

/// Core client operations. MCP and Oqto can adapt these directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Phone-signed capability activation and additive device grants.
    Approval {
        /// Approver operation.
        request: crate::approval::ApprovalRequest,
    },
    /// Phone-approved device enrollment and approval policy.
    Enrollment {
        /// Authenticated enrollment operation.
        request: crate::enrollment::EnrollmentRequest,
    },
    /// List visible active capabilities (all revisions for the local operator).
    Discover,
    /// Return authorized leaf actions compatible with the selected chain.
    ChainCandidates {
        /// Ordered exact revisions already selected.
        steps: Vec<crate::chain::Reference>,
    },
    /// Run a one-off chain using direct grants for every leaf.
    InvokeChain {
        /// Ordered exact leaf revisions.
        steps: Vec<crate::chain::Reference>,
        /// Original share content.
        input: Value,
        /// Stable retry identity.
        idempotency_key: String,
    },
    /// Submit a reusable composition for normal approval; never activates it.
    SaveChain {
        /// Ordered exact leaf revisions.
        steps: Vec<crate::chain::Reference>,
        /// Display name for the proposed target.
        title: String,
    },
    /// Enqueue a typed invocation; repeated keys return the same job.
    Invoke {
        /// Capability id.
        capability_id: String,
        /// Revision seen by the caller, preventing silent contract changes.
        revision: String,
        /// Schema-conforming input.
        input: Value,
        /// Caller-generated deduplication key.
        idempotency_key: String,
    },
    /// List the caller's recent jobs.
    Jobs,
    /// Fetch an owned job and result.
    Job {
        /// Job id.
        id: String,
    },
    /// Cancel a queued or running owned job.
    Cancel {
        /// Job id.
        id: String,
    },
    /// Read owned job events after a cursor.
    Events {
        /// Last received sequence.
        after: i64,
    },
}
