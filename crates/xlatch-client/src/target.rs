//! Granted-target compatibility checks before allocating uploads.
use crate::Client;
use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

impl Client {
    /// Resolve one exact currently granted target and check MIME/file support.
    /// This is advisory preflight, never an approval or replacement for the
    /// server's transactional grant and revision checks at invocation time.
    ///
    /// # Errors
    /// Rejects missing grants, stale revisions and unsupported input types.
    pub async fn target(
        &self,
        id: &str,
        revision: &str,
        mime: Option<&str>,
        large_file: bool,
    ) -> Result<Value> {
        let targets = self.call(&json!({"op":"discover"})).await?;
        let target = targets
            .as_array()
            .context("invalid target list")?
            .iter()
            .find(|target| target["manifest"]["id"] == id && target["revision"] == revision)
            .context("target is not granted at this exact revision; refresh list")?;
        let manifest = &target["manifest"];
        if let Some(mime) = mime {
            ensure!(
                manifest["accepts"]
                    .as_array()
                    .context("missing target MIME types")?
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|accepted| accepts(accepted, mime)),
                "target does not accept this MIME type"
            );
        }
        if large_file {
            ensure!(
                manifest["file_input"] == "path" || manifest["execution"]["kind"] == "save_file",
                "target does not support large artifact inputs; choose a save destination or file_input=path action"
            );
        }
        Ok(target.clone())
    }
}

fn accepts(pattern: &str, mime: &str) -> bool {
    pattern == mime
        || pattern == "*/*"
        || pattern
            .strip_suffix("/*")
            .is_some_and(|prefix| mime.split_once('/').is_some_and(|(kind, _)| kind == prefix))
}
