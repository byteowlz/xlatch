//! OS user-service integration. Service definitions contain no device credentials.

use crate::server;
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use std::{
    path::{Path, PathBuf},
    process::Command as Process,
};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server in the foreground.
    Run(server::Options),
    /// Start an installed service.
    Start,
    /// Stop the running service without disabling automatic startup.
    Stop,
    /// Show the OS service state.
    Status,
    /// Restart an installed service.
    Restart,
    /// Install and start the service at user login.
    Enable {
        /// Unprivileged account for executing jobs in protected mode.
        #[arg(long)]
        executor_user: Option<String>,
        /// Fingerprint displayed by the approver phone for a protected migration.
        #[arg(long)]
        approver_fingerprint: Option<String>,
        #[command(flatten)]
        options: server::Options,
        /// Show the generated definition without changing the system.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop the service, disable automatic startup, and remove the definition.
    Disable,
}

pub async fn dispatch(dir: PathBuf, command: Command, protected: bool) -> Result<()> {
    if protected {
        return match command {
            Command::Enable {
                options,
                dry_run,
                executor_user,
                approver_fingerprint,
            } => crate::protected_install::enable(
                &dir,
                &options,
                executor_user
                    .as_deref()
                    .context("protected installation requires --executor-user")?,
                dry_run,
                approver_fingerprint.as_deref(),
            ),
            command => crate::protected_install::control(&command),
        };
    }
    match command {
        Command::Run(options) => server::run(dir, options).await,
        Command::Enable {
            options,
            dry_run,
            executor_user,
            approver_fingerprint,
        } => {
            ensure!(
                executor_user.is_none() && approver_fingerprint.is_none(),
                "--executor-user requires --protected"
            );
            enable(&dir, &options, dry_run)
        }
        command => control(&command),
    }
}

fn enable(dir: &Path, options: &server::Options, dry_run: bool) -> Result<()> {
    let executable = std::env::current_exe()?;
    let dir = std::path::absolute(dir)?;
    let mut args = vec![
        executable
            .to_str()
            .context("executable path is not UTF-8")?
            .to_owned(),
        "--data-dir".into(),
        dir.to_str().context("data path is not UTF-8")?.to_owned(),
        "service".into(),
        "run".into(),
        "--listen".into(),
        options.listen.to_string(),
        "--workers".into(),
        options.workers.to_string(),
    ];
    if let Some(port) = options.port {
        args.extend(["--port".to_owned(), port.to_string()]);
    }
    if let Some(origin) = &options.public_url {
        args.extend(["--public-url".to_owned(), origin.clone()]);
    }
    let (path, definition) = definition(&args)?;
    if dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"path":path,"definition":definition})
            )?
        );
        return Ok(());
    }
    let installed = path
        .exists()
        .then(|| std::fs::read_to_string(&path))
        .transpose()?;
    if installed.as_deref() != Some(definition.as_str()) {
        if installed.is_some() {
            stop_loaded()?;
        }
        std::fs::create_dir_all(path.parent().context("missing service directory")?)?;
        std::fs::write(&path, definition)?;
    }
    if cfg!(target_os = "linux") {
        checked("systemctl", &["--user", "daemon-reload"])?;
        checked(
            "systemctl",
            &["--user", "enable", "--now", "xlatch.service"],
        )?;
    } else {
        let domain = domain()?;
        checked(
            "launchctl",
            &["enable", &format!("{domain}/com.byteowlz.xlatch")],
        )?;
        let loaded = launchctl_present(&format!("{domain}/com.byteowlz.xlatch"))?;
        if !loaded {
            checked(
                "launchctl",
                &[
                    "bootstrap",
                    &domain,
                    path.to_str().context("invalid service path")?,
                ],
            )?;
        }
        checked(
            "launchctl",
            &["kickstart", &format!("{domain}/com.byteowlz.xlatch")],
        )?;
    }
    println!("CrossLatch user service enabled and started.");
    Ok(())
}

fn definition_path() -> Result<PathBuf> {
    if cfg!(target_os = "linux") {
        let config = xlatch_core::paths::default_config_dir()?;
        let root = config.parent().context("missing configuration root")?;
        return Ok(root.join("systemd/user/xlatch.service"));
    }
    ensure!(
        cfg!(target_os = "macos"),
        "user services are supported on Linux and macOS"
    );
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents/com.byteowlz.xlatch.plist"))
}

fn definition(args: &[String]) -> Result<(PathBuf, String)> {
    let path = definition_path()?;
    if cfg!(target_os = "linux") {
        let command = args
            .iter()
            .map(|arg| systemd_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok((
            path,
            format!(
                "[Unit]\nDescription=CrossLatch capability service\nAfter=network.target\n\n[Service]\nExecStart={command}\nRestart=on-failure\nUMask=0077\n\n[Install]\nWantedBy=default.target\n"
            ),
        ));
    }
    ensure!(
        cfg!(target_os = "macos"),
        "user services are supported on Linux and macOS"
    );
    let mut command = String::new();
    for arg in args {
        command.push_str("<string>");
        command.push_str(&xml_escape(arg));
        command.push_str("</string>");
    }
    Ok((
        path,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>com.byteowlz.xlatch</string><key>ProgramArguments</key><array>{command}</array><key>LimitLoadToSessionType</key><array><string>Aqua</string><string>Background</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>Umask</key><integer>63</integer></dict></plist>\n"
        ),
    ))
}

