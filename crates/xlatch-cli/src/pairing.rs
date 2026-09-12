//! Interactive device enrollment and terminal QR presentation.

use anyhow::{Context, Result, ensure};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};
use xlatch_core::{
    capability::{Manifest, Request},
    local::{self, Control},
};

pub async fn run(
    dir: &Path,
    mut capabilities: Vec<String>,
    qr: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal() && !json;
    if capabilities.is_empty() {
        ensure!(
            interactive,
            "specify --capability ID in scripts; run pair in a terminal for guided selection"
        );
        capabilities = select_capabilities(dir).await?;
    }
    let value = local::call(dir, Control::Pair { capabilities }).await?;
    if !json && io::stdout().is_terminal() {
        let encoded = serde_json::to_string(&value)?;
        let code = qrcode::QrCode::new(encoded.as_bytes())?;
        // Explicit colors keep the QR readable on both light and dark terminal themes.
        let rendered = code.render::<qrcode::render::unicode::Dense1x2>().build();
        println!("\nScan in CrossLatch → Server → Scan QR. Expires in five minutes.\n");
        for line in rendered.lines() {
            println!("\x1b[30;47m{line}\x1b[0m");
        }
        if let Some(path) = qr {
            std::fs::write(
                path,
                code.render::<qrcode::render::svg::Color<'_>>()
                    .min_dimensions(512, 512)
                    .build(),
            )?;
        }
        return Ok(());
    }
    super::finish_output(dir, value, qr, None).await
}

async fn select_capabilities(dir: &Path) -> Result<Vec<String>> {
    let value = local::call(
        dir,
        Control::Rpc {
            request: Request::Discover,
        },
    )
    .await
    .context(
        "Cannot reach the daemon. Start xlatch service run first, using the same --data-dir.",
    )?;
    let entries = value.as_array().context("invalid capability list")?;
    let active: Vec<_> = entries
        .iter()
        .filter(|entry| entry["status"] == "active")
        .collect();
    if active.is_empty() {
        ensure!(
            entries.is_empty(),
            "No approved actions. Review and approve a pending manifest before pairing."
        );
        println!(
            "No actions yet. The test action returns the text or file you share, without running a command."
        );
        ensure!(
            prompt("Create, approve and grant this test action? [y/N] ")?.eq_ignore_ascii_case("y"),
            "pairing cancelled"
        );
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
        let revision = manifest.revision()?;
        let id = manifest.id.clone();
        local::call(dir, Control::Register { manifest }).await?;
        local::call(
            dir,
            Control::Approve {
                id: id.clone(),
                revision,
                allow_host_execution: false,
            },
        )
        .await?;
        return Ok(vec![id]);
    }
    println!("Choose the actions this phone may invoke:");
    for entry in &active {
        println!(
            "  {} — {}",
            entry["manifest"]["id"].as_str().context("missing ID")?,
            entry["manifest"]["title"]
                .as_str()
                .context("missing title")?
        );
    }
    let answer = prompt("Capability IDs (comma-separated): ")?;
    let selected: Vec<String> = answer
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    ensure!(!selected.is_empty(), "pairing cancelled");
    ensure!(
        selected.iter().all(|id| active
            .iter()
            .any(|entry| entry["manifest"]["id"].as_str() == Some(id.as_str()))),
        "select only listed capability IDs"
    );
    Ok(selected)
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_owned())
}
