//! Interactive device enrollment and terminal QR presentation.

use anyhow::{Context, Result, ensure};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};
use xlatch_core::{
    capability::Request,
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
        println!("\nScan in xlatch → Server → Scan QR. Expires in five minutes.\n");
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
        let destination =
            xlatch_core::paths::expand_path(std::path::Path::new("~/xlatch/incoming"))?;
        let answer = prompt(&format!(
            "Add Save to server ({}) and allow this phone to use it? [Y/n] ",
            destination.display()
        ))?;
        ensure!(
            answer.is_empty()
                || answer.eq_ignore_ascii_case("y")
                || answer.eq_ignore_ascii_case("yes"),
            "pairing cancelled"
        );
        let capability = crate::destination::add(dir, "incoming", Some(destination), None).await?;
        return Ok(vec![capability.manifest.id]);
    }
    if active.len() == 1 {
        let entry = active[0];
        let id = entry["manifest"]["id"].as_str().context("missing ID")?;
        let title = entry["manifest"]["title"]
            .as_str()
            .context("missing title")?;
        let answer = prompt(&format!("Allow this phone to use {title} ({id})? [Y/n] "))?;
        ensure!(
            answer.is_empty()
                || answer.eq_ignore_ascii_case("y")
                || answer.eq_ignore_ascii_case("yes"),
            "pairing cancelled"
        );
        return Ok(vec![id.to_owned()]);
    }
    println!("Choose the actions this phone may invoke:");
    for (index, entry) in active.iter().enumerate() {
        println!(
            "  {}. {} ({})",
            index + 1,
            entry["manifest"]["title"]
                .as_str()
                .context("missing title")?,
            entry["manifest"]["id"].as_str().context("missing ID")?
        );
    }
    let answer = prompt("Action numbers or IDs (comma-separated): ")?;
    let selected: Vec<String> = answer
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1))
                .and_then(|index| active.get(index))
                .and_then(|entry| entry["manifest"]["id"].as_str())
                .unwrap_or(value)
                .to_owned()
        })
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
    ensure!(io::stdin().read_line(&mut answer)? > 0, "pairing cancelled");
    Ok(answer.trim().to_owned())
}
