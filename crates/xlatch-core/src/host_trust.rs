//! Administrator-owned executable paths for protected execution.
use anyhow::{Result, ensure};
use std::path::Path;

/// Require administrator ownership and non-writable ancestors, including symlink entries.
/// # Errors
/// Rejects paths that unprivileged users can replace or modify.
#[cfg(unix)]
pub fn admin_path(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(path.is_absolute(), "protected paths must be absolute");
    ensure!(
        !path.as_os_str().as_encoded_bytes().contains(&b'\n'),
        "protected paths cannot contain newlines"
    );
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

/// Inspect extended permissions in addition to POSIX mode bits.
/// # Errors
/// Rejects writable macOS ACLs or an unsuccessful ACL inspection.
#[cfg(unix)]
pub fn check_acl(path: &Path) -> Result<()> {
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

/// Validate command and shebang interpreter in the administrator trust domain.
/// This pins entry points, not all dynamically loaded code or executor behavior.
/// # Errors
/// Rejects mutable programs, environment-selected interpreters and noncanonical paths.
#[cfg(unix)]
pub fn command(program: &str) -> Result<()> {
    use anyhow::Context;
    use std::io::Read;
    let path = Path::new(program);
    admin_path(path)?;
    ensure!(
        std::fs::metadata(path)?.is_file(),
        "protected program must be a regular file"
    );
    let mut prefix = [0_u8; 512];
    let count = std::fs::File::open(path)?.read(&mut prefix)?;
    if prefix[..count].starts_with(b"#!") {
        let end = prefix[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .context("shebang is too long")?;
        let line = std::str::from_utf8(&prefix[2..end])?;
        let interpreter = line
            .split_whitespace()
            .next()
            .context("missing shebang interpreter")?;
        ensure!(
            Path::new(interpreter).file_name() != Some(std::ffi::OsStr::new("env")),
            "protected scripts need an explicit interpreter; /usr/bin/env is not an execution binding"
        );
        admin_path(Path::new(interpreter))?;
        ensure!(
            std::fs::metadata(interpreter)?.is_file(),
            "interpreter must be a regular file"
        );
    }
    Ok(())
}
