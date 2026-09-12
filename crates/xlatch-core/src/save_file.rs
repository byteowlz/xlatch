//! Fixed-directory, non-overwriting saves for shared content.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

pub fn save(directory: &str, input: &Value) -> Result<Value> {
    let directory = Path::new(directory);
    ensure!(
        std::fs::canonicalize(directory)? == directory,
        "save destination changed since approval"
    );
    let (name, bytes) = if let Some(file) = input.get("file") {
        let raw = file["name"].as_str().context("missing filename")?;
        let name = raw.rsplit(['/', '\\']).next().context("missing filename")?;
        ensure!(
            !name.is_empty()
                && name != "."
                && name != ".."
                && name.len() <= 180
                && !name.chars().any(char::is_control),
            "invalid filename"
        );
        (
            name.to_owned(),
            STANDARD.decode(file["data_base64"].as_str().context("missing file data")?)?,
        )
    } else {
        (
            format!("shared-{}.txt", crate::store::now()),
            input["text"]
                .as_str()
                .context("missing shared text")?
                .as_bytes()
                .to_vec(),
        )
    };
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "shared content exceeds 4 MiB"
    );
    for suffix in 0..1000 {
        let filename = if suffix == 0 {
            name.clone()
        } else {
            let path = Path::new(&name);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("invalid filename")?;
            path.extension().and_then(|s| s.to_str()).map_or_else(
                || format!("{stem}-{suffix}"),
                |extension| format!("{stem}-{suffix}.{extension}"),
            )
        };
        let path = directory.join(&filename);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        file.write_all(&bytes)?;
        file.sync_all()?;
        return Ok(
            json!({"text":format!("Saved {filename}"),"filename":filename,"path":path,"bytes":bytes.len()}),
        );
    }
    anyhow::bail!("too many filename collisions")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_content_without_traversal_or_overwrite() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("xlatch-save-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let dir = std::fs::canonicalize(dir)?;
        let name = dir.to_str().context("invalid temp path")?;
        let input =
            json!({"file":{"name":"../../note.txt","data_base64":STANDARD.encode(b"hello")}});
        let first = save(name, &input)?;
        let second = save(name, &input)?;
        ensure!(
            first["filename"] == "note.txt" && second["filename"] == "note-1.txt",
            "collision names incorrect"
        );
        ensure!(
            std::fs::read(dir.join("note.txt"))? == b"hello",
            "saved bytes differ"
        );
        ensure!(
            save(name, &json!({"file":{"name":"..","data_base64":""}})).is_err(),
            "unsafe name accepted"
        );
        let text = save(name, &json!({"text":"a shared link"}))?;
        ensure!(
            std::fs::read_to_string(text["path"].as_str().context("missing path")?)?
                == "a shared link",
            "text differs"
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
