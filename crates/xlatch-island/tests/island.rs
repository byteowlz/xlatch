//! Island frames from job snapshots, and the anchr wire exchange.
#![cfg(unix)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use xlatch_core::capability::{Capability, Job, Manifest};
use xlatch_desktop::Snapshot;
use xlatch_island::{Frame, JOBS, RESULT, SOURCE, Tracker, send, socket_path};

const START: i64 = 1_000;
const LEASE: u64 = 11_000;

fn capability(id: &str, title: &str) -> Result<Capability> {
    let mut manifest: Manifest =
        serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
    manifest.id = id.into();
    manifest.title = title.into();
    Ok(Capability {
        manifest,
        revision: "r".repeat(64),
        status: "active".into(),
    })
}

fn job(id: &str, capability: &str, status: &str, created_at: i64) -> Job {
    Job {
        id: id.into(),
        capability_id: capability.into(),
        revision: "r".repeat(64),
        owner: "local".into(),
        status: status.into(),
        // Shared content must never reach the island.
        input: json!({"text": "secret shared text", "file": {"path": "/private/x"}}),
        result: Some(json!({"file_path": "/private/result"})),
        error: Some("diagnostic with /private/path".into()),
        created_at,
    }
}

fn snapshot(jobs: Vec<Job>) -> Result<Snapshot> {
    Ok(Snapshot {
        capabilities: vec![
            capability("echo", "Return shared content")?,
            capability("sessions.send", "Send to an agent session")?,
        ],
        jobs,
    })
}

fn publish(id: &'static str, label: &str, detail: &str, ttl_ms: u64) -> Frame {
    Frame::Publish {
        version: 1,
        source: SOURCE,
        id,
        label: label.into(),
        detail: Some(detail.into()),
        ttl_ms,
    }
}

const fn withdraw(id: &'static str) -> Frame {
    Frame::Withdraw {
        version: 1,
        source: SOURCE,
        id,
    }
}

#[test]
fn history_before_start_is_never_announced() -> Result<()> {
    let mut tracker = Tracker::new(START);
    let old = snapshot(vec![
        job("a", "echo", "succeeded", START - 50),
        job("b", "echo", "failed", START - 10),
    ])?;
    ensure!(tracker.update(&old, LEASE).is_empty());
    ensure!(tracker.update(&old, LEASE).is_empty());
    Ok(())
}

#[test]
fn in_flight_work_is_leased_then_announced_once_and_withdrawn() -> Result<()> {
    let mut tracker = Tracker::new(START);
    ensure!(tracker.update(&snapshot(vec![])?, LEASE).is_empty());
    let frames = tracker.update(
        &snapshot(vec![job("s1", "sessions.send", "queued", START + 1)])?,
        LEASE,
    );
    ensure!(
        frames
            == vec![publish(
                JOBS,
                "Send to an agent session",
                "xlatch · queued",
                LEASE
            )],
        "{frames:?}"
    );
    let frames = tracker.update(
        &snapshot(vec![
            job("s1", "sessions.send", "running", START + 1),
            job("e1", "echo", "queued", START + 2),
        ])?,
        LEASE,
    );
    ensure!(
        frames
            == vec![publish(
                JOBS,
                "Send to an agent session",
                "xlatch · running · 1 more",
                LEASE
            )],
        "{frames:?}"
    );
    let finished = snapshot(vec![
        job("s1", "sessions.send", "succeeded", START + 1),
        job("e1", "echo", "running", START + 2),
    ])?;
    let frames = tracker.update(&finished, LEASE);
    ensure!(
        frames
            == vec![
                publish(RESULT, "Send to an agent session", "xlatch · done", 6_000),
                publish(JOBS, "Return shared content", "xlatch · running", LEASE),
            ],
        "{frames:?}"
    );
    let frames = tracker.update(
        &snapshot(vec![
            job("s1", "sessions.send", "succeeded", START + 1),
            job("e1", "echo", "failed", START + 2),
        ])?,
        LEASE,
    );
    ensure!(
        frames
            == vec![
                publish(RESULT, "Return shared content", "xlatch · failed", 12_000),
                withdraw(JOBS),
            ],
        "{frames:?}"
    );
    let still = snapshot(vec![
        job("s1", "sessions.send", "succeeded", START + 1),
        job("e1", "echo", "failed", START + 2),
    ])?;
    ensure!(
        tracker.update(&still, LEASE).is_empty(),
        "outcomes repeated"
    );
    Ok(())
}

