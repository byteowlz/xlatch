//! Bounded, signed chunk transfer. Complete artifacts are immutable and owner-scoped.
use crate::{
    capability::{Execution, Job, Manifest},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

/// Raw chunk size, independent of the total file size.
pub const CHUNK_BYTES: usize = 1024 * 1024;
/// Adapter receives a temporary executor-local file path instead of inline bytes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileInput {
    /// Stream/copy from `file.path` before the job completes.
    Path,
}
/// Operator storage policy; no per-file ceiling by default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadPolicy {
    /// Optional maximum bytes per file.
    pub max_file_bytes: Option<u64>,
    /// Aggregate reserved upload bytes, including incomplete files.
    pub quota_bytes: u64,
    /// Retain inactive artifacts for this many hours.
    pub retention_hours: u32,
}
impl Default for UploadPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: None,
            quota_bytes: 64 * 1024 * 1024 * 1024,
            retention_hours: 168,
        }
    }
}
/// Authenticated upload operations. IDs double as stable retry identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum UploadRequest {
    /// Create or resume the exact same upload.
    Begin {
        /// Client-generated UUID.
        id: String,
        /// Original display basename.
        name: String,
        /// Declared media type.
        mime_type: String,
        /// Total raw bytes.
        size: u64,
    },
    /// Append or acknowledge an identical previously accepted chunk.
    Chunk {
        /// Upload UUID.
        id: String,
        /// Byte offset.
        offset: u64,
        /// At most one `MiB` decoded.
        data_base64: String,
    },
    /// Delete an unbound upload owned by this caller.
    Abort {
        /// Upload UUID.
        id: String,
    },
}
/// Execute an enrolled caller's upload request.
/// # Errors
/// Rejects ownership, limits, offsets, changes and exhausted quotas.
pub fn dispatch(store: &Store, owner: &str, request: UploadRequest) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    store.prune_uploads()?;
    let result = match request {
        UploadRequest::Begin {
            id,
            name,
            mime_type,
            size,
        } => store.begin_upload(owner, &id, &name, &mime_type, size)?,
        UploadRequest::Chunk {
            id,
            offset,
            data_base64,
        } => store.upload_chunk(owner, &id, offset, &data_base64)?,
        UploadRequest::Abort { id } => {
            ensure!(
                !store.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM job_uploads WHERE upload_id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0)
                )?,
                "upload is bound to a job"
            );
            store.conn.execute(
                "DELETE FROM uploads WHERE id=?1 AND owner=?2",
                params![id, owner],
            )?;
            json!({"removed":true})
        }
    };
    tx.commit()?;
    Ok(result)
}
impl Store {
    /// Read configurable aggregate storage limits.
    /// # Errors
    /// Returns invalid policy or storage errors.
    pub fn upload_policy(&self) -> Result<UploadPolicy> {
        let body: Option<String> = self
            .conn
            .query_row("SELECT body FROM upload_policy WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        body.map_or_else(
            || Ok(UploadPolicy::default()),
            |v| Ok(serde_json::from_str(&v)?),
        )
    }
    /// Change policy through trusted local-operator control only.
    /// # Errors
    /// Rejects unrepresentable quotas and invalid retention.
    pub fn set_upload_policy(&self, policy: &UploadPolicy) -> Result<()> {
        ensure!(
            policy.quota_bytes > 0
                && i64::try_from(policy.quota_bytes).is_ok()
                && policy
                    .max_file_bytes
                    .is_none_or(|limit| i64::try_from(limit).is_ok())
                && policy.retention_hours > 0
                && policy.retention_hours <= 8760,
            "invalid upload policy"
        );
        self.conn.execute(
            "INSERT OR REPLACE INTO upload_policy VALUES(1,?1)",
            [serde_json::to_string(policy)?],
        )?;
        Ok(())
    }
    fn begin_upload(
        &self,
        owner: &str,
        id: &str,
        name: &str,
        mime: &str,
        size: u64,
    ) -> Result<Value> {
        uuid::Uuid::parse_str(id)?;
        ensure!(
            i64::try_from(size).is_ok()
                && !name.is_empty()
                && name.len() <= 180
                && !name.contains(['/', '\\'])
                && name != "."
                && name != ".."
                && !name.chars().any(char::is_control)
                && mime.len() <= 120
                && mime.contains('/'),
            "invalid upload metadata"
        );
        let existing: Option<(String, String, String, u64, u64)> = self
            .conn
            .query_row(
                "SELECT owner,name,mime,size,received FROM uploads WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get::<_, i64>(3)? as u64,
                        r.get::<_, i64>(4)? as u64,
                    ))
                },
            )
            .optional()?;
        if let Some((who, n, m, s, received)) = existing {
            ensure!(
                who == owner && n == name && m == mime && s == size,
                "upload identity already used"
            );
            return Ok(
                json!({"id":id,"offset":received,"complete":received==size,"chunk_bytes":CHUNK_BYTES}),
            );
        }
        let policy = self.upload_policy()?;
        ensure!(
            policy.max_file_bytes.is_none_or(|limit| size <= limit),
            "file exceeds configured upload limit"
        );
        let reserved: i64 =
            self.conn
                .query_row("SELECT COALESCE(SUM(size),0) FROM uploads", [], |r| {
                    r.get(0)
                })?;
        ensure!(
            size <= policy.quota_bytes.saturating_sub(reserved as u64),
            "upload storage quota exhausted"
        );
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM uploads", [], |r| r.get(0))?;
        ensure!(count < 10000, "too many uploads");
        self.conn.execute(
            "INSERT INTO uploads VALUES(?1,?2,?3,?4,?5,0,?6)",
            params![
                id,
                owner,
                name,
                mime,
                i64::try_from(size)?,
                now() + i64::from(policy.retention_hours) * 3600
            ],
        )?;
        Ok(json!({"id":id,"offset":0,"complete":size==0,"chunk_bytes":CHUNK_BYTES}))
    }
    fn upload_chunk(&self, owner: &str, id: &str, offset: u64, encoded: &str) -> Result<Value> {
        ensure!(
            encoded.len() <= CHUNK_BYTES.div_ceil(3) * 4,
            "chunk too large"
        );
        let bytes = STANDARD.decode(encoded)?;
        let (size, received): (u64, u64) = self.conn.query_row(
            "SELECT size,received FROM uploads WHERE id=?1 AND owner=?2",
            params![id, owner],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?;
        ensure!(
            offset <= size
                && !bytes.is_empty()
                && bytes.len() <= CHUNK_BYTES
                && bytes.len() as u64 <= size - offset
                && offset.is_multiple_of(CHUNK_BYTES as u64)
                && (bytes.len() == CHUNK_BYTES || offset + bytes.len() as u64 == size),
            "invalid chunk range"
        );
        if offset < received {
            let prior: Vec<u8> = self.conn.query_row(
                "SELECT data FROM upload_chunks WHERE upload_id=?1 AND offset=?2",
                params![id, i64::try_from(offset)?],
                |r| r.get(0),
            )?;
            ensure!(prior == bytes, "retry changed uploaded bytes");
        } else {
            ensure!(offset == received, "resume from server offset");
            self.conn.execute(
                "INSERT INTO upload_chunks VALUES(?1,?2,?3)",
                params![id, i64::try_from(offset)?, bytes],
            )?;
            self.conn.execute(
                "UPDATE uploads SET received=received+?2 WHERE id=?1",
                params![id, bytes.len() as i64],
            )?;
        }
        let next = received.max(offset + bytes.len() as u64);
        Ok(json!({"id":id,"offset":next,"complete":next==size,"chunk_bytes":CHUNK_BYTES}))
    }
    pub(crate) fn validate_file_input(
        &self,
        owner: &str,
        manifest: &Manifest,
        input: &Value,
    ) -> Result<Option<String>> {
        let mut schema_input = input.clone();
        let first = match &manifest.execution {
            Execution::Compose { steps } => &steps.first().context("empty composition")?.manifest,
            _ => manifest,
        };
        ensure!(
            first.file_input != Some(FileInput::Path) || input.pointer("/file/path").is_none(),
            "file paths must be supplied by the executor, not the caller"
        );
        let Some(id) = input.pointer("/file/artifact_id").and_then(Value::as_str) else {
            ensure!(
                jsonschema::validator_for(&manifest.input_schema)?.is_valid(input),
                "input does not match capability schema"
            );
            return Ok(None);
        };
        ensure!(
            matches!(first.execution, Execution::SaveFile { .. })
                || first.file_input == Some(FileInput::Path),
            "target needs file-path support for large uploads"
        );
        let (name,mime,size):(String,String,u64) = self.conn.query_row("SELECT name,mime,size FROM uploads WHERE id=?1 AND owner=?2 AND received=size AND expires>?3",params![id,owner,now()],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)? as u64)))?;
        let file = input.get("file").context("file missing")?;
        ensure!(
            file["name"] == name
                && file["mime_type"] == mime
                && file["size"] == size
                && file.get("path").is_none()
                && file.get("data_base64").is_none(),
            "artifact metadata mismatch"
        );
        if matches!(first.execution, Execution::SaveFile { .. }) {
            schema_input["file"] = json!({"name":name,"mime_type":mime,"data_base64":""});
        }
        ensure!(
            jsonschema::validator_for(&manifest.input_schema)?.is_valid(&schema_input),
            "input does not match capability schema"
        );
        Ok(Some(id.into()))
    }
    pub(crate) fn bind_upload(&self, job: &str, upload: Option<&str>) -> Result<()> {
        if let Some(id) = upload {
            self.conn
                .execute("INSERT INTO job_uploads VALUES(?1,?2)", params![job, id])?;
        }
        Ok(())
    }
    /// Read a bounded chunk only for an authorized, running job bound to that upload.
    /// # Errors
    /// Rejects revoked grants, cancelled jobs and unbound artifacts.
    pub fn job_upload_chunk(&self, job: &Job, offset: u64) -> Result<Value> {
        self.authorize_job(job)?;
        ensure!(job.status == "running", "job is not running");
        let bytes: Vec<u8> = self.conn.query_row("SELECT c.data FROM upload_chunks c JOIN job_uploads j ON j.upload_id=c.upload_id WHERE j.job_id=?1 AND c.offset=?2",params![job.id,i64::try_from(offset)?],|r|r.get(0))?;
        Ok(json!({"data_base64":STANDARD.encode(bytes)}))
    }
    /// Remove expired artifacts only when no referencing job or parent remains active.
    /// # Errors
    /// Returns storage failures.
    pub fn prune_uploads(&self) -> Result<()> {
        self.conn.execute("DELETE FROM job_uploads WHERE upload_id IN (SELECT id FROM uploads WHERE expires<?1) AND NOT EXISTS(SELECT 1 FROM job_uploads other JOIN jobs j ON j.id=other.job_id WHERE other.upload_id=job_uploads.upload_id AND j.status IN ('queued','running'))",[now()])?;
        self.conn.execute("DELETE FROM uploads WHERE expires<?1 AND NOT EXISTS(SELECT 1 FROM job_uploads WHERE upload_id=uploads.id)",[now()])?;
        Ok(())
    }
}
/// Private staged copy, removed after completion or cancellation.
#[derive(Debug)]
pub struct StagedFile {
    /// Executor-local path, never selected by a caller.
    pub path: PathBuf,
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
/// Materialize an authorized upload in bounded chunks for an existing native adapter.
/// # Errors
/// Propagates transfer, cancellation and storage failures.
pub async fn stage<F, Fut>(directory: &Path, job: &Job, mut fetch: F) -> Result<Option<StagedFile>>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    if job.input.pointer("/file/artifact_id").is_none() {
        return Ok(None);
    }
    let size = job
        .input
        .pointer("/file/size")
        .and_then(Value::as_u64)
        .context("missing artifact size")?;
    let staged = StagedFile {
        path: directory.join(format!(".xlatch-file-{}", uuid::Uuid::new_v4())),
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staged.path)?;
    let mut offset = 0;
    while offset < size {
        let chunk = fetch(offset).await?;
        let encoded = chunk["data_base64"].as_str().context("missing chunk")?;
        ensure!(
            encoded.len() <= CHUNK_BYTES.div_ceil(3) * 4,
            "oversized artifact chunk"
        );
        let bytes = STANDARD.decode(encoded)?;
        ensure!(
            !bytes.is_empty() && bytes.len() as u64 <= size - offset,
            "invalid artifact chunk"
        );
        file.write_all(&bytes)?;
        offset += bytes.len() as u64;
        tokio::task::yield_now().await;
    }
    file.sync_all()?;
    Ok(Some(staged))
}
