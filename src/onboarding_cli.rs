//! First-run CLI parsing lives outside the runtime dispatcher.
use anyhow::{Result, bail};
use cortex::setup::onboarding::{Role, StartOptions};
use std::{collections::BTreeMap, io::IsTerminal, path::PathBuf, time::Duration};

#[derive(Clone, PartialEq, Eq)]
pub enum Command {
    Start(StartOptions, bool),
    Effective(bool),
    Verify(StartOptions, bool),
    Backup {
        action: cortex::update::BackupAction,
        home: Option<PathBuf>,
        stamp: Option<String>,
    },
}
// Never derive Debug for options carrying secret-file values.
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupLifecycleCommand([redacted options])")
    }
}

pub fn parse(args: &[String]) -> Result<Command> {
    let name = args.first().map(String::as_str).unwrap_or("start");
    if name == "backup" {
        return parse_backup(&args[1..]);
    }
    let mut options = StartOptions {
        interactive: true,
        ..Default::default()
    };
    let mut json = false;
    let mut i = 1;
    while i < args.len() {
        let flag = &args[i];
        match flag.as_str() {
            "--json" => {
                json = true;
                options.interactive = false;
            }
            "--dry-run" if name == "start" => options.dry_run = true,
            "--backup-schedule" if name == "start" => options.backup_schedule = Some(true),
            "--no-backup-schedule" if name == "start" => options.backup_schedule = Some(false),
            "--role" if name == "start" => {
                options.role = Some(Role::parse(&value(args, &mut i, flag)?)?)
            }
            "--server" => options.server = Some(value(args, &mut i, flag)?),
            "--token-file" => options.token_file = Some(PathBuf::from(value(args, &mut i, flag)?)),
            "--api-token-file" => {
                options.api_token_file = Some(PathBuf::from(value(args, &mut i, flag)?))
            }
            "--clients" if name == "start" => {
                options.clients = Some(csv(&value(args, &mut i, flag)?))
            }
            "--capabilities" if name == "start" => {
                options.capabilities = Some(csv(&value(args, &mut i, flag)?))
            }
            "--set" if name == "start" => {
                let assignment = value(args, &mut i, flag)?;
                let (key, val) = assignment
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("--set requires KEY=VALUE"))?;
                if cortex::setup::managed_config::secret_key(key)
                    || cortex::setup::managed_config::sensitive_value(val)
                {
                    bail!("{key} is sensitive; use --secret-file KEY=PATH");
                }
                cortex::setup::managed_config::validate_setting(key, val)?;
                options.settings.insert(key.into(), val.into());
            }
            "--secret-file" if name == "start" => {
                let assignment = value(args, &mut i, flag)?;
                let (key, path) = assignment
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("--secret-file requires KEY=PATH"))?;
                cortex::setup::managed_config::validate_setting(key, "")?;
                options.settings.insert(
                    key.into(),
                    cortex::setup::onboarding::read_secret(std::path::Path::new(path))?,
                );
            }
            _ => bail!("unknown setup {name} option: {flag}"),
        };
        i += 1;
    }
    match name {
        "start" => Ok(Command::Start(options, json)),
        "verify" => Ok(Command::Verify(options, json)),
        "effective"
            if options
                == StartOptions {
                    interactive: !json,
                    ..Default::default()
                } =>
        {
            Ok(Command::Effective(json))
        }
        _ => bail!("invalid setup lifecycle command {name}"),
    }
}
fn value(args: &[String], i: &mut usize, flag: &str) -> Result<String> {
    *i += 1;
    args.get(*i)
        .filter(|v| !v.starts_with("--"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
}
fn csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != "none")
        .map(str::to_string)
        .collect()
}

