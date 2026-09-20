//! Broker-owned scheduling: a parent grant delegates only its reviewed steps.
use crate::{
    capability::{Execution, Job, Manifest},
    composition::Step,
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

pub fn dependencies(conn: &Connection, manifest: &Manifest) -> Result<()> {
    if let Execution::Compose { steps } = &manifest.execution {
        for step in steps {
            let (body, revision, status): (String, String, String) = conn
                .query_row(
                    "SELECT manifest,revision,status FROM capabilities WHERE id=?1",
                    [&step.manifest.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .with_context(|| format!("missing composition step {}", step.manifest.id))?;
            ensure!(
                status == "active"
                    && revision == step.revision
                    && serde_json::from_str::<Manifest>(&body)? == step.manifest,
                "composition step {} changed or is not active; review a new composition revision",
                step.manifest.id
            );
        }
    }
    Ok(())
}
impl Store {
    /// Check direct permission or delegation from a currently authorized composition parent.
    pub(crate) fn authorize_job(&self, job: &Job) -> Result<()> {
        if self.is_transient_chain(&job.id)? {
            return self.authorize_transient_chain(job);
        }
        let current = self.capability(&job.capability_id)?;
        ensure!(
            current.status == "active" && current.revision == job.revision,
            "action revision changed before execution"
        );
        let parent: Option<String> = self
            .conn
            .query_row(
                "SELECT parent_id FROM composition_steps WHERE child_id=?1",
                [&job.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = parent {
            let parent = self.job("local", &id)?;
            self.authorize_job(&parent)?;
            ensure!(
                parent.owner == job.owner && parent.status == "running",
                "composition approval, grant or state changed"
            );
            dependencies(&self.conn, &self.composition_manifest(&parent)?)?;
        } else {
            ensure!(
                self.has_grant(&job.owner, &job.capability_id, &job.revision)?,
                "capability permission changed"
            );
        }
        Ok(())
    }
    pub(crate) fn advance_compositions(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT id FROM jobs WHERE status IN ('queued','running') AND json_extract(manifest,'$.execution.kind')='compose' AND NOT EXISTS (SELECT 1 FROM composition_steps s JOIN jobs child ON child.id=s.child_id WHERE s.parent_id=jobs.id AND child.status IN ('queued','running')) ORDER BY created_at,rowid LIMIT 64")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            let parent = self.job("local", &id)?;
            if matches!(parent.status.as_str(), "queued" | "running")
                && let Err(error) = self.advance_composition(&parent)
            {
                self.end_composition(
                    &parent,
                    "failed",
                    None,
                    Some(&format!("{error:#}").chars().take(4000).collect::<String>()),
                )?;
                self.cancel_composition_children(&parent.id)?;
            }
            tx.commit()?;
        }
        Ok(())
    }
    fn advance_composition(&self, parent: &Job) -> Result<()> {
        self.authorize_job(parent)?;
        let manifest = self.composition_manifest(parent)?;
        dependencies(&self.conn, &manifest)?;
        let Execution::Compose { steps } = &manifest.execution else {
            anyhow::bail!("expected composition");
        };
        let last: Option<(u32,String)> = self.conn.query_row("SELECT position,child_id FROM composition_steps WHERE parent_id=?1 ORDER BY position DESC LIMIT 1",[&parent.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((position, id)) = last else {
            self.conn
                .execute("UPDATE jobs SET status='running' WHERE id=?1", [&parent.id])?;
            return self.enqueue_step(parent, 0, &steps[0], &parent.input);
        };
        let child = self.job("local", &id)?;
        match child.status.as_str() {
            "queued" | "running" => Ok(()),
            "succeeded" => {
                let result = child
                    .result
                    .as_ref()
                    .context("successful step has no result")?;
                if let Some(step) = steps.get(position as usize + 1) {
                    self.enqueue_step(parent, position + 1, step, result)
                } else {
                    ensure!(
                        jsonschema::validator_for(&manifest.output_schema)?.is_valid(result),
                        "composition result violates output schema"
                    );
                    self.end_composition(parent, "succeeded", Some(result), None)
                }
            }
            "failed" | "cancelled" => self.end_composition(
                parent,
                &child.status,
                None,
                Some(&format!(
                    "Step {} ({}): {}",
                    position + 1,
                    child.capability_id,
                    child.error.as_deref().unwrap_or(&child.status)
                )),
            ),
            _ => anyhow::bail!("invalid step state"),
        }
    }
    fn enqueue_step(
        &self,
        parent: &Job,
        position: u32,
        step: &Step,
        previous: &Value,
    ) -> Result<()> {
        let input = step.input.apply(&parent.input, previous)?;
        ensure!(
            serde_json::to_vec(&input)?.len() <= crate::capability::MAX_BYTES,
            "mapped input exceeds request limit"
        );
        ensure!(
            jsonschema::validator_for(&step.manifest.input_schema)?.is_valid(&input),
            "mapped input for {} does not match its schema",
            step.manifest.id
        );
        let id = uuid::Uuid::new_v4().to_string();
        // Distinct reserved owner-scoped key: internal creation is never a caller invocation.
        let key = format!("composition:{}:{position}", parent.id);
        self.conn.execute("INSERT INTO jobs(id,capability_id,revision,manifest,owner,status,input,idempotency_key,created_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8)",params![id,step.manifest.id,step.revision,serde_json::to_string(&step.manifest)?,parent.owner,serde_json::to_string(&input)?,key,now()])?;
        self.conn.execute(
            "INSERT INTO composition_steps VALUES(?1,?2,?3)",
            params![parent.id, position, id],
        )?;
        self.event(&id, &parent.owner, "queued")?;
        self.event(&parent.id, &parent.owner, "running")?;
        Ok(())
    }
    fn end_composition(
        &self,
        parent: &Job,
        status: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute("UPDATE jobs SET status=?2,result=?3,error=?4 WHERE id=?1 AND status IN ('queued','running')",params![parent.id,status,result.map(serde_json::to_string).transpose()?,error])?;
        self.event(&parent.id, &parent.owner, status)
    }
    pub(crate) fn cancel_composition_children(&self, id: &str) -> Result<()> {
        self.conn.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'cancelled' FROM jobs WHERE id IN (SELECT child_id FROM composition_steps WHERE parent_id=?1) AND status IN ('queued','running')",[id])?;
        self.conn.execute("UPDATE jobs SET status='cancelled' WHERE id IN (SELECT child_id FROM composition_steps WHERE parent_id=?1) AND status IN ('queued','running')",[id])?;
        Ok(())
    }
    /// Step receipts visible only to the parent owner or trusted operator; results stay in child jobs.
    /// # Errors
    /// Returns authorization or storage errors.
    pub fn composition_steps(&self, owner: &str, id: &str) -> Result<Vec<Value>> {
        self.job(owner, id)?;
        let mut stmt = self.conn.prepare("SELECT s.position,j.id,j.capability_id,j.revision,j.status,j.error FROM composition_steps s JOIN jobs j ON j.id=s.child_id WHERE s.parent_id=?1 ORDER BY s.position")?;
        Ok(stmt.query_map([id],|r|Ok(json!({"position":r.get::<_,u32>(0)?,"job_id":r.get::<_,String>(1)?,"capability_id":r.get::<_,String>(2)?,"revision":r.get::<_,String>(3)?,"status":r.get::<_,String>(4)?,"error":r.get::<_,Option<String>>(5)?})))?.collect::<Result<Vec<_>,_>>()?)
    }
}
