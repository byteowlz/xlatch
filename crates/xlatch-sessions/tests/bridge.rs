//! Session bridge rules against fake hc/herdr programs (ADR 0008).
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, ensure};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use xlatch_sessions::{FileRef, Listing, Providers, Receipt, SendInput, Session, Unavailable};

const HC: &str = r#"#!/usr/bin/env bash
dir=$(dirname "$0")
trap 'printf "\036" >> "$dir/log"' EXIT
printf 'hc|HOME=%s|' "$HOME" >> "$dir/log"; printf '%s|' "$@" >> "$dir/log"
case "$1" in
  ls) cat "$dir/hc.json" ;;
  put) printf 'content=%s|' "$(cat "$3")" >> "$dir/log"; printf '{"bytes":3,"path":"/landed/%s"}\n' "$(basename "$3")" ;;
  send)
    if [ "$2" = "t-fail" ]; then echo '{"error":{"kind":"x","message":"boom"}}'; exit 5; fi
    echo '{"started":null,"status":null,"turn":0}' ;;
esac
"#;

const HERDR: &str = r#"#!/usr/bin/env bash
dir=$(dirname "$0")
trap 'printf "\036" >> "$dir/log"' EXIT
printf 'herdr|' >> "$dir/log"; printf '%s|' "$@" >> "$dir/log"
if [ -f "$dir/herdr-down" ]; then
  echo '{"id":"cli:agent:list","error":{"code":"server_not_running","message":"no herdr server"}}'; exit 1
fi
case "$2" in
  list) cat "$dir/herdr.json" ;;
  prompt) echo '{"id":"cli:agent:prompt","result":{}}' ;;
esac
"#;

struct Fixture {
    dir: tempfile::TempDir,
    providers: Providers,
}

impl Fixture {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let script = |name: &str, body: &str| -> Result<PathBuf> {
            let path = dir.path().join(name);
            fs::write(&path, body)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            Ok(path)
        };
        let providers = Providers {
            hc: Some(script("hc", HC)?),
            herdr: Some(script("herdr", HERDR)?),
        };
        fs::write(
            dir.path().join("hc.json"),
            json!({"hidden_finished": 0, "terminals": [
                {"address": "t1", "agent": "claude", "cwd": "/w/app", "state": "live", "status": "idle", "turn": 2, "where": "app / main"},
                {"address": "t2", "agent": "sh", "cwd": "/w/govnr", "state": "live", "turn": 0, "where": "govnr / mirror"},
                {"address": "t3", "agent": "codex", "cwd": "/w/old", "state": "finished", "turn": 1},
                {"address": "box/t4", "agent": "codex", "cwd": "/srv", "state": "live", "turn": 0, "name": "remote codex"},
                {"address": "t-fail", "agent": "pi", "cwd": "/w", "state": "live", "turn": 0}
            ]})
            .to_string(),
        )?;
        fs::write(
            dir.path().join("herdr.json"),
            json!({"id": "cli:agent:list", "result": {"agents": [
                {"agent": "pi", "agent_status": "idle", "cwd": "/w/govnr", "pane_id": "w2:p88", "terminal_title_stripped": "govnr-main"},
                {"agent": "pi", "agent_status": "blocked", "cwd": "/w/govnr", "pane_id": "w2:p9", "terminal_title_stripped": "asking"}
            ]}})
            .to_string(),
        )?;
        Ok(Self { dir, providers })
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }

    fn sends(&self) -> Vec<String> {
        self.log()
            .split('\u{1e}')
            .filter(|l| l.contains("|send|") || l.contains("|prompt|") || l.contains("|put|"))
            .map(str::to_owned)
            .collect()
    }
}

fn session(
    provider: &str,
    address: &str,
    agent: &str,
    status: Option<&str>,
    cwd: &str,
    title: Option<&str>,
) -> Session {
    Session {
        id: format!("{provider}:{address}"),
        provider: provider.into(),
        address: address.into(),
        agent: agent.into(),
        status: status.map(Into::into),
        cwd: Some(cwd.into()),
        title: title.map(Into::into),
    }
}

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::new(), |out, b| out + &format!("{b:02x}"))
}

fn send(target: &str, text: &str, file: Option<&Path>) -> SendInput {
    SendInput {
        target: target.into(),
        text: text.into(),
        file: file.map(FileRef::staged),
    }
}

#[test]
fn list_maps_live_agents_and_omits_shells_and_finished_terminals() -> Result<()> {
    let fixture = Fixture::new()?;
    let listing = fixture.providers.list();
    ensure!(
        listing
            == Listing {
                sessions: vec![
                    session(
                        "hc",
                        "t1",
                        "claude",
                        Some("idle"),
                        "/w/app",
                        Some("app / main")
                    ),
                    session("hc", "box/t4", "codex", None, "/srv", Some("remote codex")),
                    session("hc", "t-fail", "pi", None, "/w", None),
                    session(
                        "herdr",
                        "w2:p88",
                        "pi",
                        Some("idle"),
                        "/w/govnr",
                        Some("govnr-main")
                    ),
                    session(
                        "herdr",
                        "w2:p9",
                        "pi",
                        Some("blocked"),
                        "/w/govnr",
                        Some("asking")
                    ),
                ],
                unavailable: vec![],
            },
        "unexpected listing: {listing:#?}"
    );
    Ok(())
}

