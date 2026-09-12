//! Explicit administrator installation. Dry-run produces the full reviewable plan.

use crate::{protected, server, service_install};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const BROKER: &str = "com.byteowlz.xlatch.protected";
const EXECUTOR: &str = "com.byteowlz.xlatch.executor";

fn root() -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        Ok(PathBuf::from("/Library/Application Support/xlatch"))
    } else if cfg!(target_os = "linux") {
        Ok(PathBuf::from("/var/lib/xlatch"))
    } else {
        anyhow::bail!("protected installation supports macOS and Linux");
    }
}

fn definition_path(label: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from(format!("/Library/LaunchDaemons/{label}.plist"))
    } else {
        PathBuf::from(format!("/etc/systemd/system/{label}.service"))
    }
}

#[cfg(unix)]
struct Plan {
    root: PathBuf,
    executor: nix::unistd::User,
    service_uid: u32,
    broker_definition: String,
    executor_definition: String,
    config: String,
    work_dir: PathBuf,
}

#[cfg(unix)]
fn plan(source: &Path, options: &server::Options, executor: &str) -> Result<Plan> {
    use nix::unistd::{Gid, Group, Uid, User};
    ensure!(
        options.protected_config.is_none(),
        "installer generates its own protected configuration"
    );
    ensure!(
        executor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid executor account name"
    );
    let executor = User::from_name(executor)?.context("executor account does not exist")?;
    ensure!(
        executor.uid.as_raw() != 0 && executor.name != "_xlatch",
        "select your ordinary non-root user as executor"
    );
    ensure!(
        User::from_name("_xlatch")?.is_none() && Group::from_name("_xlatch")?.is_none(),
        "_xlatch account already exists; review existing installation before replacing it"
    );
    let mut service_uid = None;
    for candidate in 300..500 {
        if User::from_uid(Uid::from_raw(candidate))?.is_none()
            && Group::from_gid(Gid::from_raw(candidate))?.is_none()
        {
            service_uid = Some(candidate);
            break;
        }
    }
    let service_uid = service_uid.context("no unused service UID/GID in 300..500")?;
    let root = root()?;
    ensure!(
        !root.exists(),
        "protected installation already exists; use service start --protected"
    );
    protected::admin_path(root.parent().context("missing install parent")?)?;
    let binary = root.join("xlatch");
    let data = root.join("data");
    let control_dir = root.join("run");
    let config_path = root.join("protected.toml");
    let config = toml::to_string_pretty(&protected::Config {
        service_uid,
        executor_uid: executor.uid.as_raw(),
        control_dir: control_dir.clone(),
    })?;
    let mut listen = options.listen;
    if let Some(port) = options.port {
        listen.set_port(port);
    }
    let mut args = vec![
        path_string(&binary)?,
        "--data-dir".into(),
        path_string(&data)?,
        "service".into(),
        "run".into(),
        "--listen".into(),
        listen.to_string(),
        "--protected-config".into(),
        path_string(&config_path)?,
    ];
    if let Some(url) = &options.public_url {
        args.extend(["--public-url".into(), url.clone()]);
    }
    let broker_definition = definition(BROKER, "_xlatch", &args, &data)?;
    let work_dir = executor.dir.join(".local/share/xlatch/executor");
    ensure!(
        !source.starts_with(&root),
        "source must be the existing user service data"
    );
    let args = vec![
        path_string(&binary)?,
        "--control-dir".into(),
        path_string(&control_dir)?,
        "executor".into(),
        "--work-dir".into(),
        path_string(&work_dir)?,
    ];
    let executor_definition = definition(EXECUTOR, &executor.name, &args, &executor.dir)?;
    Ok(Plan {
        root,
        executor,
        service_uid,
        broker_definition,
        executor_definition,
        config,
        work_dir,
    })
}

fn path_string(path: &Path) -> Result<String> {
    Ok(path.to_str().context("path is not UTF-8")?.into())
}

