use std::io::{self, ErrorKind};
use std::path::Path;
use std::time::Instant;

use super::firstrun::ensure_private_dir;
use super::{
    PhaseTimer, SetupPhase, SetupReport, SetupStatus, ShellAgentAction, check_file_phase,
    host_local_report_input, setup_path_value, setup_report, write_executable_file,
    write_private_file,
};

pub async fn run_shell_agent_setup(action: ShellAgentAction) -> io::Result<SetupReport> {
    let started = Instant::now();
    let home = super::cortex_home_dir()?;
    let env_path = home.join(".env");
    let compose_dir = home.join("compose");
    let data_dir = home.join("data");
    let user_home = super::user_home_dir()?;
    let state_dir = user_home.join(".local/state/cortex");
    let spool_path = state_dir.join("agent-command.jsonl");
    let wrapper_path = user_home.join(".local/bin/cortex-agent-command-wrapper");
    let mut phases = Vec::new();

    match action {
        ShellAgentAction::Install => {
            let cortex_bin = resolve_agent_command_cortex_binary().await?;
            phases.push(install_agent_command_files(
                &wrapper_path,
                &spool_path,
                &state_dir,
                &cortex_bin,
            )?);
            phases.push(agent_command_env_phase(&wrapper_path, &user_home));
        }
        ShellAgentAction::Remove => {
            phases.push(remove_agent_command_wrapper(&wrapper_path)?);
            phases.push(agent_command_env_phase(&wrapper_path, &user_home));
        }
        ShellAgentAction::Check => {
            let cortex_bin = resolve_agent_command_cortex_binary().await?;
            phases.push(check_file_phase(
                "agent-command-wrapper",
                &wrapper_path,
                "run cortex setup shell agent install",
            ));
            phases.push(agent_command_content_phase(
                &wrapper_path,
                &spool_path,
                &cortex_bin,
            ));
            phases.push(agent_command_state_phase(&state_dir, &spool_path));
            phases.push(agent_command_env_phase(&wrapper_path, &user_home));
        }
    }

    let elapsed_ms = started.elapsed().as_millis();
    Ok(setup_report(
        host_local_report_input(
            action.as_str(),
            elapsed_ms,
            home,
            env_path,
            compose_dir,
            data_dir,
        ),
        phases,
    ))
}

fn install_agent_command_files(
    wrapper_path: &Path,
    spool_path: &Path,
    state_dir: &Path,
    cortex_bin: &Path,
) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("agent-command-files");
    ensure_private_dir(state_dir)?;
    if let Some(parent) = wrapper_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_executable_file(
        wrapper_path,
        &agent_command_wrapper_script(cortex_bin, spool_path),
    )?;
    ensure_agent_command_spool_file(spool_path)?;
    Ok(timer.finish(
        SetupStatus::Ok,
        format!("wrote {}, {}", wrapper_path.display(), spool_path.display()),
    ))
}

fn ensure_agent_command_spool_file(spool_path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(spool_path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} must not be a symlink", spool_path.display()),
                ));
            }
            if !file_type.is_file() {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} must be a regular file", spool_path.display()),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(spool_path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => write_private_file(spool_path, ""),
        Err(error) => Err(error),
    }
}

