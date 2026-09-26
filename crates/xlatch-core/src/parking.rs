//! Durable owner-scoped content saved before an execution target is chosen.

use crate::{
    capability::{Capability, Job, MAX_BYTES},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

struct StoredParkedItem {
    item: ParkedItem,
    input: Value,
}

impl Store {
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
        })
    }

    /// List parked content without returning potentially large bodies.
    ///
    /// # Errors
    /// Returns storage errors.
    pub fn parked(&self, owner: &str) -> Result<Vec<ParkedItem>> {
        let mut statement = self.conn.prepare(
            "SELECT id,label,mime_type,created_at FROM parked_items WHERE (owner=?1 OR ?1='local') ORDER BY created_at DESC,rowid DESC LIMIT 1000",
        )?;
        Ok(statement
            .query_map([owner], |row| {
                Ok(ParkedItem {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    mime_type: row.get(2)?,
                    created_at: row.get(3)?,
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

    fn parked_item(&self, owner: &str, id: &str) -> Result<StoredParkedItem> {
        self.conn
            .query_row(
                "SELECT id,label,mime_type,input,created_at FROM parked_items WHERE id=?1 AND (owner=?2 OR ?2='local')",
                params![id, owner],
                |row| {
                    let input: String = row.get(3)?;
                    Ok((
                        ParkedItem {
                            id: row.get(0)?,
                            label: row.get(1)?,
                            mime_type: row.get(2)?,
                            created_at: row.get(4)?,
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

fn mime_matches(accepted: &str, actual: &str) -> bool {
    accepted == "*/*"
        || accepted == actual
        || accepted
            .strip_suffix("/*")
            .is_some_and(|prefix| actual.starts_with(&format!("{prefix}/")))
}
