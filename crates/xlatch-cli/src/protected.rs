//! Protected broker startup and separate executor entry points.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub service_uid: u32,
    pub executor_uid: u32,
    pub control_dir: PathBuf,
}

pub fn require_guard(dir: &Path) -> Result<()> {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join("xlatch.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let enabled: bool = conn
        .query_row(
            "SELECT enabled FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .context("enable phone enrollment approval before protected installation")?;
    ensure!(
        enabled,
        "enable phone enrollment approval in the iPhone app before protected installation"
    );
    Ok(())
}

pub fn load(path: Option<&Path>, data: &Path) -> Result<Option<Config>> {
    let Some(path) = path else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        admin_path(path)?;
        admin_path(&std::env::current_exe()?)?;
        let config: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
        let uid = nix::unistd::getuid().as_raw();
        ensure!(
            uid != 0
                && uid == config.service_uid
                && uid != config.executor_uid
                && config.executor_uid != 0,
            "protected broker and executor require distinct non-root identities"
        );
        private_path(data, uid, 0o700)?;
        private_path(&config.control_dir, uid, 0o755)?;
        Ok(Some(config))
    }
    #[cfg(not(unix))]
    {
        let _ = data;
        anyhow::bail!("protected service installation currently requires macOS or Linux");
    }
}

#[cfg(unix)]
pub fn admin_path(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(path.is_absolute(), "protected paths must be absolute");
    // Check the supplied path as well as the target: a user-owned symlink to a
    // root-owned target must not pass, because it can be retargeted after startup.
    for path in [path.to_path_buf(), std::fs::canonicalize(path)?] {
        for parent in path.ancestors() {
            let meta = std::fs::symlink_metadata(parent)?;
            ensure!(
                meta.uid() == 0 && (meta.file_type().is_symlink() || meta.mode() & 0o022 == 0),
                "protected path must be administrator-owned and not group/world writable: {}",
                parent.display()
            );
            check_acl(parent)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn private_path(path: &Path, uid: u32, mode: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == uid && meta.mode() & 0o777 == mode,
        "protected directory has incorrect ownership or permissions: {}",
        path.display()
    );
    check_acl(path)?;
    admin_path(
        path.parent()
            .context("protected directory needs an administrator-owned parent")?,
    )
}

#[cfg(unix)]
fn check_acl(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/ls")
            .args(["-lde"])
            .arg(path)
            .output()?;
        ensure!(output.status.success(), "cannot inspect protected path ACL");
        let text = String::from_utf8(output.stdout)?;
        for line in text.lines().skip(1) {
            ensure!(
                !(line.contains(" allow ")
                    && ["write", "add_file", "add_subdirectory", "delete", "chown"]
                        .iter()
                        .any(|right| line.contains(right))),
                "protected path has a writable ACL: {}",
                path.display()
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
    Ok(())
}

pub async fn run_executor(control_dir: PathBuf, work_dir: PathBuf) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = nix::unistd::getuid().as_raw();
        let owner = std::fs::metadata(&control_dir)?.uid();
        ensure!(
            uid != 0 && uid != owner,
            "executor must run as an unprivileged user distinct from the broker"
        );
        private_path(&control_dir, owner, 0o755)?;
        eprintln!(
            "xlatch executor running as UID {uid}; broker state is not accessible to this process"
        );
        xlatch_core::executor::run(control_dir, work_dir).await
    }
    #[cfg(not(unix))]
    {
        let _ = (control_dir, work_dir);
        anyhow::bail!("protected executor currently requires macOS or Linux");
    }
}

pub async fn reap_leases(dir: PathBuf) -> Result<()> {
    loop {
        xlatch_core::store::Store::open(&dir)?.expire_executor_leases()?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
