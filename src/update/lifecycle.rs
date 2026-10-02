use std::io::{self, Write};
use std::path::Path;

pub(crate) const MANAGED_LIFECYCLE: &str = include_str!("../../scripts/managed-lifecycle.sh");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupAction {
    Create,
    ScheduleInstall,
    ScheduleCheck,
    ScheduleRemove,
    Restore,
    Rollback,
}

impl BackupAction {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::ScheduleInstall => "schedule-install",
            Self::ScheduleCheck => "schedule-check",
            Self::ScheduleRemove => "schedule-remove",
            Self::Restore => "restore",
            Self::Rollback => "rollback",
        }
    }
}

/// Install a private, self-contained helper. It never depends on a Git checkout.
fn install_helper(home: &Path) -> io::Result<()> {
    let path = home.join("managed-lifecycle.sh");
    let mut temporary = tempfile::NamedTempFile::new_in(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    temporary.write_all(MANAGED_LIFECYCLE.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn run_action(action: &str, home: &Path, stamp: Option<&str>) -> io::Result<String> {
    run_action_locked(action, home, stamp, None)
}
fn run_action_locked(
    action: &str,
    home: &Path,
    stamp: Option<&str>,
    token: Option<&str>,
) -> io::Result<String> {
    if cfg!(windows) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "managed Compose recovery currently requires a POSIX host shell; run this command on the Linux server or in WSL. Windows agent collection remains supported",
        ));
    }
    super::validate_remote_home(&home.to_string_lossy())?;
    if !home.join("compose/docker-compose.yml").is_file() || !home.join(".env").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "managed Compose deployment is missing",
        ));
    }
    if action != "schedule-check" {
        install_helper(home)?;
    }
    let binary = std::env::current_exe()?;
    // Reject development executables when scheduling. They can disappear during
    // build-cache cleanup and do not establish a managed installation.
    if action == "schedule-install"
        && binary.components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some(".cache" | "target" | "deps")
            )
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "backup schedules require the installed Cortex binary; this executable is a development build",
        ));
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    let script = format!(
        "export CORTEX_BIN={}\nset -- {} {} {}\n{}",
        quote(&binary.to_string_lossy()),
        quote(action),
        quote(&home.to_string_lossy()),
        quote(stamp.unwrap_or_default()),
        MANAGED_LIFECYCLE
    );
    let saved = crate::setup::parse_env(&std::fs::read_to_string(home.join(".env"))?);
    let policy = ["CORTEX_BACKUP_RETAIN_COUNT", "CORTEX_BACKUP_MIN_FREE_MB"]
        .into_iter()
        .filter_map(|key| {
            crate::env::var(key)
                .ok()
                .or_else(|| saved.get(key).cloned())
                .map(|value| format!("export {key}={}\n", quote(&value)))
        })
        .collect::<String>();
    let script = format!("{policy}{script}");
    let script = match token {
        Some(token) => format!("export CORTEX_LIFECYCLE_TOKEN={}\n{script}", quote(token)),
        None => script,
    };
    crate::deploy::run_local_script(&script)
}

pub fn run_backup_action(
    action: BackupAction,
    home: Option<&Path>,
    stamp: Option<&str>,
) -> io::Result<String> {
    let home = home
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(crate::setup::cortex_home_dir)?;
    if matches!(action, BackupAction::Restore | BackupAction::Rollback)
        && !stamp.is_some_and(|value| {
            !value.is_empty()
                && value != "."
                && value != ".."
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "restore and rollback require a safe recovery timestamp",
        ));
    }
    let lock = if matches!(action, BackupAction::Restore | BackupAction::Rollback) {
        Some(crate::deploy::lifecycle_lock::LocalLifecycleLock::acquire(
            &home,
        )?)
    } else {
        None
    };
    let applied = run_action_locked(
        action.name(),
        &home,
        stamp,
        lock.as_ref().map(|lock| lock.token()),
    )?;
    if matches!(action, BackupAction::Restore | BackupAction::Rollback) {
        let phases = crate::deploy::verify_local_managed_server(&home).map_err(|error| io::Error::other(format!("{applied}; restored state was applied, but verification failed: {error}. No automatic database rollback was attempted")))?;
        if let Some(failed) = phases
            .iter()
            .find(|phase| matches!(phase.status, crate::setup::SetupStatus::Error))
        {
            return Err(io::Error::other(format!(
                "{applied}; restored state was applied, but {} failed: {}. No automatic database rollback was attempted",
                failed.name, failed.detail
            )));
        }
        let warnings = phases
            .iter()
            .filter(|phase| matches!(phase.status, crate::setup::SetupStatus::Warn))
            .map(|phase| phase.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Ok(format!(
            "{applied}; readiness and authenticated server verification passed{}",
            if warnings.is_empty() {
                String::new()
            } else {
                format!("; {warnings}")
            }
        ));
    }
    Ok(applied)
}

pub fn prepare_local_upgrade(home: &Path) -> io::Result<String> {
    run_action("prepare", home, None)
}

pub(crate) fn prepare_local_upgrade_locked(home: &Path, token: &str) -> io::Result<String> {
    run_action_locked("prepare", home, None, Some(token))
}