#[test]
fn an_unavailable_provider_does_not_hide_the_other() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::write(fixture.dir.path().join("herdr-down"), "")?;
    let listing = fixture.providers.list();
    ensure!(listing.sessions.iter().all(|s| s.provider == "hc") && listing.sessions.len() == 3);
    let [Unavailable { provider, error }] = listing.unavailable.as_slice() else {
        anyhow::bail!("{:?}", listing.unavailable);
    };
    ensure!(
        provider == "herdr" && error.ends_with("no herdr server"),
        "{error}"
    );
    Ok(())
}

#[test]
fn text_reaches_a_live_hc_agent_with_a_hashed_receipt() -> Result<()> {
    let fixture = Fixture::new()?;
    let text = "Review the attached plan.\nSecond line.";
    let receipt = fixture.providers.send(&send("hc:t1", text, None))?;
    ensure!(
        receipt
            == Receipt {
                provider: "hc".into(),
                target: "hc:t1".into(),
                agent: "claude".into(),
                text_sha256: sha(text),
                file_path: None,
            }
    );
    let sends = fixture.sends();
    ensure!(
        sends.len() == 1 && sends[0].ends_with(&format!("|send|t1|{text}|--json|")),
        "{sends:?}"
    );
    Ok(())
}

#[test]
fn stale_unknown_and_shell_targets_are_rejected_before_anything_is_typed() -> Result<()> {
    let fixture = Fixture::new()?;
    for target in ["hc:t9", "hc:t2", "hc:t3", "herdr:w2:p1", "ssh:t1", "t1"] {
        ensure!(
            fixture
                .providers
                .send(&send(target, "hello", None))
                .is_err(),
            "{target} accepted"
        );
    }
    ensure!(fixture.sends().is_empty(), "{:?}", fixture.sends());
    Ok(())
}

#[test]
fn blocked_herdr_agents_are_never_prompted() -> Result<()> {
    let fixture = Fixture::new()?;
    let error = fixture
        .providers
        .send(&send("herdr:w2:p9", "hello", None))
        .err()
        .context("blocked agent accepted")?;
    ensure!(error.to_string().contains("waiting on a dialog"), "{error}");
    ensure!(fixture.sends().is_empty());
    let receipt = fixture
        .providers
        .send(&send("herdr:w2:p88", "hello", None))?;
    ensure!(receipt.provider == "herdr" && receipt.agent == "pi");
    ensure!(fixture.sends() == vec!["herdr|agent|prompt|w2:p88|hello|".to_owned()]);
    Ok(())
}

#[test]
fn hc_places_the_file_before_naming_it_in_the_prompt() -> Result<()> {
    let fixture = Fixture::new()?;
    let staged = fixture.dir.path().join("bundle.ctx");
    fs::write(&staged, "zip")?;
    let receipt = fixture
        .providers
        .send(&send("hc:t1", "Use this context.", Some(&staged)))?;
    let text = "Use this context.\n\nAttached file: /landed/bundle.ctx";
    ensure!(receipt.file_path.as_deref() == Some("/landed/bundle.ctx"));
    ensure!(receipt.text_sha256 == sha(text));
    let sends = fixture.sends();
    ensure!(sends.len() == 2, "{sends:?}");
    ensure!(sends[0].contains(&format!("|put|t1|{}|--json|", staged.display())));
    ensure!(sends[0].ends_with("content=zip|"), "{}", sends[0]);
    ensure!(sends[1].ends_with(&format!("|send|t1|{text}|--json|")));
    let only_file = fixture
        .providers
        .send(&send("hc:t1", "  ", Some(&staged)))?;
    ensure!(only_file.text_sha256 == sha("Attached file: /landed/bundle.ctx"));
    Ok(())
}

#[test]
fn herdr_cannot_take_files_and_nothing_is_typed() -> Result<()> {
    let fixture = Fixture::new()?;
    let staged = fixture.dir.path().join("note.txt");
    fs::write(&staged, "x")?;
    ensure!(
        fixture
            .providers
            .send(&send("herdr:w2:p88", "see file", Some(&staged)))
            .is_err()
    );
    ensure!(fixture.sends().is_empty());
    Ok(())
}

#[test]
fn empty_and_oversized_prompts_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    ensure!(fixture.providers.send(&send("hc:t1", " \n", None)).is_err());
    let large = "x".repeat(xlatch_sessions::MAX_TEXT_BYTES + 1);
    ensure!(
        fixture
            .providers
            .send(&send("hc:t1", &large, None))
            .is_err()
    );
    ensure!(fixture.sends().is_empty());
    Ok(())
}

