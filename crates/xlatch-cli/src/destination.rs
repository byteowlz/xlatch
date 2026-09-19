//! Operator-defined save destinations using the normal registry and grant flow.
use anyhow::Result;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use xlatch_core::{
    capability::{Capability, Execution, Manifest},
    local::{self, Control},
};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add and approve a fixed save directory; optionally grant it to a paired device.
    Add {
        /// Action name, such as incoming or documents.
        name: String,
        /// Destination directory; defaults to ~/xlatch/incoming.
        directory: Option<PathBuf>,
        #[arg(long)]
        device: Option<String>,
    },
}

pub async fn add(
    data_dir: &Path,
    name: &str,
    directory: Option<PathBuf>,
    device: Option<&str>,
) -> Result<Capability> {
    let path = xlatch_core::paths::expand_path(
        &directory.unwrap_or_else(|| PathBuf::from("~/xlatch/incoming")),
    )?;
    std::fs::create_dir_all(&path)?;
    let path = std::fs::canonicalize(path)?;
    let mut manifest: Manifest =
        serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
    manifest.id = format!("save.{name}");
    manifest.title = if name == "incoming" {
        "Save to server".to_owned()
    } else {
        format!("Save to {name}")
    };
    "Save shared text, links or files in this server destination. Existing files are never overwritten.".clone_into(&mut manifest.description);
    manifest.execution = Execution::SaveFile {
        directory: path.to_string_lossy().into_owned(),
    };
    manifest.validate()?;
    let id = manifest.id.clone();
    let revision = manifest.revision()?;
    local::call(data_dir, Control::Register { manifest }).await?;
    let approved = local::call(
        data_dir,
        Control::Approve {
            id: id.clone(),
            revision: revision.clone(),
            allow_host_execution: false,
        },
    )
    .await?;
    if let Some(device) = device {
        local::call(
            data_dir,
            Control::Grant {
                device: device.to_owned(),
                id,
                revision,
            },
        )
        .await?;
    }
    Ok(serde_json::from_value(approved)?)
}

/// Run an operator-selected destination command.
pub async fn dispatch(dir: &std::path::Path, command: Command) -> anyhow::Result<()> {
    let Command::Add {
        name,
        directory,
        device,
    } = command;
    let capability = add(dir, &name, directory, device.as_deref()).await?;
    println!("{}", serde_json::to_string_pretty(&capability)?);
    Ok(())
}