fn remove_agent_command_wrapper(wrapper_path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("agent-command-wrapper");
    match std::fs::remove_file(wrapper_path) {
        Ok(()) => Ok(timer.finish(
            SetupStatus::Ok,
            format!("removed {}", wrapper_path.display()),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(timer.finish(
            SetupStatus::Ok,
            format!("{} already absent", wrapper_path.display()),
        )),
        Err(error) => Err(error),
    }
}

fn agent_command_content_phase(
    wrapper_path: &Path,
    spool_path: &Path,
    cortex_bin: &Path,
) -> SetupPhase {
    let timer = PhaseTimer::start("agent-command-content");
    let expected = agent_command_wrapper_script(cortex_bin, spool_path);
    match std::fs::read_to_string(wrapper_path) {
        Ok(current) if current == expected => timer.finish(
            SetupStatus::Ok,
            "agent command wrapper matches generated content",
        ),
        Ok(_) => timer.finish(
            SetupStatus::Error,
            format!(
                "{} does not match generated agent command wrapper",
                wrapper_path.display()
            ),
        ),
        Err(error) => timer.finish(SetupStatus::Error, error.to_string()),
    }
}

fn agent_command_state_phase(state_dir: &Path, spool_path: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("agent-command-state");
    let state_metadata = match std::fs::symlink_metadata(state_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return timer.finish(
                SetupStatus::Warn,
                format!(
                    "missing {}; run cortex setup shell agent install",
                    state_dir.display()
                ),
            );
        }
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    let state_type = state_metadata.file_type();
    if state_type.is_symlink() || !state_type.is_dir() {
        return timer.finish(
            SetupStatus::Error,
            format!("{} must be a real directory", state_dir.display()),
        );
    }
    let spool_metadata = match std::fs::symlink_metadata(spool_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return timer.finish(
                SetupStatus::Warn,
                format!(
                    "missing {}; run cortex setup shell agent install",
                    spool_path.display()
                ),
            );
        }
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    let spool_type = spool_metadata.file_type();
    if spool_type.is_symlink() || !spool_type.is_file() {
        return timer.finish(
            SetupStatus::Error,
            format!("{} must be a regular file", spool_path.display()),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let state_mode = state_metadata.permissions().mode() & 0o777;
        let spool_mode = spool_metadata.permissions().mode() & 0o777;
        if state_mode & 0o077 != 0 || spool_mode & 0o077 != 0 {
            return timer.finish(
                SetupStatus::Error,
                format!(
                    "unsafe permissions state={state_mode:o} spool={spool_mode:o}; run cortex setup shell agent install"
                ),
            );
        }
    }
    timer.finish(
        SetupStatus::Ok,
        format!("state ready at {}", spool_path.display()),
    )
}

fn agent_command_env_phase(wrapper_path: &Path, user_home: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("agent-command-env");
    let expected = wrapper_path.display().to_string();
    if crate::env::var("CLAUDE_CODE_SHELL_PREFIX").ok().as_deref() == Some(expected.as_str()) {
        return timer.finish(
            SetupStatus::Ok,
            "CLAUDE_CODE_SHELL_PREFIX matches the generated wrapper",
        );
    }
    match claude_settings_shell_prefix(user_home) {
        Ok(Some(value)) if value == expected => timer.finish(
            SetupStatus::Ok,
            format!(
                "{} configures CLAUDE_CODE_SHELL_PREFIX",
                user_home.join(".claude/settings.json").display()
            ),
        ),
        Ok(Some(value)) => timer.finish(
            SetupStatus::Warn,
            format!(
                "CLAUDE_CODE_SHELL_PREFIX points to {}; expected {}",
                value,
                wrapper_path.display()
            ),
        ),
        Ok(None) => timer.finish(
            SetupStatus::Warn,
            format!(
                "set CLAUDE_CODE_SHELL_PREFIX={} in Claude Code's environment or ~/.claude/settings.json",
                wrapper_path.display()
            ),
        ),
        Err(error) => timer.finish(SetupStatus::Warn, error.to_string()),
    }
}

fn agent_command_wrapper_script(cortex_bin: &Path, spool_path: &Path) -> String {
    let cortex_bin = setup_path_value(cortex_bin).expect("validated cortex binary path");
    let spool_path = setup_path_value(spool_path).expect("validated agent command spool path");
    format!(
        r#"#!/usr/bin/env sh
# Best-effort agent-command logging. The probe confirms `ingest shell agent
# wrap` is runnable; if cortex is missing or its CLI changed, fall through and
# exec the command directly so logging can never brick the shell.
if {cortex_bin} ingest shell agent wrap --probe >/dev/null 2>&1; then
  exec {cortex_bin} ingest shell agent wrap --spool {spool_path} -- "$@"
fi
exec "$@"
"#
    )
}

/// Resolves and validates the cortex binary to embed in the generated
/// wrapper script. Runs on the blocking thread pool: `validate_agent_command_binary`
/// spawns a `cortex --version` subprocess (`Command::output()`), which would
/// otherwise block a Tokio worker thread for the subprocess's duration —
/// the same anti-pattern already fixed elsewhere in this codebase (see
/// `src/app/watch_status.rs`'s `ai_watcher_process_start_time()` and
/// `src/setup/doctor.rs`'s `stale_agent_command_units_phase`).
async fn resolve_agent_command_cortex_binary() -> io::Result<std::path::PathBuf> {
    tokio::task::spawn_blocking(resolve_agent_command_cortex_binary_blocking)
        .await
        .map_err(|error| io::Error::other(format!("resolve cortex binary task failed: {error}")))?
}

fn resolve_agent_command_cortex_binary_blocking() -> io::Result<std::path::PathBuf> {
    let path = super::resolve_cortex_binary()?;
    validate_agent_command_binary(&path)?;
    Ok(path)
}

fn validate_agent_command_binary(path: &Path) -> io::Result<()> {
    let output = crate::env::command(path).arg("--version").output()?;
    if !output.status.success() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} --version failed", path.display()),
        ));
    }
    let version_output = String::from_utf8_lossy(&output.stdout);
    let expected = format!("cortex {}", env!("CARGO_PKG_VERSION"));
    let actual = version_output.trim();
    if actual != expected {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "{} is not the current cortex binary: expected version {expected}, got {}",
                path.display(),
                actual
            ),
        ));
    }
    Ok(())
}

fn claude_settings_shell_prefix(user_home: &Path) -> io::Result<Option<String>> {
    let path = user_home.join(".claude/settings.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("parse {}: {error}", path.display()),
        )
    })?;
    Ok(value
        .get("env")
        .and_then(|env| env.get("CLAUDE_CODE_SHELL_PREFIX"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

#[cfg(test)]
#[path = "shell_agent_tests.rs"]
mod tests;