#[test]
fn provider_failures_surface_their_bounded_message() -> Result<()> {
    let fixture = Fixture::new()?;
    let error = fixture
        .providers
        .send(&send("hc:t-fail", "hello", None))
        .err()
        .context("failed send reported success")?;
    ensure!(format!("{error:#}").contains("boom"), "{error:#}");
    Ok(())
}

#[test]
fn the_binary_restores_home_under_a_cleared_environment() -> Result<()> {
    let fixture = Fixture::new()?;
    let hc = fixture.providers.hc.as_ref().context("hc")?;
    let output = Command::new(env!("CARGO_BIN_EXE_xlatch-sessions"))
        .args(["--hc", hc.to_str().context("utf-8")?, "list"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    ensure!(
        reply["sessions"].as_array().map(Vec::len) == Some(3),
        "{reply}"
    );
    let home = dirs::home_dir().context("home")?;
    ensure!(
        fixture
            .log()
            .contains(&format!("hc|HOME={}|ls|", home.display())),
        "{}",
        fixture.log()
    );
    let relative = Command::new(env!("CARGO_BIN_EXE_xlatch-sessions"))
        .args(["--hc", "hc", "list"])
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        !relative.status.success(),
        "relative provider path accepted"
    );
    Ok(())
}

#[test]
fn inline_files_are_staged_privately_under_a_safe_name() -> Result<()> {
    let fixture = Fixture::new()?;
    for (name, landed) in [
        (Some("../photo.png"), "photo.png"),
        (Some(".env"), "shared-file"),
        (None, "shared-file"),
    ] {
        let input = SendInput {
            target: "hc:t1".into(),
            text: String::new(),
            file: Some(FileRef {
                name: name.map(Into::into),
                data_base64: Some("aGk=".into()),
                ..FileRef::default()
            }),
        };
        let receipt = fixture.providers.send(&input)?;
        ensure!(receipt.file_path.as_deref() == Some(&*format!("/landed/{landed}")));
    }
    let puts: Vec<_> = fixture
        .sends()
        .into_iter()
        .filter(|s| s.contains("|put|"))
        .collect();
    ensure!(
        puts.len() == 3 && puts.iter().all(|p| p.ends_with("content=hi|")),
        "{puts:?}"
    );
    ensure!(
        puts.iter()
            .all(|p| !p.contains(&fixture.dir.path().display().to_string())),
        "inline files must not be staged in the caller's directory"
    );
    Ok(())
}

#[test]
fn rendered_manifests_bind_this_adapter_and_pass_xlatch_validation() -> Result<()> {
    let out = tempfile::tempdir()?;
    let status = Command::new(env!("CARGO_BIN_EXE_xlatch-sessions"))
        .args([
            "--hc",
            "/opt/hc/bin/hc",
            "--herdr",
            "/opt/herdr/bin/herdr",
            "manifests",
            "--out",
        ])
        .arg(out.path())
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let adapter = fs::canonicalize(env!("CARGO_BIN_EXE_xlatch-sessions"))?;
    for (id, verb) in [("sessions.list", "list"), ("sessions.send", "send")] {
        let manifest: xlatch_core::capability::Manifest =
            serde_json::from_slice(&fs::read(out.path().join(format!("{id}.json")))?)?;
        manifest.validate()?;
        manifest.validate_host_binding()?;
        let xlatch_core::capability::Execution::Command { program, args, .. } = &manifest.execution
        else {
            anyhow::bail!("{id} is not a command capability");
        };
        ensure!(Path::new(program) == adapter, "{program}");
        ensure!(
            *args
                == [
                    "--hc",
                    "/opt/hc/bin/hc",
                    "--herdr",
                    "/opt/herdr/bin/herdr",
                    verb
                ],
            "{args:?}"
        );
    }
    Ok(())
}

#[test]
fn the_send_schema_accepts_inline_and_uploaded_files_but_not_caller_paths() -> Result<()> {
    let manifest: serde_json::Value = serde_json::from_str(xlatch_sessions::TEMPLATES[1].1)?;
    let schema = jsonschema::validator_for(&manifest["input_schema"])?;
    for valid in [
        json!({"target": "hc:t1", "text": "hi", "mime_type": "text/plain"}),
        json!({"target": "hc:t1", "file": {"name": "a.ctx", "mime_type": "application/zip", "data_base64": "aGk="}}),
        json!({"target": "hc:t1", "text": "see", "file": {"artifact_id": "u1", "name": "a.ctx", "mime_type": "application/zip", "size": 2}}),
    ] {
        ensure!(schema.is_valid(&valid), "rejected {valid}");
    }
    for invalid in [
        json!({"target": "hc:t1"}),
        json!({"text": "hi"}),
        json!({"target": "hc:t1", "file": {"path": "/etc/passwd", "name": "p", "mime_type": "text/plain", "data_base64": ""}}),
        json!({"target": "hc:t1", "text": "hi", "force": true}),
    ] {
        ensure!(!schema.is_valid(&invalid), "accepted {invalid}");
    }
    Ok(())
}
