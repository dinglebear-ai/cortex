use std::collections::BTreeMap;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use super::firstrun;
use super::{SetupPhase, setup_path_value};

pub(crate) struct EnvResult {
    pub(crate) phase: SetupPhase,
    pub(crate) values: BTreeMap<String, String>,
}

/// Infer `<user>/.cortex` from the executable path when the binary lives
/// under `/home/<user>/...` but `$HOME` points elsewhere (sudo, systemd).
///
/// Only a filesystem-root `/home` (or ostree-style `/var/home`) qualifies —
/// matching ANY ancestor literally named `home` also captured layouts like
/// `/opt/home/svc/bin` or `/tmp/home/x` and silently redirected config
/// resolution (full-review QM6).
pub(crate) fn cortex_home_dir_from_exe_path(exe: &Path) -> Option<PathBuf> {
    for ancestor in exe.ancestors() {
        let Some(parent) = ancestor.parent() else {
            continue;
        };
        if parent == Path::new("/home") || parent == Path::new("/var/home") {
            return Some(ancestor.join(".cortex"));
        }
    }
    None
}

pub fn cortex_home_dir() -> io::Result<PathBuf> {
    if let Ok(value) = std::env::var("CORTEX_HOME") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return validate_absolute_home(PathBuf::from(trimmed));
        }
    }
    let home =
        std::env::var("HOME").map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME is unset"))?;
    let home_candidate = PathBuf::from(home).join(".cortex");
    if home_candidate.join(".env").is_file() || home_candidate.is_dir() {
        return validate_absolute_home(home_candidate);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_candidate) = cortex_home_dir_from_exe_path(&exe)
        && (exe_candidate.join(".env").is_file() || exe_candidate.is_dir())
    {
        tracing::debug!(
            candidate = %exe_candidate.display(),
            "cortex_home_dir: using exe-derived home (HOME candidate absent)"
        );
        return validate_absolute_home(exe_candidate);
    }
    validate_absolute_home(home_candidate)
}

/// Default agent-command spool path (`~/.local/state/cortex/agent-command.jsonl`),
/// matching what `cortex setup shell agent install` wires the wrapper script
/// to write to. Used as the default for `cortex heartbeat agent
/// --agent-command-forward`'s spool source.
pub fn default_agent_command_spool_path() -> io::Result<PathBuf> {
    Ok(user_home_dir()?
        .join(".local/state/cortex")
        .join("agent-command.jsonl"))
}

pub(crate) fn user_home_dir() -> io::Result<PathBuf> {
    let home =
        std::env::var("HOME").map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME is unset"))?;
    let home = PathBuf::from(home);
    setup_path_value(&home)?;
    Ok(home)
}

fn validate_absolute_home(path: PathBuf) -> io::Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "setup home must be an absolute path without '..': {}",
                path.display()
            ),
        ));
    }
    Ok(path)
}

pub(crate) fn resolve_cortex_binary() -> io::Result<PathBuf> {
    #[cfg(windows)]
    const BIN: &str = "cortex.exe";
    #[cfg(not(windows))]
    const BIN: &str = "cortex";

    // Check same directory as the running binary first (co-installed).
    let current = std::env::current_exe()?;
    if let Some(dir) = current.parent() {
        let candidate = dir.join(BIN);
        if candidate.is_file() {
            return validate_executable_path(candidate);
        }
    }
    // Walk PATH without spawning a shell — works on Windows and Unix.
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(BIN);
            if candidate.is_file() {
                return validate_executable_path(candidate);
            }
        }
    }
    // Fall back to the running executable if it is named cortex.
    if current
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == "cortex" || n == "cortex.exe")
        .unwrap_or(false)
    {
        return validate_executable_path(current);
    }
    Err(io::Error::new(
        ErrorKind::NotFound,
        "cortex binary not found on PATH",
    ))
}

pub(crate) fn validate_executable_path(path: PathBuf) -> io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    setup_path_value(&canonical)?;
    if !allow_debug_binary() && looks_like_debug_build_path(&canonical) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "refusing to install AI watch service with debug/worktree binary {}; put the cortex wrapper on PATH or set CORTEX_AI_WATCH_ALLOW_DEBUG_BINARY=true",
                canonical.display()
            ),
        ));
    }
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("not a file: {}", canonical.display()),
        ));
    }
    Ok(canonical)
}

fn allow_debug_binary() -> bool {
    std::env::var("CORTEX_AI_WATCH_ALLOW_DEBUG_BINARY")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn looks_like_debug_build_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/target/debug/") || text.contains("/.cache/cargo/debug/")
}

pub(crate) fn resolve_ai_watch_db_path(setup_home: &Path, user_home: &Path) -> io::Result<PathBuf> {
    if let Ok(value) = std::env::var("CORTEX_DB_PATH")
        && !value.trim().is_empty()
    {
        return validate_db_path(PathBuf::from(value));
    }
    if let Some(path) = db_path_from_setup_env(&setup_home.join(".env"))? {
        return validate_db_path(path);
    }
    let plugin_db = user_home.join(".claude/plugins/data/syslog-jmagar-lab/cortex.db");
    if plugin_db.exists() {
        return validate_db_path(plugin_db);
    }
    validate_db_path(setup_home.join("data/cortex.db"))
}

pub(crate) fn validate_db_path(path: PathBuf) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("AI watch DB path must be absolute: {}", path.display()),
        ));
    }
    setup_path_value(&path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let Some(parent) = parent.filter(|parent| *parent != Path::new("/")) else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "AI watch DB path must live under a non-root directory: {}",
                path.display()
            ),
        ));
    };
    std::fs::create_dir_all(parent)?;
    Ok(path)
}

pub(crate) fn db_path_from_setup_env(env_path: &Path) -> io::Result<Option<PathBuf>> {
    let raw = match std::fs::read_to_string(env_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let values = firstrun::parse_env(&raw);
    if let Some(db_path) = values.get("CORTEX_DB_PATH")
        && !db_path.trim().is_empty()
        && db_path != "/data/cortex.db"
    {
        return Ok(Some(PathBuf::from(db_path)));
    }
    let uses_container_db_path = values
        .get("CORTEX_DB_PATH")
        .is_some_and(|db_path| db_path == "/data/cortex.db");
    let Some(data_volume) = values.get("CORTEX_DATA_VOLUME") else {
        if uses_container_db_path {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "{} uses CORTEX_DB_PATH=/data/cortex.db but does not set absolute CORTEX_DATA_VOLUME",
                    env_path.display()
                ),
            ));
        }
        return Ok(None);
    };
    let volume_path = PathBuf::from(data_volume);
    if volume_path.is_absolute() {
        return Ok(Some(volume_path.join("cortex.db")));
    }
    if uses_container_db_path {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "{} uses CORTEX_DB_PATH=/data/cortex.db but CORTEX_DATA_VOLUME is not absolute: {}",
                env_path.display(),
                data_volume
            ),
        ));
    }
    Ok(None)
}

pub(crate) fn current_uid_gid() -> (String, String) {
    firstrun::current_uid_gid()
}

pub(crate) fn write_executable_file(path: &Path, body: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = std::fs::OpenOptions::new();
        options
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .custom_flags(libc::O_NOFOLLOW);
        options.open(path)?.write_all(body.as_bytes())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, body)?;
    Ok(())
}

pub(crate) fn write_private_file(path: &Path, body: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = std::fs::OpenOptions::new();
        options
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        options.open(path)?.write_all(body.as_bytes())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, body)?;
    Ok(())
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
