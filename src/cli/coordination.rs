use std::collections::HashMap;
use std::path::PathBuf;

use super::sessions_watch::systemctl_user_output;
use super::setup::{SetupPhase, SetupStatus, read_env_value, setup_data_dir};
#[derive(Debug, Clone)]
pub(crate) struct ContainerMountInfo {
    pub(crate) mount_type: Option<String>,
    pub(crate) mount_source: Option<String>,
    pub(crate) running: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SystemctlEnv {
    /// Inline `Environment=` KEY=VALUE pairs (from `-p Environment`).
    pub(crate) inline: Vec<(String, String)>,
    /// Paths from `EnvironmentFiles=`.
    pub(crate) files: Vec<PathBuf>,
    /// True when `systemctl show` succeeded but the unit was not found.
    pub(crate) unit_missing: bool,
}

#[derive(Debug, Default)]
pub(crate) struct DoctorCache {
    container_inspect: HashMap<String, Result<ContainerMountInfo, String>>,
    systemctl_env: HashMap<String, Result<SystemctlEnv, String>>,
}

impl DoctorCache {
    pub(crate) fn container_inspect(
        &mut self,
        container: &str,
    ) -> Result<ContainerMountInfo, String> {
        if let Some(cached) = self.container_inspect.get(container) {
            return cached.clone();
        }
        let result = docker_inspect_data_mount(container);
        self.container_inspect
            .insert(container.to_string(), result.clone());
        result
    }

    pub(crate) fn systemctl_env(&mut self, unit: &str) -> Result<SystemctlEnv, String> {
        if let Some(cached) = self.systemctl_env.get(unit) {
            return cached.clone();
        }
        let result = systemctl_show_env(unit);
        self.systemctl_env.insert(unit.to_string(), result.clone());
        result
    }
}

/// Run both coordination phases (data-mount + sessions-watch-coord) with a shared
/// cache so the underlying `docker inspect` only fires once.
pub(crate) fn run_coordination_phases() -> Vec<SetupPhase> {
    let data_dir = setup_data_dir();
    let env_path = data_dir.join(".env");
    let mut cache = DoctorCache::default();
    vec![
        data_mount_phase_cached(data_dir.as_path(), env_path.as_path(), &mut cache),
        sessions_watch_coordination_phase(env_path.as_path(), &mut cache),
    ]
}

fn docker_inspect_data_mount(container: &str) -> Result<ContainerMountInfo, String> {
    let output = crate::env::command("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Mounts}}{{if eq .Destination \"/data\"}}{{.Type}}|{{.Source}}{{end}}{{end}}|{{.State.Running}}",
            container,
        ])
        .output()
        .map_err(|error| format!("docker not available: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "container '{container}' not present (docker inspect failed: {})",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let parts: Vec<&str> = stdout.split('|').collect();
    let running = parts.last().is_some_and(|s| *s == "true");
    if parts.len() < 3 || parts[0].is_empty() {
        return Ok(ContainerMountInfo {
            mount_type: None,
            mount_source: None,
            running,
        });
    }
    Ok(ContainerMountInfo {
        mount_type: Some(parts[0].to_string()),
        mount_source: Some(parts[1].to_string()),
        running,
    })
}

fn systemctl_show_env(unit: &str) -> Result<SystemctlEnv, String> {
    // Reuse the shared --user wrapper so we pick up the
    // DBUS_SESSION_BUS_ADDRESS / XDG_RUNTIME_DIR fallback for headless
    // hosts where the user bus isn't auto-discovered.
    let stdout = systemctl_user_output(&[
        "show",
        unit,
        "-p",
        "Environment",
        "-p",
        "EnvironmentFiles",
        "-p",
        "LoadState",
        "--no-pager",
    ])
    .map_err(|error| error.to_string())?;
    Ok(parse_systemctl_env_output(&stdout))
}

/// Parse `systemctl --user show -p Environment -p EnvironmentFiles -p LoadState`
/// output into a `SystemctlEnv`. Lines look like:
///
/// ```text
/// Environment=KEY1=val1 KEY2=val2
/// EnvironmentFiles=/etc/foo (ignore_errors=no)
/// LoadState=loaded
/// ```
///
/// Uses `split_once('=')` everywhere — values may legitimately contain `=`.
pub(crate) fn parse_systemctl_env_output(stdout: &str) -> SystemctlEnv {
    let mut inline = Vec::new();
    let mut files = Vec::new();
    let mut unit_missing = false;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Environment" => {
                inline.extend(parse_systemctl_env_inline(value));
            }
            "EnvironmentFiles" => {
                for path in parse_systemctl_env_files(value) {
                    files.push(path);
                }
            }
            "LoadState" if value.trim() == "not-found" => {
                unit_missing = true;
            }
            _ => {}
        }
    }
    SystemctlEnv {
        inline,
        files,
        unit_missing,
    }
}

