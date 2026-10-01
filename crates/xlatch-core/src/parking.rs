//! Durable owner-scoped content saved before an execution target is chosen.

use crate::{
    capability::{Capability, Job, MAX_BYTES},
    chain::Reference,
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Write as _, path::Path};

/// Small list representation; stored content is returned only through dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParkedItem {
    /// Client-generated stable retry identity.
    pub id: String,
    /// Human-readable content summary.
    pub label: String,
    /// MIME type used for deterministic compatibility filtering.
    pub mime_type: String,
    /// Unix creation time.
    pub created_at: i64,
    /// Optional durable preparation associated with this item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<ParkedPreparation>,
}

/// Current state of a client-selected preparation action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParkedPreparation {
    /// Exact capability selected by the client.
    pub capability_id: String,
    /// Exact granted revision selected by the client.
    pub revision: String,
    /// `queued`, `running`, `succeeded`, `failed`, or `cancelled`.
    pub status: String,
    /// Durable preparation job when it was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Safe failure detail when preparation could not be accepted or completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parked content prepared for a trusted local consumer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParkedContent {
    /// List metadata for the source item.
    pub item: ParkedItem,
    /// Original text input, or file metadata with an executor-local path.
    pub input: Value,
    /// Successful typed preparation output, when ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<Value>,
}

struct StoredParkedItem {
    item: ParkedItem,
    input: Value,
}

impl Store {
    /// Save content and optionally enqueue an exact granted preparation action.
    ///
    /// Preparation acceptance or execution failure never removes the original item.
    ///
    /// # Errors
    /// Rejects malformed parked content or a conflicting retry identity.
    pub fn park_with_preparation(
        &mut self,
        owner: &str,
        id: &str,
        label: &str,
        mime_type: &str,
        input: &Value,
        preparation: Option<&Reference>,
    ) -> Result<ParkedItem> {
        self.park(owner, id, label, mime_type, input)?;
        let Some(preparation) = preparation else {
            return Ok(self.parked_item(owner, id)?.item);
        };
        self.prepare_parked(owner, id, preparation)
    }