fn parse_backup(args: &[String]) -> Result<Command> {
    use cortex::update::BackupAction;
    let (action, mut i) = match args.first().map(String::as_str) {
        Some("create") => (BackupAction::Create, 1),
        Some("schedule") => match args.get(1).map(String::as_str) {
            Some("install") => (BackupAction::ScheduleInstall, 2),
            Some("check") => (BackupAction::ScheduleCheck, 2),
            Some("remove") => (BackupAction::ScheduleRemove, 2),
            _ => bail!("backup schedule requires install|check|remove"),
        },
        Some("restore") => (BackupAction::Restore, 1),
        Some("rollback") => (BackupAction::Rollback, 1),
        _ => bail!("backup requires create|schedule|restore|rollback"),
    };
    let destructive = matches!(action, BackupAction::Restore | BackupAction::Rollback);
    let mut stamp = None;
    let mut home = None;
    let mut yes = false;
    while i < args.len() {
        match args[i].as_str() {
            "--home" => home = Some(PathBuf::from(value(args, &mut i, "--home")?)),
            "--yes" => yes = true,
            v if destructive && !v.starts_with('-') && stamp.is_none() => stamp = Some(v.into()),
            v => bail!("unknown backup argument {v}"),
        };
        i += 1;
    }
    if destructive && (!yes || stamp.is_none()) {
        bail!(
            "restore/rollback requires STAMP --yes; it stops the service and restores the database, keys, config and image"
        );
    }
    Ok(Command::Backup {
        action,
        home,
        stamp,
    })
}

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Start(mut options, json) => {
            options.interactive = options.interactive && std::io::stdin().is_terminal();
            let report = cortex::setup::onboarding::run_start(options).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Cortex {:?}: {}{}",
                    report.role,
                    report.server,
                    if report.dry_run {
                        " (preview only)"
                    } else {
                        ""
                    }
                );
                for s in &report.effective {
                    println!("{}={} ({})", s.key, s.value, s.origin);
                }
                for c in &report.capabilities {
                    println!("{}: {:?} — {}", c.id, c.status, c.detail);
                }
                for p in &report.phases {
                    println!("{:?}\t{}\t{}", p.status, p.name, p.detail);
                }
                for p in &report.client_paths {
                    println!("MCP config: {} (reload client)", p.display());
                }
            }
            if report.has_errors {
                bail!(
                    "setup has failed phases; retained state can be repaired by rerunning setup start"
                );
            }
        }
        Command::Effective(json) => {
            let entries = cortex::setup::managed_config::effective_environment(
                &cortex::setup::cortex_home_dir()?,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for s in entries {
                    println!("{}={} ({})", s.key, s.value, s.origin);
                }
            }
        }
        Command::Verify(options, json) => {
            let path = cortex::setup::cortex_home_dir()?.join(".env");
            let raw = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.into()),
            };
            let mut env: BTreeMap<String, String> = cortex::setup::dotenv::parse(&raw);
            for key in cortex::setup::managed_config::supported_env_keys() {
                if let Some(v) = cortex::config::config_env_var(&key) {
                    env.insert(key, v);
                }
            }
            if let Some(p) = &options.token_file {
                env.insert(
                    "CORTEX_TOKEN".into(),
                    cortex::setup::onboarding::read_secret(p)?,
                );
            }
            if let Some(p) = options.api_token_file {
                env.insert(
                    "CORTEX_API_TOKEN".into(),
                    cortex::setup::onboarding::read_secret(&p)?,
                );
            }
            let url = options
                .server
                .or_else(|| env.get("CORTEX_SERVER_URL").cloned())
                .unwrap_or_else(|| {
                    format!(
                        "http://127.0.0.1:{}",
                        env.get("CORTEX_PORT").map(String::as_str).unwrap_or("3100")
                    )
                });
            let oauth = env.get("CORTEX_AUTH_MODE").is_some_and(|v| v == "oauth");
            let mcp_token = if oauth
                && env
                    .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
                    .is_none_or(|v| v != "false")
                && options.token_file.is_none()
            {
                None
            } else {
                env.get("CORTEX_TOKEN").map(String::as_str)
            };
            let _overlay = cortex::config::PluginEnvGuard::install(
                env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            );
            let phases = cortex::setup::verification::verify_connection(
                &url,
                mcp_token,
                env.get("CORTEX_API_TOKEN").map(String::as_str),
                oauth,
                Duration::from_secs(10),
            )
            .await;
            if json {
                println!("{}", serde_json::to_string_pretty(&phases)?);
            } else {
                for p in &phases {
                    println!("{:?}\t{}\t{}", p.status, p.name, p.detail);
                }
            }
            if phases.iter().any(|p| {
                p.status == cortex::setup::SetupStatus::Error
                    || p.status == cortex::setup::SetupStatus::Warn
            }) {
                bail!("connection is not fully verified; resolve the reported auth/login phase");
            }
        }
        Command::Backup {
            action,
            home,
            stamp,
        } => println!(
            "{}",
            cortex::update::run_backup_action(action, home.as_deref(), stamp.as_deref())?
        ),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|v| v.to_string()).collect()
    }
    #[test]
    fn parser_prevents_secret_arguments_and_unsafe_restore() {
        assert!(parse(&args(&["start", "--set", "CORTEX_TOKEN=secret"])).is_err());
        assert!(parse(&args(&["backup", "restore", "stamp"])).is_err());
        assert!(parse(&args(&["backup", "rollback", "stamp", "--yes"])).is_ok());
        assert!(parse(&args(&["effective", "--server", "http://example.test"])).is_err());
    }
    #[test]
    fn unattended_roles_capabilities_and_preview_parse() {
        let Command::Start(options, json) = parse(&args(&[
            "start",
            "--role",
            "client",
            "--server",
            "https://example.test",
            "--clients",
            "codex,gemini",
            "--dry-run",
            "--json",
        ]))
        .unwrap() else {
            panic!()
        };
        assert_eq!(options.role, Some(Role::Client));
        assert!(options.dry_run && json && !options.interactive);
        assert_eq!(options.clients.unwrap().len(), 2);
    }
}
