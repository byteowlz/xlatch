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
        #[command(flatten)]
        options: server::Options,
        /// Show the generated definition without changing the system.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop the service and disable automatic startup.
    Disable,
}

pub async fn dispatch(dir: PathBuf, command: Command) -> Result<()> {
    match command {
        Command::Run(options) => server::run(dir, options).await,
        Command::Enable { options, dry_run } => enable(&dir, &options, dry_run),
        command => control(&command),
    }
}

fn enable(dir: &Path, options: &server::Options, dry_run: bool) -> Result<()> {
    let executable = std::env::current_exe()?;
    let dir = std::path::absolute(dir)?;
    let args = vec![
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
        "--public-url".into(),
        options.public_url.clone(),
        "--workers".into(),
        options.workers.to_string(),
    ];
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
    if path.exists() {
        ensure!(
            std::fs::read_to_string(&path)? == definition,
            "service definition already exists with different settings: {}; disable and remove it before reconfiguring",
            path.display()
        );
    } else {
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
        let loaded = Process::new("launchctl")
            .args(["print", &format!("{domain}/com.byteowlz.xlatch")])
            .output()?
            .status
            .success();
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

fn definition(args: &[String]) -> Result<(PathBuf, String)> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    if cfg!(target_os = "linux") {
        let config = xlatch_core::paths::default_config_dir()?;
        let root = config.parent().context("missing configuration root")?;
        let command = args
            .iter()
            .map(|arg| systemd_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok((
            root.join("systemd/user/xlatch.service"),
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
        PathBuf::from(home).join("Library/LaunchAgents/com.byteowlz.xlatch.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>com.byteowlz.xlatch</string><key>ProgramArguments</key><array>{command}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>Umask</key><integer>63</integer></dict></plist>\n"
        ),
    ))
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
        return checked("systemctl", &args);
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
            checked("launchctl", &["disable", &target])?;
            let loaded = Process::new("launchctl")
                .args(["print", &target])
                .output()?
                .status
                .success();
            if loaded {
                checked("launchctl", &["bootout", &target])?;
            }
            Ok(())
        }
        _ => anyhow::bail!("expected a service control command"),
    }
}

fn domain() -> Result<String> {
    let output = Process::new("id").arg("-u").output()?;
    ensure!(output.status.success(), "cannot determine user ID");
    let uid: u32 = std::str::from_utf8(&output.stdout)?.trim().parse()?;
    Ok(format!("gui/{uid}"))
}

fn checked(program: &str, args: &[&str]) -> Result<()> {
    let status = Process::new(program)
        .args(args)
        .status()
        .with_context(|| format!("run {program}"))?;
    ensure!(
        status.success(),
        "{program} failed ({status}); install the service with xlatch service enable first"
    );
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd_quote(value: &str) -> String {
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