/// Stop the running user instance so a replaced definition starts fresh.
fn stop_loaded() -> Result<()> {
    if cfg!(target_os = "linux") {
        return checked("systemctl", &["--user", "stop", "xlatch.service"]);
    }
    let target = format!("{}/com.byteowlz.xlatch", domain()?);
    if launchctl_present(&target)? {
        checked("launchctl", &["bootout", &target])?;
    }
    Ok(())
}

fn remove_definition() -> Result<()> {
    let path = definition_path()?;
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(&path)?;
    if cfg!(target_os = "linux") {
        checked("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

fn control(command: &Command) -> Result<()> {
    let verb = match command {
        Command::Status => "status",
        Command::Start => "start",
        Command::Stop => "stop",
        Command::Restart => "restart",
        Command::Disable => "disable",
        _ => anyhow::bail!("expected a service control command"),
    };
    if cfg!(target_os = "linux") {
        let mut args = vec!["--user", verb];
        if matches!(command, Command::Disable) {
            args.push("--now");
        }
        args.push("xlatch.service");
        checked("systemctl", &args)?;
        if matches!(command, Command::Disable) {
            remove_definition()?;
        }
        return Ok(());
    }
    ensure!(
        cfg!(target_os = "macos"),
        "user services are supported on Linux and macOS"
    );
    let target = format!("{}/com.byteowlz.xlatch", domain()?);
    match command {
        Command::Status => checked("launchctl", &["print", &target]),
        Command::Start => checked("launchctl", &["kickstart", &target]),
        Command::Restart => checked("launchctl", &["kickstart", "-k", &target]),
        Command::Stop => checked("launchctl", &["kill", "SIGTERM", &target]),
        Command::Disable => {
            // Remove the definition even when no instance has been bootstrapped.
            if launchctl_present(&target)? {
                checked("launchctl", &["disable", &target])?;
                checked("launchctl", &["bootout", &target])?;
            }
            remove_definition()
        }
        _ => anyhow::bail!("expected a service control command"),
    }
}

fn domain() -> Result<String> {
    let output = Process::new("id").arg("-u").output()?;
    ensure!(output.status.success(), "cannot determine user ID");
    let uid: u32 = std::str::from_utf8(&output.stdout)?.trim().parse()?;
    select_domain(uid, launchctl_present)
}

fn select_domain(uid: u32, mut present: impl FnMut(&str) -> Result<bool>) -> Result<String> {
    let mut available = None;
    for domain in [format!("gui/{uid}"), format!("user/{uid}")] {
        if !present(&domain)? {
            continue;
        }
        // Keep controlling a background instance even if a GUI login appears later.
        if present(&format!("{domain}/com.byteowlz.xlatch"))? {
            return Ok(domain);
        }
        if available.is_none() {
            available = Some(domain);
        }
    }
    available.context("no accessible launchd user domain; log in as the service owner and retry")
}

fn launchctl_present(target: &str) -> Result<bool> {
    let output = Process::new("launchctl")
        .args(["print", target])
        .output()
        .context("probe launchd domain/service")?;
    match output.status.code() {
        Some(0) => Ok(true),
        // launchd's service-absent and domain-unavailable codes, respectively.
        Some(113 | 125) => Ok(false),
        _ => anyhow::bail!(
            "launchctl print {target} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn checked(program: &str, args: &[&str]) -> Result<()> {
    let status = Process::new(program)
        .args(args)
        .status()
        .with_context(|| format!("run {program}"))?;
    ensure!(
        status.success(),
        "{program} {} failed ({status})",
        args.join(" ")
    );
    Ok(())
}

pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn systemd_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

#[cfg(test)]
mod tests {
    use super::select_domain;
    use anyhow::{Result, ensure};

    #[test]
    fn unavailable_gui_uses_background_domain() -> Result<()> {
        let mut queried = Vec::new();
        let selected = select_domain(501, |target| {
            queried.push(target.to_owned());
            Ok(target == "user/501")
        })?;
        ensure!(
            selected == "user/501",
            "wrong background domain: {selected}"
        );
        ensure!(
            queried == ["gui/501", "user/501", "user/501/com.byteowlz.xlatch"],
            "unexpected probes: {queried:?}"
        );
        Ok(())
    }

    #[test]
    fn new_install_prefers_gui_when_available() -> Result<()> {
        let selected = select_domain(501, |target| Ok(!target.ends_with("/com.byteowlz.xlatch")))?;
        ensure!(selected == "gui/501", "wrong default domain: {selected}");
        Ok(())
    }

    #[test]
    fn existing_background_instance_wins_over_new_gui_login() -> Result<()> {
        let selected = select_domain(501, |target| Ok(target != "gui/501/com.byteowlz.xlatch"))?;
        ensure!(
            selected == "user/501",
            "lost existing background instance: {selected}"
        );
        Ok(())
    }

    #[test]
    fn unavailable_domains_and_probe_errors_are_not_hidden() {
        assert!(select_domain(501, |_| Ok(false)).is_err());
        let result = select_domain(501, |_| anyhow::bail!("permission denied"));
        assert!(result.is_err_and(|error| error.to_string() == "permission denied"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn agent_allows_gui_and_background_sessions() -> Result<()> {
        let (_, contents) = super::definition(&["/usr/local/bin/xlatch".into()])?;
        ensure!(contents.contains(
            "<key>LimitLoadToSessionType</key><array><string>Aqua</string><string>Background</string></array>"
        ), "session types missing from agent");
        Ok(())
    }
}
