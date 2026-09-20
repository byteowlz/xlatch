//! Opt-in routing dataset, separate from operational jobs and their retry identities.
use crate::{
    capability::Capability,
    store::{Store, now},
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Content captured in newly accepted invocations only.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capture {
    /// Do not create dataset rows.
    #[default]
    Off,
    /// Capture routing labels and sizes without shared content.
    Metadata,
    /// Capture input JSON, excluding binary attachments. May contain sensitive text.
    Content,
}

/// Operator-owned policy; not writable through device or agent RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryPolicy {
    /// Explicit opt-in capture mode.
    pub capture: Capture,
    /// Maximum dataset age in days.
    pub retention_days: u32,
    /// Maximum retained records.
    pub max_rows: u32,
    /// Maximum serialized dataset bytes.
    pub max_bytes: u64,
    /// Exact capability IDs that must not be captured.
    pub excluded_capabilities: Vec<String>,
}
impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            capture: Capture::Off,
            retention_days: 30,
            max_rows: 10_000,
            max_bytes: 64 * 1024 * 1024,
            excluded_capabilities: Vec::new(),
        }
    }
}
impl Store {
    /// Read the current policy, defaulting to no capture.
    /// # Errors
    /// Returns storage or malformed policy errors.
    pub fn history_policy(&self) -> Result<HistoryPolicy> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT config FROM history_policy WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        raw.map_or_else(
            || Ok(HistoryPolicy::default()),
            |s| Ok(serde_json::from_str(&s)?),
        )
    }
    /// Replace capture policy using direct operator access to the private database.
    /// Existing records are never backfilled. Setting off does not erase old records.
    /// # Errors
    /// Rejects unbounded policies and returns storage errors.
    pub fn set_history_policy(&self, policy: &HistoryPolicy) -> Result<()> {
        ensure!(
            (1..=3650).contains(&policy.retention_days),
            "retention_days must be 1–3650"
        );
        ensure!(
            (1..=1_000_000).contains(&policy.max_rows),
            "max_rows must be 1–1000000"
        );
        ensure!(
            (1024..=1_073_741_824).contains(&policy.max_bytes),
            "max_bytes must be 1024–1073741824"
        );
        ensure!(
            policy.excluded_capabilities.len() <= 1000,
            "too many excluded capabilities"
        );
        self.conn.execute("INSERT INTO history_policy VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET config=excluded.config", [serde_json::to_string(policy)?])?;
        self.prune_history(policy)
    }
    pub(crate) fn capture_history(
        &self,
        job_id: &str,
        owner: &str,
        cap: &Capability,
        input: &Value,
    ) -> Result<()> {
        let policy = self.history_policy()?;
        if policy.capture == Capture::Off || policy.excluded_capabilities.contains(&cap.manifest.id)
        {
            return Ok(());
        }
        let mut record = json!({"schema_version":1,"job_id":job_id,"created_at":now(),"server_id":self.server_identity()?,"owner":owner,"capability_id":cap.manifest.id,"revision":cap.revision,"target_label":cap.manifest.title,"choice_provenance":"unknown","input_bytes":serde_json::to_vec(input)?.len()});
        if policy.capture == Capture::Content {
            let mut content = input.clone();
            omit_binary(&mut content);
            record["input"] = content;
        }
        let body = serde_json::to_string(&record)?;
        if body.len() as u64 <= policy.max_bytes {
            self.conn.execute(
                "INSERT INTO routing_history VALUES(?1,?2,?3,?4)",
                params![job_id, owner, now(), body],
            )?;
        }
        self.prune_history(&policy)
    }
    /// Enforce time, row and logical byte limits. Does not erase operational jobs.
    /// # Errors
    /// Returns storage errors.
    pub fn prune_history(&self, policy: &HistoryPolicy) -> Result<()> {
        self.conn.execute(
            "DELETE FROM routing_history WHERE created_at<?1",
            [now() - i64::from(policy.retention_days) * 86400],
        )?;
        self.conn.execute("DELETE FROM routing_history WHERE job_id IN (SELECT job_id FROM routing_history ORDER BY created_at DESC,job_id DESC LIMIT -1 OFFSET ?1)", [policy.max_rows])?;
        self.conn.execute("DELETE FROM routing_history WHERE job_id IN (SELECT job_id FROM (SELECT job_id,SUM(length(CAST(record AS BLOB))) OVER (ORDER BY created_at DESC,job_id DESC) AS bytes FROM routing_history) WHERE bytes>?1)", [i64::try_from(policy.max_bytes)?])?;
        Ok(())
    }
    /// Export reproducible JSON records, optionally restricted to one owner.
    /// Outcome is operational status, not a correctness label.
    /// # Errors
    /// Returns storage or malformed record errors.
    pub fn history(&self, owner: Option<&str>) -> Result<Vec<Value>> {
        self.prune_history(&self.history_policy()?)?;
        let mut stmt = self.conn.prepare("SELECT h.record,j.status FROM routing_history h JOIN jobs j ON j.id=h.job_id WHERE (?1 IS NULL OR h.owner=?1) ORDER BY h.created_at,h.job_id")?;
        let rows = stmt.query_map([owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (raw, status) = row?;
            let mut value: Value = serde_json::from_str(&raw)?;
            value["outcome"] = json!(status);
            Ok(value)
        })
        .collect()
    }
    /// Remove dataset records for one owner or all owners, preserving retry deduplication.
    /// # Errors
    /// Returns storage errors.
    pub fn purge_history(&self, owner: Option<&str>) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM routing_history WHERE ?1 IS NULL OR owner=?1",
            [owner],
        )?)
    }
}
fn omit_binary(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.remove("base64");
            fields.remove("data_base64");
            for child in fields.values_mut() {
                omit_binary(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                omit_binary(child);
            }
        }
        _ => {}
    }
}

/// Prune idle-server datasets once per minute, including protected brokers.
/// # Errors
/// Returns database or policy errors to server supervision.
pub async fn maintain(dir: std::path::PathBuf) -> Result<()> {
    loop {
        {
            let store = Store::open(&dir)?;
            store.prune_history(&store.history_policy()?)?;
            store.prune_uploads()?;
        }
        tokio::time::sleep(std::time::Duration::from_mins(1)).await;
    }
}