/// Parse the inline value of `Environment=...`. Each space-separated token is
/// a `KEY=VALUE` pair; `VALUE` may contain `=`, so we use `split_once('=')`.
fn parse_systemctl_env_inline(value: &str) -> Vec<(String, String)> {
    value
        .split_whitespace()
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Parse the inline value of `EnvironmentFiles=...`. systemd renders this as
/// a space-separated list of `<path> (ignore_errors=<bool>)` pairs. We take
/// just the path token.
fn parse_systemctl_env_files(value: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for token in value.split_whitespace() {
        if token.starts_with('(') {
            continue;
        }
        if token.is_empty() {
            continue;
        }
        paths.push(PathBuf::from(token));
    }
    paths
}

/// Look up `CORTEX_DB_PATH` from the systemctl-rendered env. Inline
/// `Environment=` values take precedence; otherwise we walk each
/// `EnvironmentFiles` entry. Missing files are skipped (not fatal).
pub(crate) fn lookup_systemd_db_path(env: &SystemctlEnv) -> Option<String> {
    if let Some((_, value)) = env.inline.iter().find(|(k, _)| k == "CORTEX_DB_PATH") {
        return Some(value.clone());
    }
    for path in &env.files {
        if let Some(value) = read_env_value(path, "CORTEX_DB_PATH") {
            return Some(value);
        }
    }
    None
}

// data_mount_phase (uncached wrapper) removed by bead cortex-0p8r.11.
// Sole caller was setup_report (SessionStart hook), which no longer needs it
// post-cutover. All remaining callers (compose doctor, db status --check-coord)
// use data_mount_phase_cached so the docker inspect result can be shared with
// sessions_watch_coordination_phase within a single invocation.

/// Cached variant of `data_mount_phase`. See module-level note for why we
/// share `docker inspect` across phases.
pub(crate) fn data_mount_phase_cached(
    data_dir: &std::path::Path,
    env_path: &std::path::Path,
    cache: &mut DoctorCache,
) -> SetupPhase {
    let name = "data-mount";
    let container =
        crate::env::var("CORTEX_CONTAINER_NAME").unwrap_or_else(|_| "cortex".to_string());

    let expected_dir = crate::env::var("CORTEX_DATA_VOLUME")
        .ok()
        .or_else(|| read_env_value(env_path, "CORTEX_DATA_VOLUME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.to_path_buf());

    let info = match cache.container_inspect(&container) {
        Ok(info) => info,
        Err(detail) => {
            // Distinguish "container absent" (Skipped per doctor spec —
            // sessions-watch absent style) from "docker enumeration failed"
            // (Warn — could not enumerate inputs). docker inspect on a
            // missing container reports "No such object" / "no such
            // container"; anything else is a probe failure.
            let lower = detail.to_ascii_lowercase();
            let status = if lower.contains("no such object") || lower.contains("no such container")
            {
                SetupStatus::Skipped
            } else {
                SetupStatus::Warn
            };
            return SetupPhase {
                name,
                status,
                detail,
            };
        }
    };
    if !info.running {
        return SetupPhase {
            name,
            status: SetupStatus::Skipped,
            detail: format!("container '{container}' not running"),
        };
    }
    let Some(mount_source) = info.mount_source else {
        return SetupPhase {
            name,
            status: SetupStatus::Error,
            detail: format!("container '{container}' has no /data mount — run `cortex compose up`"),
        };
    };
    let mount_type = info.mount_type.unwrap_or_default();
    if mount_type != "bind" {
        return SetupPhase {
            name,
            status: SetupStatus::Error,
            detail: format!(
                "container /data is a {} (expected bind to {}). \
                 CLI and container are writing different DBs. \
                 Repair: `cortex compose up` (recreates with --env-file)",
                mount_type,
                expected_dir.display()
            ),
        };
    }
    let expected = match canonicalize_with_warning(&expected_dir) {
        Ok(path) => path,
        Err(detail) => {
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail,
            };
        }
    };
    let actual_source = PathBuf::from(&mount_source);
    let actual = match canonicalize_with_warning(&actual_source) {
        Ok(path) => path,
        Err(detail) => {
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail,
            };
        }
    };
    if actual != expected {
        return SetupPhase {
            name,
            status: SetupStatus::Error,
            detail: format!(
                "container /data bind source ({}) does not match CORTEX_DATA_VOLUME ({}). \
                 CLI and container are writing different DBs. Repair: `cortex compose up`",
                mount_source,
                expected_dir.display()
            ),
        };
    }
    SetupPhase {
        name,
        status: SetupStatus::Ok,
        detail: format!("bind {} -> /data matches CORTEX_DATA_VOLUME", mount_source),
    }
}

/// Verify the host systemd `cortex-sessions-watch.service`'s effective
/// `CORTEX_DB_PATH` resolves to the same canonical host path as the
/// container's `/data` bind source. A mismatch means the host sessions-watch
/// service is writing checkpoints to a DB the container will never read.
///
/// Status semantics (per epic decisions):
/// - `Skipped` — sessions-watch unit not installed or not loadable. Only valid
///   skipped reason.
/// - `Ok` — canonical paths match.
/// - `Warning` — could not enumerate the inputs (docker/systemctl failed,
///   canonicalize ENOENT/EACCES). The drift bug was a silent literal-string
///   compare fallback; we never do that — always warn with the OS error.
/// - `Error` — both sides resolved and the canonical paths differ.
pub(crate) fn sessions_watch_coordination_phase(
    env_path: &std::path::Path,
    cache: &mut DoctorCache,
) -> SetupPhase {
    let name = "sessions-watch-coord";
    let unit = crate::env::var("CORTEX_AI_WATCH_UNIT")
        .unwrap_or_else(|_| "cortex-sessions-watch.service".to_string());
    let container =
        crate::env::var("CORTEX_CONTAINER_NAME").unwrap_or_else(|_| "cortex".to_string());

    let env = match cache.systemctl_env(&unit) {
        Ok(env) => env,
        Err(detail) => {
            // systemctl enumeration failed (binary missing, bus error,
            // permission denied, etc.); per the doctor spec this is
            // `warn` — `skipped` is reserved for "sessions-watch absent".
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail,
            };
        }
    };
    if env.unit_missing {
        return SetupPhase {
            name,
            status: SetupStatus::Skipped,
            detail: format!("systemd unit {unit} is not installed"),
        };
    }
    let Some(ai_db_path) = lookup_systemd_db_path(&env) else {
        return SetupPhase {
            name,
            status: SetupStatus::Warn,
            detail: format!(
                "could not find CORTEX_DB_PATH in {unit} (Environment/EnvironmentFiles)"
            ),
        };
    };
    let info = match cache.container_inspect(&container) {
        Ok(info) => info,
        Err(detail) => {
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail: format!("could not inspect container: {detail}"),
            };
        }
    };
    if !info.running {
        return SetupPhase {
            name,
            status: SetupStatus::Warn,
            detail: format!("container '{container}' not running"),
        };
    }
    let Some(mount_source) = info.mount_source else {
        return SetupPhase {
            name,
            status: SetupStatus::Warn,
            detail: format!("container '{container}' has no /data mount"),
        };
    };

    // sessions-watch points at the SQLite *file*; container exposes the parent dir.
    let ai_path = PathBuf::from(&ai_db_path);
    let ai_dir = ai_path.parent().map(PathBuf::from).unwrap_or(ai_path);
    let canonical_ai = match canonicalize_with_warning(&ai_dir) {
        Ok(path) => path,
        Err(detail) => {
            // NEVER silently compare literal strings on canonicalize failure
            // — that was the original drift bug.
            let _ = env_path; // env_path reserved for future plugin .env cross-checks.
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail,
            };
        }
    };
    let mount_pathbuf = PathBuf::from(&mount_source);
    let canonical_container = match canonicalize_with_warning(&mount_pathbuf) {
        Ok(path) => path,
        Err(detail) => {
            return SetupPhase {
                name,
                status: SetupStatus::Warn,
                detail,
            };
        }
    };
    if canonical_ai == canonical_container {
        return SetupPhase {
            name,
            status: SetupStatus::Ok,
            detail: format!(
                "sessions-watch CORTEX_DB_PATH ({}) and container /data bind ({}) resolve to {}",
                ai_db_path,
                mount_source,
                canonical_ai.display()
            ),
        };
    }
    SetupPhase {
        name,
        status: SetupStatus::Error,
        detail: format!(
            "sessions-watch CORTEX_DB_PATH canonicalizes to {} but container /data bind canonicalizes to {} — \
             host service and container are writing different DBs",
            canonical_ai.display(),
            canonical_container.display()
        ),
    }
}

/// Canonicalize a path, returning a structured warning string on ENOENT /
/// EACCES instead of falling back to the literal path. The literal-fallback
/// pattern is the drift bug we're guarding against.
pub(crate) fn canonicalize_with_warning(path: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|err| format!("could not canonicalize {}: {err}", path.display()))
}

#[cfg(test)]
#[path = "coordination_tests.rs"]
mod tests;
