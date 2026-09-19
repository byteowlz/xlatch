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
pub use xlatch_core::host_trust::admin_path;
#[cfg(unix)]
use xlatch_core::host_trust::check_acl;

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
