//! Explicit new executions created from retained input of completed jobs.

use crate::{
    capability::{Capability, Job},
    parking::mime_matches,
    store::Store,
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::Value;

impl Store {
    /// Return current granted targets that accept the retained input of an owned job.
    ///
    /// The input stays on the server. Current grants, revisions, schemas, and artifact
    /// retention are checked rather than replaying the historical authorization.
    ///
    /// # Errors
    /// Rejects foreign, active, or no-longer-retained source jobs.
    pub fn job_candidates(&self, owner: &str, id: &str) -> Result<Vec<Capability>> {
        let job = self.resend_source(owner, id)?;
        let source_owner = &job.owner;
        let mime = job
            .input
            .get("mime_type")
            .and_then(Value::as_str)
            .context("original job input has no MIME type")?;
        self.ensure_retained_input(source_owner, &job.input)?;
        self.input_candidates(source_owner, mime, &job.input)
    }

    /// Create a fresh job from the retained input of a completed owned job.
    ///
    /// # Errors
    /// Rejects foreign or active source jobs, unavailable input, and inactive,
    /// stale, incompatible, or ungranted targets.
    pub fn resend_job(
        &mut self,
        owner: &str,
        id: &str,
        capability_id: &str,
        revision: &str,
        idempotency_key: &str,
    ) -> Result<Job> {
        let job = self.resend_source(owner, id)?;
        let source_owner = job.owner.clone();
        self.ensure_retained_input(&source_owner, &job.input)?;
        self.invoke(
            &source_owner,
            capability_id,
            revision,
            &job.input,
            idempotency_key,
        )
    }

    pub(crate) fn input_candidates(
        &self,
        owner: &str,
        mime: &str,
        input: &Value,
    ) -> Result<Vec<Capability>> {
        Ok(self
            .discover(owner)?
            .into_iter()
            .filter(|capability| {
                capability
                    .manifest
                    .accepts
                    .iter()
                    .any(|accepted| mime_matches(accepted, mime))
            })
            .filter_map(|capability| {
                self.validate_file_input(owner, &capability.manifest, input)
                    .ok()
                    .map(|_| capability)
            })
            .collect())
    }

    fn resend_source(&self, owner: &str, id: &str) -> Result<Job> {
        let job = self.job(owner, id)?;
        ensure!(
            matches!(job.status.as_str(), "succeeded" | "failed" | "cancelled"),
            "wait for the original job to finish before sending it again"
        );
        Ok(job)
    }

    fn ensure_retained_input(&self, owner: &str, input: &Value) -> Result<()> {
        let Some(upload_id) = input.pointer("/file/artifact_id").and_then(Value::as_str) else {
            return Ok(());
        };
        let retained: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM uploads WHERE id=?1 AND owner=?2 AND received=size AND (expires>?3 OR EXISTS(SELECT 1 FROM parked_items WHERE upload_id=uploads.id)))",
            params![upload_id, owner, crate::store::now()],
            |row| row.get(0),
        )?;
        ensure!(
            retained,
            "the original file is no longer retained; share it again"
        );
        Ok(())
    }
}
