//! Exercise service lifecycle commands without touching the host's launchd state.
#![cfg(target_os = "macos")]

use anyhow::{Result, ensure};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("xlatch-service-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("bin"))?;
        let fixture = Self(root);
        let launchctl = fixture.0.join("bin/launchctl");
        fs::write(
            &launchctl,
            r#"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$XLATCH_TEST_DIR/commands"
case "$1" in
    print)
        case "$2" in
            gui/*) exit 125 ;;
            user/*/*) test -f "$XLATCH_TEST_DIR/loaded" || exit 113 ;;
            user/*) exit 0 ;;
        esac
        printf 'state = running\n'
        ;;
    bootstrap) touch "$XLATCH_TEST_DIR/loaded" ;;
    bootout)
        test "$XLATCH_TEST_BOOTOUT_FAIL" = 0 || exit 5
        rm "$XLATCH_TEST_DIR/loaded"
        ;;
    enable|disable|kickstart) ;;
    *) exit 64 ;;
esac
"#,
        )?;
        fs::set_permissions(launchctl, fs::Permissions::from_mode(0o755))?;
        Ok(fixture)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xlatch"));
        command
            .args(["--data-dir", self.0.join("data").to_string_lossy().as_ref()])
            .args(args)
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XLATCH_TEST_DIR", &self.0)
            .env("XLATCH_TEST_BOOTOUT_FAIL", "0")
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.0.join("bin").display()),
            );
        command
    }

    fn run(&self, args: &[&str]) -> Result<()> {
        let output = self.command(args).output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    fn definition(&self) -> PathBuf {
        self.0
            .join("Library/LaunchAgents/com.byteowlz.xlatch.plist")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn background_enable_reconfigure_and_disable() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.run(&["service", "enable", "--port", "7898", "--dry-run"])?;
    ensure!(!fixture.definition().exists(), "dry-run wrote a definition");
    ensure!(
        !fixture.0.join("commands").exists(),
        "dry-run ran launchctl"
    );

    fixture.run(&["service", "enable", "--port", "7898"])?;
    fixture.run(&["service", "status"])?;
    ensure!(
        fs::read_to_string(fixture.definition())?.contains("<string>7898</string>"),
        "initial port missing"
    );

    fs::write(fixture.0.join("commands"), "")?;
    fixture.run(&["service", "enable", "--port", "7899"])?;
    let commands = fs::read_to_string(fixture.0.join("commands"))?;
    let unload = commands
        .find("bootout user/")
        .ok_or_else(|| anyhow::anyhow!("missing bootout"))?;
    let load = commands
        .find("bootstrap user/")
        .ok_or_else(|| anyhow::anyhow!("missing bootstrap"))?;
    ensure!(
        unload < load,
        "reconfiguration bootstrapped before unloading"
    );
    ensure!(
        fs::read_to_string(fixture.definition())?.contains("<string>7899</string>"),
        "new port missing"
    );

    fixture.run(&["service", "disable"])?;
    ensure!(
        !fixture.definition().exists(),
        "disable kept the definition"
    );
    ensure!(
        !fixture.0.join("loaded").exists(),
        "disable kept the instance"
    );
    fixture.run(&["service", "disable"])?;

    fs::create_dir_all(
        fixture
            .definition()
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing parent"))?,
    )?;
    fs::write(fixture.definition(), "stale unloaded definition")?;
    fixture.run(&["service", "disable"])?;
    ensure!(
        !fixture.definition().exists(),
        "disable kept an unloaded definition"
    );
    Ok(())
}

#[test]
fn failed_unload_preserves_existing_definition() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.run(&["service", "enable", "--port", "7898"])?;
    let original = fs::read_to_string(fixture.definition())?;
    let output = fixture
        .command(&["service", "enable", "--port", "7899"])
        .env("XLATCH_TEST_BOOTOUT_FAIL", "1")
        .output()?;
    ensure!(!output.status.success(), "unload failure was hidden");
    ensure!(
        fs::read_to_string(fixture.definition())? == original,
        "failed unload replaced definition"
    );
    ensure!(
        fixture.0.join("loaded").exists(),
        "failed unload lost instance"
    );
    Ok(())
}
