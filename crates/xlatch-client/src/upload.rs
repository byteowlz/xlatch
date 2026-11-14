//! Bounded-memory native file transfer using the broker's owner-scoped uploads.
use crate::{Client, file_input};
use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    io::{Read as _, Seek as _, Write as _},
    path::Path,
};

const CHUNK_BYTES: usize = 1024 * 1024;

/// Locally frozen native file input. Preparation performs no network writes;
/// callers can validate the complete file-plus-prompt schema before upload.
pub struct PreparedFile {
    input: Value,
    snapshot: Option<std::fs::File>,
    owner: String,
}

impl std::fmt::Debug for PreparedFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparedFile([private content])")
    }
}

impl PreparedFile {
    /// Exact future invocation input, including the content-bound artifact ID.
    #[must_use]
    pub const fn input(&self) -> &Value {
        &self.input
    }

    /// Transfer the already frozen bytes after target/schema review. Retrying
    /// this object resumes the same owner-scoped artifact, never a new input.
    ///
    /// # Errors
    /// Returns owner mismatch, upload bounds, storage and transport failures.
    pub async fn upload(&mut self, client: &Client) -> Result<()> {
        ensure!(
            client.identity.device_id.as_deref() == Some(&self.owner),
            "prepared file belongs to a different pairing"
        );
        let Some(snapshot) = &mut self.snapshot else {
            return Ok(());
        };
        let file = &self.input["file"];
        let id = file["artifact_id"]
            .as_str()
            .context("missing artifact id")?;
        let name = file["name"].as_str().context("missing file name")?;
        let mime = file["mime_type"].as_str().context("missing MIME")?;
        let size = file["size"].as_u64().context("missing file size")?;
        let mut state = client.call(&json!({"op":"upload","request":{"action":"begin","id":id,"name":name,"mime_type":mime,"size":size}})).await?;
        let mut offset = validate_state(&state, size)?;
        snapshot.seek(std::io::SeekFrom::Start(offset))?;
        let mut bytes = vec![0; CHUNK_BYTES];
        while offset < size {
            let count = usize::try_from((size - offset).min(CHUNK_BYTES as u64))?;
            snapshot.read_exact(&mut bytes[..count])?;
            state = client.call(&json!({"op":"upload","request":{"action":"chunk","id":id,"offset":offset,"data_base64":STANDARD.encode(&bytes[..count])}})).await?;
            let next = validate_state(&state, size)?;
            ensure!(
                next == offset + count as u64,
                "server upload offset changed unexpectedly"
            );
            offset = next;
        }
        ensure!(state["complete"] == true, "server upload is incomplete");
        Ok(())
    }
}

impl Client {
    /// Freeze input locally without uploading. Large files use private temporary
    /// disk storage and a deterministic content/owner/key-bound upload UUID.
    ///
    /// # Errors
    /// Rejects non-regular files, unsafe names, changing sources or disk failures.
    pub fn prepare_file(&self, path: &Path, mime_type: &str, key: &str) -> Result<PreparedFile> {
        let metadata = std::fs::metadata(path)?;
        ensure!(
            metadata.is_file(),
            "send a regular exported file, not a directory or device"
        );
        let owner = self.identity.device_id.clone().context("not paired")?;
        if metadata.len() <= 4 * 1024 * 1024 {
            return Ok(PreparedFile {
                input: file_input(path, mime_type)?,
                snapshot: None,
                owner,
            });
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid file name")?;
        ensure!(
            name.len() <= 180 && !name.contains(['/', '\\']) && !name.chars().any(char::is_control),
            "invalid upload basename"
        );
        let (snapshot, hash, size) = snapshot(std::fs::File::open(path)?)?;
        let id = upload_id(&owner, key, name, mime_type, &hash);
        Ok(PreparedFile {
            input: json!({"file":{"artifact_id":id,"name":name,"mime_type":mime_type,"size":size},"mime_type":mime_type}),
            snapshot: Some(snapshot),
            owner,
        })
    }

    /// Prepare and upload native input. Existing file-only clients retain this
    /// convenience path; prompted clients should validate before calling upload.
    ///
    /// # Errors
    /// Returns local preparation, quota, ownership or bounded network errors.
    pub async fn file_input(&self, path: &Path, mime_type: &str, key: &str) -> Result<Value> {
        let mut prepared = self.prepare_file(path, mime_type, key)?;
        prepared.upload(self).await?;
        Ok(prepared.input)
    }
}

fn validate_state(state: &Value, size: u64) -> Result<u64> {
    let offset = state["offset"].as_u64().context("missing upload offset")?;
    ensure!(
        offset <= size && state["chunk_bytes"] == CHUNK_BYTES,
        "invalid server upload bounds"
    );
    Ok(offset)
}

fn upload_id(device: &str, key: &str, name: &str, mime: &str, digest: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"xlatch.desktop.upload.v1");
    // Length-framed fields cannot collide through embedded delimiters.
    for field in [
        device.as_bytes(),
        key.as_bytes(),
        name.as_bytes(),
        mime.as_bytes(),
        digest,
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    let digest = hash.finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn snapshot(mut source: std::fs::File) -> Result<(std::fs::File, Vec<u8>, u64)> {
    let before = source.metadata()?;
    let mut temporary = tempfile::tempfile()?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut size = 0;
    loop {
        let count = source.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        ensure!(
            size <= before.len(),
            "source file grew during snapshot; export a finished bundle"
        );
        temporary.write_all(&bytes[..count])?;
        hash.update(&bytes[..count]);
    }
    let after = source.metadata()?;
    ensure!(
        size == before.len() && before.modified().ok() == after.modified().ok(),
        "source file changed during snapshot; export a finished bundle"
    );
    temporary.rewind()?;
    Ok((temporary, hash.finalize().to_vec(), size))
}