#[test]
fn work_that_finished_between_refreshes_is_still_announced() -> Result<()> {
    let mut tracker = Tracker::new(START);
    ensure!(tracker.update(&snapshot(vec![])?, LEASE).is_empty());
    let frames = tracker.update(
        &snapshot(vec![job("fast", "echo", "succeeded", START + 5)])?,
        LEASE,
    );
    ensure!(
        frames
            == vec![publish(
                RESULT,
                "Return shared content",
                "xlatch · done",
                6_000
            )],
        "{frames:?}"
    );
    Ok(())
}

#[test]
fn no_shared_content_reaches_the_island() -> Result<()> {
    let mut tracker = Tracker::new(START);
    let _ = tracker.update(&snapshot(vec![])?, LEASE);
    let mut frames = tracker.update(
        &snapshot(vec![job("a", "echo", "running", START + 1)])?,
        LEASE,
    );
    frames.extend(tracker.update(
        &snapshot(vec![job("a", "echo", "failed", START + 1)])?,
        LEASE,
    ));
    let wire = serde_json::to_string(&frames)?;
    for leak in ["secret", "/private", "diagnostic", "file_path"] {
        ensure!(!wire.contains(leak), "{leak} leaked: {wire}");
    }
    Ok(())
}

#[test]
fn unknown_and_hostile_titles_are_bounded_single_lines() -> Result<()> {
    let mut tracker = Tracker::new(START);
    let mut snap = snapshot(vec![job("u", "unregistered", "running", START + 1)])?;
    let frames = tracker.update(&snap, LEASE);
    ensure!(frames == vec![publish(JOBS, "xlatch action", "xlatch · running", LEASE)]);
    snap.capabilities.push(capability(
        "weird",
        &format!("Line\none\t{}", "x".repeat(300)),
    )?);
    snap.jobs = vec![job("w", "weird", "running", START + 2)];
    let frames = tracker.update(&snap, LEASE);
    let Some(Frame::Publish { label, .. }) = frames.first() else {
        anyhow::bail!("{frames:?}");
    };
    ensure!(
        label.chars().count() == 120 && !label.chars().any(char::is_control),
        "{label:?}"
    );
    Ok(())
}

#[test]
fn losing_xlatch_withdraws_in_flight_work_once() -> Result<()> {
    let mut tracker = Tracker::new(START);
    let _ = tracker.update(
        &snapshot(vec![job("a", "echo", "running", START + 1)])?,
        LEASE,
    );
    ensure!(tracker.lost() == vec![withdraw(JOBS)]);
    ensure!(tracker.lost().is_empty());
    Ok(())
}

#[test]
fn frames_reach_a_host_and_rejections_surface() -> Result<()> {
    // anchr limits socket paths to 103 bytes, so keep the directory short.
    let dir = tempfile::Builder::new().prefix("xi").tempdir_in("/tmp")?;
    let path = dir.path().join("host.sock");
    let listener = UnixListener::bind(&path)?;
    let host = std::thread::spawn(move || -> Result<Vec<Value>> {
        let mut seen = Vec::new();
        for reply in [
            r#"{"ok":true}"#,
            r#"{"ok":false,"error":{"code":"invalid_label"}}"#,
        ] {
            let (stream, _) = listener.accept()?;
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line)?;
            seen.push(serde_json::from_str(&line)?);
            (&stream).write_all(format!("{reply}\n").as_bytes())?;
        }
        Ok(seen)
    });
    send(
        &path,
        &publish(JOBS, "Return shared content", "xlatch · running", LEASE),
    )?;
    let error = send(&path, &withdraw(JOBS))
        .err()
        .context("rejection accepted")?;
    ensure!(format!("{error:#}").contains("invalid_label"), "{error:#}");
    let seen = host
        .join()
        .map_err(|_| anyhow::anyhow!("host thread panicked"))??;
    ensure!(
        seen == vec![
            json!({"type":"publish","version":1,"source":"xlatch","id":"jobs","label":"Return shared content","detail":"xlatch · running","ttl_ms":LEASE}),
            json!({"type":"withdraw","version":1,"source":"xlatch","id":"jobs"}),
        ],
        "{seen:?}"
    );
    ensure!(send(&dir.path().join("missing.sock"), &withdraw(JOBS)).is_err());
    Ok(())
}

#[test]
fn socket_paths_follow_anchr() -> Result<()> {
    ensure!(
        socket_path(Some(PathBuf::from("/tmp/a/host.sock")))? == PathBuf::from("/tmp/a/host.sock")
    );
    ensure!(socket_path(Some(PathBuf::from("relative.sock"))).is_err());
    ensure!(socket_path(Some(PathBuf::from(format!("/tmp/{}", "x".repeat(120))))).is_err());
    Ok(())
}
