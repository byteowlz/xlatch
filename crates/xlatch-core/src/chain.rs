//! One-off chains preserve direct grants; they never create active registry entries.
use crate::{
    capability::{Capability, Execution, Job, Manifest},
    composition::{Binding, Step},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Exact client-selected revision; executable bindings always come from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Registered action identifier.
    pub capability_id: String,
    /// Granted revision digest.
    pub revision: String,
}
impl Store {
    fn chain_steps(&self, owner: &str, refs: &[Reference]) -> Result<Vec<Step>> {
        ensure!(!refs.is_empty() && refs.len() <= 16, "select 1–16 steps");
        let mut steps = Vec::new();
        for r in refs {
            let cap = self.capability(&r.capability_id)?;
            ensure!(
                cap.status == "active"
                    && cap.revision == r.revision
                    && self.has_grant(owner, &r.capability_id, &r.revision)?,
                "chain step changed or permission denied"
            );
            ensure!(
                !matches!(cap.manifest.execution, Execution::Compose { .. }),
                "select leaf actions for a chain"
            );
            if let Some(previous) = steps.last() {
                let previous: &Step = previous;
                ensure!(
                    crate::composition::compatible(
                        &previous.manifest.output_schema,
                        &cap.manifest.input_schema
                    ),
                    "incompatible chain steps"
                );
            }
            steps.push(Step {
                manifest: cap.manifest,
                revision: cap.revision,
                input: Binding::Previous,
            });
        }
        Ok(steps)
    }
    /// List authorized next steps using the same compatibility check as execution.
    /// # Errors
    /// Rejects stale or unauthorized selected steps.
    pub fn chain_candidates(&self, owner: &str, refs: &[Reference]) -> Result<Vec<Capability>> {
        let steps = self.chain_steps(owner, refs)?;
        let previous = steps.last().context("missing step")?;
        if steps.len() == 16 {
            return Ok(Vec::new());
        }
        Ok(self
            .discover(owner)?
            .into_iter()
            .filter(|c| {
                c.status == "active"
                    && !matches!(c.manifest.execution, Execution::Compose { .. })
                    && crate::composition::compatible(
                        &previous.manifest.output_schema,
                        &c.manifest.input_schema,
                    )
            })
            .collect())
    }
    fn chain_manifest(
        &self,
        owner: &str,
        refs: &[Reference],
        id: String,
        title: String,
    ) -> Result<Manifest> {
        let steps = self.chain_steps(owner, refs)?;
        ensure!(steps.len() >= 2, "select at least two steps");
        let first = steps.first().context("missing first step")?;
        let last = steps.last().context("missing last step")?;
        let manifest = Manifest {
            icon: None,
            id,
            title,
            description: "Run the selected action revisions in order".into(),
            accepts: first.manifest.accepts.clone(),
            input_schema: first.manifest.input_schema.clone(),
            output_schema: last.manifest.output_schema.clone(),
            timeout_seconds: steps.iter().map(|s| s.manifest.timeout_seconds).sum(),
            execution: Execution::Compose { steps },
        };
        manifest.validate()?;
        Ok(manifest)
    }
    /// Submit a fresh, pending target. Existing capabilities cannot be overwritten.
    /// # Errors
    /// Returns contract, permission or storage errors.
    pub fn save_chain(&self, owner: &str, refs: &[Reference], title: &str) -> Result<Capability> {
        ensure!(
            !title.trim().is_empty() && title.len() <= 120,
            "name must contain 1–120 bytes"
        );
        let manifest = self.chain_manifest(
            owner,
            refs,
            format!("chain.{}", uuid::Uuid::new_v4()),
            title.trim().into(),
        )?;
        self.register(&manifest)
    }
    /// Atomically enqueue without granting or activating any capability.
    /// # Errors
    /// Rejects incompatible steps, missing direct grants, invalid content or reused keys.
    pub fn invoke_chain(
        &self,
        owner: &str,
        refs: &[Reference],
        input: &Value,
        key: &str,
    ) -> Result<Job> {
        ensure!(
            !key.is_empty() && key.len() <= 120,
            "invalid idempotency key"
        );
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let manifest =
            self.chain_manifest(owner, refs, "one-off-chain".into(), "Shared chain".into())?;
        let revision = manifest.revision()?;
        ensure!(
            serde_json::to_vec(input)?.len() <= crate::capability::MAX_BYTES
                && jsonschema::validator_for(&manifest.input_schema)?.is_valid(input),
            "invalid chain input"
        );
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM jobs WHERE owner=?1 AND idempotency_key=?2",
                params![owner, key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            let job = self.job(owner, &id)?;
            ensure!(
                self.is_transient_chain(&id)? && job.revision == revision && job.input == *input,
                "idempotency key belongs to another invocation"
            );
            tx.commit()?;
            return Ok(job);
        }
        let queued: i64 = self.conn.query_row(
            "SELECT count(*) FROM jobs WHERE status IN ('queued','running')",
            [],
            |r| r.get(0),
        )?;
        ensure!(queued < 1000, "job queue is full");
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO jobs(id,capability_id,revision,manifest,owner,status,input,idempotency_key,created_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8)",params![id,manifest.id,revision,serde_json::to_string(&manifest)?,owner,serde_json::to_string(input)?,key,now()])?;
        self.conn
            .execute("INSERT INTO transient_compositions VALUES(?1)", [&id])?;
        self.event(&id, owner, "queued")?;
        let job = self.job(owner, &id)?;
        tx.commit()?;
        Ok(job)
    }
    pub(crate) fn is_transient_chain(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM transient_compositions WHERE parent_id=?1)",
            [id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn composition_manifest(&self, job: &Job) -> Result<Manifest> {
        let raw: String =
            self.conn
                .query_row("SELECT manifest FROM jobs WHERE id=?1", [&job.id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub(crate) fn authorize_transient_chain(&self, job: &Job) -> Result<()> {
        let manifest = self.composition_manifest(job)?;
        let Execution::Compose { steps } = manifest.execution else {
            anyhow::bail!("invalid chain job");
        };
        for step in steps {
            let cap = self.capability(&step.manifest.id)?;
            ensure!(
                cap.status == "active"
                    && cap.revision == step.revision
                    && self.has_grant(&job.owner, &cap.manifest.id, &cap.revision)?,
                "chain step permission changed"
            );
        }
        Ok(())
    }
}
