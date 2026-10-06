//! Broker-owned scheduling: a parent grant delegates only its reviewed steps.
use crate::{
    capability::{Execution, Job, Manifest},
    composition::{Step, Target},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

pub fn dependencies(conn: &Connection, manifest: &Manifest) -> Result<()> {
    let dependencies: Vec<(&Manifest, &str)> = match &manifest.execution {
        Execution::Compose { steps } => steps
            .iter()
            .map(|step| (&step.manifest, step.revision.as_str()))
            .collect(),
        Execution::FanOut { targets } => targets
            .iter()
            .map(|target| (&target.manifest, target.revision.as_str()))
            .collect(),
        _ => Vec::new(),
    };
    for (manifest, expected_revision) in dependencies {
        let (body, revision, status): (String, String, String) = conn
            .query_row(
                "SELECT manifest,revision,status FROM capabilities WHERE id=?1",
                [&manifest.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .with_context(|| format!("missing composition target {}", manifest.id))?;
        ensure!(
            status == "active"
                && revision == expected_revision
                && serde_json::from_str::<Manifest>(&body)? == *manifest,
            "composition target {} changed or is not active; review a new composition revision",
            manifest.id
        );
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
        let mut stmt = self.conn.prepare("SELECT id FROM jobs WHERE status IN ('queued','running') AND json_extract(manifest,'$.execution.kind') IN ('compose','fan_out') AND NOT EXISTS (SELECT 1 FROM composition_steps s JOIN jobs child ON child.id=s.child_id WHERE s.parent_id=jobs.id AND child.status IN ('queued','running')) ORDER BY created_at,rowid LIMIT 64")?;
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
        if let Execution::FanOut { targets } = &manifest.execution {
            return self.advance_fan_out(parent, &manifest, targets);
        }
        let Execution::Compose { steps } = &manifest.execution else {
            anyhow::bail!("expected composition")
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
    fn advance_fan_out(&self, parent: &Job, manifest: &Manifest, targets: &[Target]) -> Result<()> {
        let count: u32 = self.conn.query_row(
            "SELECT count(*) FROM composition_steps WHERE parent_id=?1",
            [&parent.id],
            |row| row.get(0),
        )?;
        if count == 0 {
            self.conn
                .execute("UPDATE jobs SET status='running' WHERE id=?1", [&parent.id])?;
            for (position, target) in targets.iter().enumerate() {
                self.enqueue_target(
                    parent,
                    position as u32,
                    &target.manifest,
                    &target.revision,
                    &parent.input,
                )?;
            }
            return Ok(());
        }
        ensure!(
            count as usize == targets.len(),
            "fan-out receipts are incomplete"
        );
        let mut statement = self.conn.prepare("SELECT j.capability_id,j.revision,j.status,j.result,j.error FROM composition_steps s JOIN jobs j ON j.id=s.child_id WHERE s.parent_id=?1 ORDER BY s.position")?;
        let rows = statement
            .query_map([&parent.id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some((index, row)) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| matches!(row.2.as_str(), "failed" | "cancelled"))
        {
            return self.end_composition(
                parent,
                &row.2,
                None,
                Some(&format!(
                    "Target {} ({}): {}",
                    index + 1,
                    row.0,
                    row.4.as_deref().unwrap_or(&row.2)
                )),
            );
        }
        ensure!(
            rows.iter().all(|row| row.2 == "succeeded"),
            "fan-out target is not terminal"
        );
        let results = rows
            .into_iter()
            .map(|(capability_id, revision, _, result, _)| {
                Ok(json!({
                    "capability_id": capability_id,
                    "revision": revision,
                    "result": serde_json::from_str::<Value>(&result.context("successful target has no result")?)?
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let result = json!({"results": results});
        ensure!(
            jsonschema::validator_for(&manifest.output_schema)?.is_valid(&result),
            "fan-out result violates output schema"
        );
        self.end_composition(parent, "succeeded", Some(&result), None)
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
        self.enqueue_target(parent, position, &step.manifest, &step.revision, &input)
    }
    fn enqueue_target(
        &self,
        parent: &Job,
        position: u32,
        manifest: &Manifest,
        revision: &str,
        input: &Value,
    ) -> Result<()> {
        let upload = self.validate_file_input(&parent.owner, manifest, input)?;
        let id = uuid::Uuid::new_v4().to_string();
        // Distinct reserved owner-scoped key: internal creation is never a caller invocation.
        let key = format!("composition:{}:{position}", parent.id);
        self.conn.execute("INSERT INTO jobs(id,capability_id,revision,manifest,owner,status,input,idempotency_key,created_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8)",params![id,manifest.id,revision,serde_json::to_string(manifest)?,parent.owner,serde_json::to_string(input)?,key,now()])?;
        self.conn.execute(
            "INSERT INTO composition_steps VALUES(?1,?2,?3)",
            params![parent.id, position, id],
        )?;
        self.bind_upload(&id, upload.as_deref())?;
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