    /// Save content without selecting or invoking a capability.
    ///
    /// # Errors
    /// Rejects malformed, oversized, foreign, incomplete, or conflicting content.
    pub fn park(
        &self,
        owner: &str,
        id: &str,
        label: &str,
        mime_type: &str,
        input: &Value,
    ) -> Result<ParkedItem> {
        ensure!(uuid::Uuid::parse_str(id).is_ok(), "invalid parked item id");
        ensure!(
            !label.is_empty() && label.len() <= 500 && !label.chars().any(char::is_control),
            "invalid parked item label"
        );
        ensure!(
            mime_type.len() <= 120 && mime_type.contains('/'),
            "invalid MIME type"
        );
        let encoded = serde_json::to_vec(input)?;
        ensure!(encoded.len() <= MAX_BYTES, "parked input is too large");
        ensure!(
            input.get("mime_type").and_then(Value::as_str) == Some(mime_type),
            "parked input MIME type does not match"
        );
        ensure!(
            input.pointer("/file/path").is_none(),
            "file paths cannot be parked"
        );
        let upload_id = input
            .pointer("/file/artifact_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(upload_id) = &upload_id {
            let valid: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM uploads WHERE id=?1 AND owner=?2 AND received=size)",
                params![upload_id, owner],
                |row| row.get(0),
            )?;
            ensure!(valid, "parked upload is incomplete or unavailable");
            let file = input.get("file").context("file metadata missing")?;
            let (name, mime, size): (String, String, u64) = self.conn.query_row(
                "SELECT name,mime,size FROM uploads WHERE id=?1 AND owner=?2",
                params![upload_id, owner],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64)),
            )?;
            ensure!(
                file["name"] == name
                    && file["mime_type"] == mime
                    && file["size"] == size
                    && mime == mime_type,
                "parked upload metadata does not match"
            );
        } else {
            ensure!(
                input.get("text").is_some() || input.pointer("/file/data_base64").is_some(),
                "parked input needs text or file content"
            );
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM parked_items WHERE id=?1 AND (owner=?2 OR ?2='local'))",
            params![id, owner],
            |row| row.get(0),
        )?;
        if exists {
            let existing = self.parked_item(owner, id)?;
            ensure!(
                existing.item.label == label
                    && existing.item.mime_type == mime_type
                    && existing.input == *input,
                "parked item id already used with different content"
            );
            return Ok(existing.item);
        }
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM parked_items WHERE owner=?1",
            [owner],
            |row| row.get(0),
        )?;
        ensure!(count < 1000, "too many parked items");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO parked_items(id,owner,label,mime_type,input,upload_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![id, owner, label, mime_type, serde_json::to_string(input)?, upload_id, created_at],
        )?;
        Ok(ParkedItem {
            id: id.to_owned(),
            label: label.to_owned(),
            mime_type: mime_type.to_owned(),
            created_at,
            preparation: None,
        })
    }

    fn prepare_parked(
        &mut self,
        owner: &str,
        id: &str,
        preparation: &Reference,
    ) -> Result<ParkedItem> {
        let parked = self.parked_item(owner, id)?;
        if let Some(existing) = &parked.item.preparation {
            ensure!(
                existing.capability_id == preparation.capability_id
                    && existing.revision == preparation.revision,
                "parked item id already used with different preparation"
            );
            if existing.job_id.is_some() {
                return Ok(parked.item);
            }
        }
        self.conn.execute(
            "UPDATE parked_items SET preparation_capability_id=?1,preparation_revision=?2,preparation_error=NULL WHERE id=?3 AND owner=?4",
            params![preparation.capability_id, preparation.revision, id, owner],
        )?;
        let outcome = self.invoke(
            owner,
            &preparation.capability_id,
            &preparation.revision,
            &parked.input,
            &format!("park-prepare:{id}"),
        );
        match outcome {
            Ok(job) => {
                self.conn.execute(
                    "UPDATE parked_items SET preparation_job_id=?1,preparation_error=NULL WHERE id=?2 AND owner=?3",
                    params![job.id, id, owner],
                )?;
            }
            Err(error) => {
                self.conn.execute(
                    "UPDATE parked_items SET preparation_error=?1 WHERE id=?2 AND owner=?3",
                    params![error.to_string(), id, owner],
                )?;
            }
        }
        Ok(self.parked_item(owner, id)?.item)
    }

    /// List parked content without returning potentially large bodies.
    ///
    /// # Errors
    /// Returns storage errors.
    pub fn parked(&self, owner: &str) -> Result<Vec<ParkedItem>> {
        let mut statement = self.conn.prepare(
            "SELECT p.id,p.label,p.mime_type,p.created_at,p.preparation_capability_id,p.preparation_revision,p.preparation_job_id,COALESCE(j.status,CASE WHEN p.preparation_capability_id IS NOT NULL THEN 'failed' END),COALESCE(j.error,p.preparation_error) FROM parked_items p LEFT JOIN jobs j ON j.id=p.preparation_job_id WHERE (p.owner=?1 OR ?1='local') ORDER BY p.created_at DESC,p.rowid DESC LIMIT 1000",
        )?;
        Ok(statement
            .query_map([owner], |row| {
                Ok(ParkedItem {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    mime_type: row.get(2)?,
                    created_at: row.get(3)?,
                    preparation: preparation_from_row(row, 4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Return granted active capabilities that can accept this exact parked input.
    ///
    /// # Errors
    /// Returns an error when the item is unavailable or storage cannot be read.
    pub fn parked_candidates(&self, owner: &str, id: &str) -> Result<Vec<Capability>> {
        let parked = self.parked_item(owner, id)?;
        let candidates = self
            .discover(owner)?
            .into_iter()
            .filter(|capability| {
                capability
                    .manifest
                    .accepts
                    .iter()
                    .any(|accepted| mime_matches(accepted, &parked.item.mime_type))
            })
            .filter_map(|capability| {
                self.validate_file_input(owner, &capability.manifest, &parked.input)
                    .ok()
                    .map(|_| capability)
            })
            .collect();
        Ok(candidates)
    }

    /// Atomically create or recover the one job for a parked item, then remove it.
    ///
    /// # Errors
    /// Rejects unavailable items and inactive, stale, incompatible, or ungranted targets.
    pub fn dispatch_parked(
        &mut self,
        owner: &str,
        id: &str,
        capability_id: &str,
        revision: &str,
    ) -> Result<Job> {
        let key = format!("park:{id}");
        let parked = self.parked_item(owner, id);
        let job = match parked {
            Ok(parked) => self.invoke(owner, capability_id, revision, &parked.input, &key)?,
            Err(error) => {
                let job_id = self
                    .conn
                    .query_row(
                        "SELECT id FROM jobs WHERE owner=?1 AND idempotency_key=?2",
                        params![owner, key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                let Some(job_id) = job_id else {
                    return Err(error);
                };
                let job = self.job(owner, &job_id)?;
                ensure!(
                    job.capability_id == capability_id && job.revision == revision,
                    "parked item was dispatched to a different target"
                );
                job
            }
        };
        self.conn.execute(
            "DELETE FROM parked_items WHERE id=?1 AND (owner=?2 OR ?2='local')",
            params![id, owner],
        )?;
        Ok(job)
    }

    /// Delete an owned parked item and its unbound upload.
    ///
    /// # Errors
    /// Rejects unavailable items and propagates storage errors.
    pub fn delete_parked(&self, owner: &str, id: &str) -> Result<()> {
        let parked = self.parked_item(owner, id)?;
        let upload_id = parked
            .input
            .pointer("/file/artifact_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.conn.execute(
            "DELETE FROM parked_items WHERE id=?1 AND (owner=?2 OR ?2='local')",
            params![id, owner],
        )?;
        if let Some(upload_id) = upload_id {
            self.conn.execute(
                "DELETE FROM uploads WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM job_uploads WHERE upload_id=?1)",
                [upload_id],
            )?;
        }
        Ok(())
    }

    /// Read parked content for a trusted local consumer without removing it.
    ///
    /// Files are copied into an explicit canonical directory and represented by
    /// a local path; arbitrary-size upload bytes never enter the control response.
    ///
    /// # Errors
    /// Rejects unavailable items, unsafe destinations, and malformed file content.
    pub fn read_parked(&self, owner: &str, id: &str, directory: &Path) -> Result<ParkedContent> {
        let parked = self.parked_item(owner, id)?;
        if parked.input.get("file").is_none() {
            let prepared = self.prepared_result(&parked.item)?;
            return Ok(ParkedContent {
                item: parked.item,
                input: parked.input,
                prepared,
            });
        }
        ensure!(
            std::fs::canonicalize(directory)? == directory,
            "parked export directory must be canonical"
        );
        let file = parked.input.get("file").context("file metadata missing")?;
        let raw_name = file["name"].as_str().context("file name missing")?;
        let name = safe_name(raw_name)?;
        let mime_type = file["mime_type"]
            .as_str()
            .unwrap_or(&parked.item.mime_type)
            .to_owned();
        let (path, mut output) = create_export(directory, &name)?;
        let mut pending = PendingExport {
            path: path.clone(),
            complete: false,
        };
        let bytes = if let Some(upload_id) = file["artifact_id"].as_str() {
            let mut statement = self
                .conn
                .prepare("SELECT data FROM upload_chunks WHERE upload_id=?1 ORDER BY offset")?;
            let mut rows = statement.query([upload_id])?;
            let mut total = 0u64;
            while let Some(row) = rows.next()? {
                let chunk: Vec<u8> = row.get(0)?;
                output.write_all(&chunk)?;
                total += chunk.len() as u64;
            }
            let expected = file["size"].as_u64().context("file size missing")?;
            ensure!(total == expected, "parked upload is incomplete");
            total
        } else {
            let data = STANDARD.decode(
                file["data_base64"]
                    .as_str()
                    .context("inline file data missing")?,
            )?;
            output.write_all(&data)?;
            data.len() as u64
        };
        output.sync_all()?;
        pending.complete = true;
        let mut input = parked.input;
        input["file"] = json!({
            "name": name,
            "mime_type": mime_type,
            "size": bytes,
            "path": path,
        });
        Ok(ParkedContent {
            prepared: self.prepared_result(&parked.item)?,
            item: parked.item,
            input,
        })
    }

    fn prepared_result(&self, item: &ParkedItem) -> Result<Option<Value>> {
        let Some(preparation) = &item.preparation else {
            return Ok(None);
        };
        let Some(job_id) = &preparation.job_id else {
            return Ok(None);
        };
        if preparation.status != "succeeded" {
            return Ok(None);
        }
        Ok(self.job("local", job_id)?.result)
    }

    fn parked_item(&self, owner: &str, id: &str) -> Result<StoredParkedItem> {
        self.conn
            .query_row(
                "SELECT p.id,p.label,p.mime_type,p.input,p.created_at,p.preparation_capability_id,p.preparation_revision,p.preparation_job_id,COALESCE(j.status,CASE WHEN p.preparation_capability_id IS NOT NULL THEN 'failed' END),COALESCE(j.error,p.preparation_error) FROM parked_items p LEFT JOIN jobs j ON j.id=p.preparation_job_id WHERE p.id=?1 AND (p.owner=?2 OR ?2='local')",
                params![id, owner],
                |row| {
                    let input: String = row.get(3)?;
                    Ok((
                        ParkedItem {
                            id: row.get(0)?,
                            label: row.get(1)?,
                            mime_type: row.get(2)?,
                            created_at: row.get(4)?,
                            preparation: preparation_from_row(row, 5)?,
                        },
                        input,
                    ))
                },
            )
            .context("parked item not found")
            .and_then(|(item, input)| {
                Ok(StoredParkedItem {
                    item,
                    input: serde_json::from_str(&input)?,
                })
            })
    }
}

fn preparation_from_row(
    row: &rusqlite::Row<'_>,
    start: usize,
) -> rusqlite::Result<Option<ParkedPreparation>> {
    let capability_id: Option<String> = row.get(start)?;
    let revision: Option<String> = row.get(start + 1)?;
    let job_id: Option<String> = row.get(start + 2)?;
    let status: Option<String> = row.get(start + 3)?;
    let error: Option<String> = row.get(start + 4)?;
    Ok(capability_id.map(|capability_id| ParkedPreparation {
        capability_id,
        revision: revision.unwrap_or_default(),
        job_id,
        status: status.unwrap_or_else(|| "failed".to_owned()),
        error,
    }))
}

fn mime_matches(accepted: &str, actual: &str) -> bool {
    accepted == "*/*"
        || accepted == actual
        || accepted
            .strip_suffix("/*")
            .is_some_and(|prefix| actual.starts_with(&format!("{prefix}/")))
}

fn safe_name(raw: &str) -> Result<String> {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .context("file name missing")?;
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && name.len() <= 180
            && !name.chars().any(char::is_control),
        "invalid file name"
    );
    Ok(name.to_owned())
}

fn create_export(directory: &Path, name: &str) -> Result<(std::path::PathBuf, std::fs::File)> {
    for suffix in 0..1000 {
        let filename = if suffix == 0 {
            name.to_owned()
        } else {
            let path = Path::new(name);
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .context("invalid file name")?;
            path.extension()
                .and_then(|value| value.to_str())
                .map_or_else(
                    || format!("{stem}-{suffix}"),
                    |extension| format!("{stem}-{suffix}.{extension}"),
                )
        };
        let path = directory.join(filename);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("too many parked export filename collisions")
}

struct PendingExport {
    path: std::path::PathBuf,
    complete: bool,
}

impl Drop for PendingExport {
    fn drop(&mut self) {
        if !self.complete {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
