//! Exercise the desktop client at the real Unix wire boundary.
#![cfg(unix)]
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use xlatch_core::{capability::Request, local::Control};

#[test]
fn registry_and_jobs_use_control_protocol() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = UnixListener::bind(directory.path().join("control.sock"))?;
    let server = std::thread::spawn(move || -> Result<()> {
        for expected in [Request::Discover, Request::Jobs] {
            let (mut stream, _) = listener.accept()?;
            let mut body = String::new();
            BufReader::new(&mut stream).read_line(&mut body)?;
            let received: Value = serde_json::from_str(&body)?;
            ensure!(received == serde_json::to_value(Control::Rpc { request: expected })?);
            writeln!(stream, "{}", json!({"ok":true,"value":[]}))?;
        }
        Ok(())
    });
    let snapshot = xlatch_desktop::snapshot(directory.path())?;
    ensure!(serde_json::to_value(snapshot)? == json!({"capabilities":[],"jobs":[]}));
    server
        .join()
        .map_err(|_| anyhow::anyhow!("fixture failed"))??;
    Ok(())
}

#[test]
fn protected_denial_is_not_reported_as_success() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let listener = UnixListener::bind(directory.path().join("control.sock"))?;
    let server = std::thread::spawn(move || -> Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut body = String::new();
        BufReader::new(&mut stream).read_line(&mut body)?;
        writeln!(
            stream,
            "{}",
            json!({"ok":false,"error":"phone approval required"})
        )?;
        Ok(())
    });
    let response = xlatch_desktop::call(
        directory.path(),
        Control::Approve {
            id: "example".into(),
            revision: "a".repeat(64),
            allow_host_execution: true,
        },
    );
    ensure!(response.is_err_and(|error| error.to_string().contains("phone approval required")));
    server
        .join()
        .map_err(|_| anyhow::anyhow!("fixture failed"))??;
    Ok(())
}