fn definition(label: &str, user: &str, args: &[String], home: &Path) -> Result<String> {
    if cfg!(target_os = "macos") {
        let mut arguments = String::new();
        for arg in args {
            arguments.push_str("<string>");
            arguments.push_str(&service_install::xml_escape(arg));
            arguments.push_str("</string>");
        }
        let args = arguments;
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>UserName</key><string>{user}</string><key>ProgramArguments</key><array>{args}</array><key>EnvironmentVariables</key><dict><key>HOME</key><string>{}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer><key>Umask</key><integer>63</integer></dict></plist>\n",
            service_install::xml_escape(&path_string(home)?)
        ))
    } else {
        let args = args
            .iter()
            .map(|s| service_install::systemd_quote(s))
            .collect::<Vec<_>>()
            .join(" ");
        let restrictions = if label == BROKER {
            format!(
                "ProtectSystem=strict\nReadWritePaths={} {}\n",
                service_install::systemd_quote(&path_string(home)?),
                service_install::systemd_quote(&path_string(&root()?.join("run"))?)
            )
        } else {
            String::new()
        };
        Ok(format!(
            "[Unit]\nDescription=xlatch {label}\nAfter=network.target\n\n[Service]\nUser={user}\nExecStart={args}\nEnvironment=HOME={}\nRestart=on-failure\nRestartSec=5\nUMask=0077\nNoNewPrivileges=true\n{restrictions}\n[Install]\nWantedBy=multi-user.target\n",
            service_install::systemd_quote(&path_string(home)?)
        ))
    }
}

pub fn enable(
    source: &Path,
    options: &server::Options,
    executor: &str,
    dry_run: bool,
    approver_fingerprint: Option<&str>,
) -> Result<()> {
    #[cfg(unix)]
    {
        let plan = plan(source, options, executor)?;
        if dry_run {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "source_data":source,"source_binary":std::env::current_exe()?,"install_directory":plan.root,
                    "service_account":"_xlatch","service_uid":plan.service_uid,"executor_user":plan.executor.name,
                    "executor_work_directory":plan.work_dir,"configuration":plan.config,
                    "broker_definition":{"path":definition_path(BROKER),"contents":plan.broker_definition},
                    "executor_definition":{"path":definition_path(EXECUTOR),"contents":plan.executor_definition},
                    "requirements":["Enable phone enrollment approval first","Stop the user daemon and finish outstanding jobs","Run this installation with administrator privileges","Review the copied binary and existing trusted state","Supply --approver-fingerprint matching the value displayed on the phone"],
                    "effects":["Create a dedicated non-login account","Copy the SQLite snapshot; generate a fresh TLS key inaccessible to the old user service","Install administrator-owned executable, configuration and two OS services","Start the approval broker and unprivileged executor","Retain only the verified approver phone; other devices must pair and be approved again","Scan the new server identity QR on the approver phone to trust the fresh TLS key"],
                    "limitations":["Capability activation and grant changes are unavailable through the protected local socket until phone approval for those operations ships","Windows protected installation is not implemented"]
                }))?
            );
            return Ok(());
        }
        install(
            source,
            &plan,
            approver_fingerprint.context(
                "--approver-fingerprint from the phone is required for protected installation",
            )?,
        )?;
        println!(
            "Protected xlatch installed. Local proposals use: xlatch --control-dir \"{}\" list",
            plan.root.join("run").display()
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (source, options, executor, dry_run, approver_fingerprint);
        anyhow::bail!("protected installation supports macOS and Linux");
    }
}

#[cfg(unix)]
fn install(source: &Path, plan: &Plan, fingerprint: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        nix::unistd::geteuid().is_root(),
        "review --dry-run, then run the protected installation with sudo"
    );
    protected::require_guard(source)?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(source.join("daemon.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("stop the user daemon before protected installation")?;
    let conn = rusqlite::Connection::open_with_flags(
        source.join("xlatch.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let outstanding: i64 = conn.query_row(
        "SELECT count(*) FROM jobs WHERE status IN ('queued','running')",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        outstanding == 0,
        "finish or cancel outstanding jobs before migration"
    );
    for label in [BROKER, EXECUTOR] {
        ensure!(
            !definition_path(label).exists(),
            "service definition already exists; refusing to overwrite"
        );
    }
    let binary = std::fs::read(std::env::current_exe()?)?;
    std::fs::create_dir(&plan.root)?;
    std::fs::set_permissions(&plan.root, std::fs::Permissions::from_mode(0o755))?;
    write(&plan.root.join("xlatch"), &binary, 0o755)?;
    write(
        &plan.root.join("protected.toml"),
        plan.config.as_bytes(),
        0o644,
    )?;
    let data = plan.root.join("data");
    std::fs::create_dir(&data)?;
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
    conn.execute(
        "VACUUM INTO ?1",
        [path_string(&data.join("xlatch.sqlite3"))?],
    )?;
    let mut imported = xlatch_core::store::Store::initialize(&data)?;
    imported.prepare_protected_migration(fingerprint)?;
    drop(imported);
    create_account(plan)?;
    own(&data.join("xlatch.sqlite3"), plan.service_uid, 0o600)?;
    own(&data, plan.service_uid, 0o700)?;
    let run = plan.root.join("run");
    std::fs::create_dir(&run)?;
    own(&run, plan.service_uid, 0o755)?;
    write(
        &definition_path(BROKER),
        plan.broker_definition.as_bytes(),
        0o644,
    )?;
    write(
        &definition_path(EXECUTOR),
        plan.executor_definition.as_bytes(),
        0o644,
    )?;
    start_installed()
}

#[cfg(unix)]
fn own(path: &Path, uid: u32, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(uid)),
    )?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(unix)]
fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(path)?;
    file.write_all(bytes)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn create_account(plan: &Plan) -> Result<()> {
    let uid = plan.service_uid.to_string();
    let home = path_string(&plan.root.join("data"))?;
    if cfg!(target_os = "macos") {
        run("/usr/bin/dscl", &[".", "-create", "/Groups/_xlatch"])?;
        run(
            "/usr/bin/dscl",
            &[".", "-create", "/Groups/_xlatch", "PrimaryGroupID", &uid],
        )?;
        run("/usr/bin/dscl", &[".", "-create", "/Users/_xlatch"])?;
        for (key, value) in [
            ("UniqueID", uid.as_str()),
            ("PrimaryGroupID", uid.as_str()),
            ("NFSHomeDirectory", home.as_str()),
            ("UserShell", "/usr/bin/false"),
            ("IsHidden", "1"),
            ("AuthenticationAuthority", ";DisabledUser;"),
        ] {
            run(
                "/usr/bin/dscl",
                &[".", "-create", "/Users/_xlatch", key, value],
            )?;
        }
    } else {
        run(
            "/usr/sbin/groupadd",
            &["--system", "--gid", &uid, "_xlatch"],
        )?;
        run(
            "/usr/sbin/useradd",
            &[
                "--system",
                "--uid",
                &uid,
                "--gid",
                &uid,
                "--home-dir",
                &home,
                "--shell",
                "/usr/sbin/nologin",
                "_xlatch",
            ],
        )?;
    }
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    ensure!(
        Command::new(program).args(args).status()?.success(),
        "{program} failed; inspect the partial installation before retrying"
    );
    Ok(())
}

fn start_installed() -> Result<()> {
    if cfg!(target_os = "macos") {
        for label in [BROKER, EXECUTOR] {
            run("/bin/launchctl", &["enable", &format!("system/{label}")])?;
            run(
                "/bin/launchctl",
                &[
                    "bootstrap",
                    "system",
                    &path_string(&definition_path(label))?,
                ],
            )?;
        }
    } else {
        run("/usr/bin/systemctl", &["daemon-reload"])?;
        run("/usr/bin/systemctl", &["enable", "--now", BROKER, EXECUTOR])?;
    }
    Ok(())
}

pub fn control(command: &service_install::Command) -> Result<()> {
    use service_install::Command as Service;
    let verb = match command {
        Service::Start => "start",
        Service::Stop => "stop",
        Service::Restart => "restart",
        Service::Status => "status",
        Service::Disable => "disable",
        _ => anyhow::bail!("protected run is managed by the installed system service"),
    };
    if cfg!(target_os = "linux") {
        return if matches!(command, Service::Disable) {
            run("/usr/bin/systemctl", &[verb, "--now", BROKER, EXECUTOR])
        } else {
            run("/usr/bin/systemctl", &[verb, BROKER, EXECUTOR])
        };
    }
    ensure!(
        cfg!(target_os = "macos"),
        "protected services require macOS or Linux"
    );
    for label in [BROKER, EXECUTOR] {
        let target = format!("system/{label}");
        match command {
            Service::Status => run("/bin/launchctl", &["print", &target])?,
            Service::Restart => run("/bin/launchctl", &["kickstart", "-k", &target])?,
            Service::Start => {
                run("/bin/launchctl", &["enable", &target])?;
                if Command::new("/bin/launchctl")
                    .args(["print", &target])
                    .output()?
                    .status
                    .success()
                {
                    run("/bin/launchctl", &["kickstart", &target])?;
                } else {
                    run(
                        "/bin/launchctl",
                        &[
                            "bootstrap",
                            "system",
                            &path_string(&definition_path(label))?,
                        ],
                    )?;
                }
            }
            Service::Stop | Service::Disable => {
                if matches!(command, Service::Disable) {
                    run("/bin/launchctl", &["disable", &target])?;
                }
                run("/bin/launchctl", &["bootout", &target])?;
            }
            _ => anyhow::bail!("unsupported protected service operation"),
        }
    }
    Ok(())
}
