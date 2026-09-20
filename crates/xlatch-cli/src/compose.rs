//! Assemble a proposed capability from existing active action revisions.
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use xlatch_core::{
    capability::{Capability, Execution, Manifest, Request},
    composition::{Binding, Step},
    local::{self, Control},
};

#[derive(Debug, Args)]
pub struct Options {
    /// New capability ID; existing IDs are never overwritten by this command.
    pub id: String,
    /// Ordered existing action IDs (use --spec for explicit mappings).
    #[arg(long, value_delimiter = ',', conflicts_with = "spec")]
    pub steps: Vec<String>,
    /// JSON or TOML plan containing title, steps and optional `output_schema`.
    #[arg(long, required_unless_present = "steps")]
    pub spec: Option<PathBuf>,
    /// Human-readable composed target name.
    #[arg(long)]
    pub title: Option<String>,
    /// Print the complete proposal without registering it.
    #[arg(long)]
    pub dry_run: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    title: String,
    steps: Vec<Reference>,
    #[serde(default)]
    output_schema: Option<Value>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    capability_id: String,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default = "previous")]
    input: Binding,
}
const fn previous() -> Binding {
    Binding::Previous
}
fn proposal(options: &Options, catalog: &[Capability]) -> Result<Manifest> {
    ensure!(
        !catalog.iter().any(|cap| cap.manifest.id == options.id),
        "composition ID already exists; choose a new ID or explicitly register a reviewed revision"
    );
    let plan: Plan = if let Some(path) = &options.spec {
        let raw = std::fs::read_to_string(path)?;
        if path.extension().is_some_and(|ext| ext == "toml") {
            toml::from_str(&raw)?
        } else {
            serde_json::from_str(&raw)?
        }
    } else {
        Plan {
            title: options.title.clone().unwrap_or_else(|| options.id.clone()),
            steps: options
                .steps
                .iter()
                .map(|id| Reference {
                    capability_id: id.clone(),
                    revision: None,
                    input: Binding::Previous,
                })
                .collect(),
            output_schema: None,
        }
    };
    let mut steps = Vec::new();
    for reference in plan.steps {
        let cap = catalog
            .iter()
            .find(|c| c.manifest.id == reference.capability_id)
            .with_context(|| format!("{} is not registered", reference.capability_id))?;
        ensure!(
            reference
                .revision
                .as_ref()
                .is_none_or(|revision| revision == &cap.revision),
            "step revision changed"
        );
        steps.push(Step {
            manifest: cap.manifest.clone(),
            revision: cap.revision.clone(),
            input: reference.input,
        });
    }
    let first = steps.first().context("choose at least two steps")?;
    let last = steps.last().context("choose at least two steps")?;
    let manifest = Manifest {
        icon: None,
        file_input: if matches!(first.manifest.execution, Execution::SaveFile { .. }) {
            Some(xlatch_core::uploads::FileInput::Path)
        } else {
            first.manifest.file_input
        },
        id: options.id.clone(),
        title: options.title.clone().unwrap_or(plan.title),
        description: format!(
            "Run these exact action revisions in order: {}. Access to this composition delegates only these steps and mappings.",
            steps
                .iter()
                .map(|s| s.manifest.title.as_str())
                .collect::<Vec<_>>()
                .join(" → ")
        ),
        accepts: first.manifest.accepts.clone(),
        input_schema: first.manifest.input_schema.clone(),
        output_schema: plan
            .output_schema
            .unwrap_or_else(|| last.manifest.output_schema.clone()),
        timeout_seconds: steps.iter().map(|s| s.manifest.timeout_seconds).sum(),
        execution: Execution::Compose { steps },
    };
    manifest.validate()?;
    Ok(manifest)
}
pub async fn dispatch(dir: &Path, options: Options) -> Result<()> {
    let catalog: Vec<Capability> = serde_json::from_value(
        local::call(
            dir,
            Control::Rpc {
                request: Request::Discover,
            },
        )
        .await?,
    )?;
    let manifest = proposal(&options, &catalog)?;
    let output = if options.dry_run {
        serde_json::to_value(manifest)?
    } else {
        local::call(dir, Control::Register { manifest }).await?
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
