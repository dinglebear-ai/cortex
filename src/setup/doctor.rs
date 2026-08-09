use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use super::debug_wrapper::{check_debug_compose_content_phase, check_debug_wrapper_content_phase};
use super::firstrun::filesystem_phase;
use super::sessions_watch::{run_sessions_watch_service_setup, transcript_root_permissions_phase};
use super::{
    PhaseTimer, SessionsWatchServiceAction, SetupPhase, SetupReport, SetupReportInput, SetupStatus,
    check_file_phase, setup_report,
};

pub async fn run_setup_doctor(fix: bool, yes: bool) -> io::Result<SetupReport> {
    let started = Instant::now();
    let home = super::cortex_home_dir()?;
    let env_path = home.join(".env");
    let compose_dir = home.join("compose");
    let data_dir = home.join("data");
    let user_home = super::user_home_dir()?;
    let repo_path = std::env::current_dir()?;
    let wrapper_path = user_home.join(".local/bin/cortex");
    let debug_override_path = compose_dir.join("docker-compose.override.yml");
    let mut phases = vec![
        filesystem_phase(super::SetupMode::Check, &home, &data_dir, &compose_dir)?,
        check_file_phase("env", &env_path, "run cortex setup"),
        check_file_phase(
            "compose-assets",
            &compose_dir.join("docker-compose.yml"),
            "run cortex setup repair",
        ),
        check_file_phase(
            "debug-wrapper",
            &wrapper_path,
            "run cortex setup debugwrapper install",
        ),
        downgrade_dev_phase(
            check_debug_wrapper_content_phase(&wrapper_path, &repo_path),
            "production binary installed (not the dev wrapper — expected in production)",
        ),
        check_file_phase(
            "debug-compose",
            &debug_override_path,
            "run cortex setup debugcompose install",
        ),
        downgrade_dev_phase(
            check_debug_compose_content_phase(&debug_override_path, &repo_path),
            "override uses production config (not the debug build override — expected in production)",
        ),
        transcript_root_permissions_phase(&user_home),
        super::sessions_watch::ai_index_timer_disabled_phase(),
    ];

    phases.extend(
        run_sessions_watch_service_setup(SessionsWatchServiceAction::Check)
            .await?
            .phases,
    );
    phases.push(runtime_current_phase(&repo_path));
    phases.push(check_transcript_forward_env_migration(&env_path, fix, yes));
    phases.push(stale_agent_command_units_phase(fix, yes).await);

    let elapsed_ms = started.elapsed().as_millis();
    Ok(setup_report(
        SetupReportInput {
            mode: "doctor",
            elapsed_ms,
            home,
            env_path,
            compose_dir,
            data_dir,
            health_url: "setup doctor".to_string(),
            mcp_url: "setup doctor".to_string(),
        },
        phases,
    ))
}

/// Dev-mode checks (debug-wrapper-content, debug-compose-content) always fail
/// when a production binary/override is installed. In `setup doctor` that's the
/// expected steady state, so we downgrade Error → Warn with a clearer detail
/// and rewrite the issue kind accordingly. Other contexts (e.g. `cortex setup
/// debugwrapper check`) keep the raw Error semantics.
pub(super) fn downgrade_dev_phase(phase: SetupPhase, detail: &str) -> SetupPhase {
    if matches!(phase.status, SetupStatus::Error) {
        SetupPhase {
            status: SetupStatus::Warn,
            issue_kind: None,
            detail: detail.to_string(),
            ..phase
        }
    } else {
        phase
    }
}

fn runtime_current_phase(repo_path: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("runtime-current");
    let script = repo_path.join("scripts/check-runtime-current.sh");
    if !script.exists() {
        return timer.finish(SetupStatus::Error, format!("missing {}", script.display()));
    }
    match Command::new("bash")
        .arg(script)
        .arg("--allow-local-image")
        .current_dir(repo_path)
        .output()
    {
        Ok(output) if output.status.success() => timer.finish(
            SetupStatus::Ok,
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .last()
                .unwrap_or("runtime current")
                .to_string(),
        ),
        Ok(output) => timer.finish(
            SetupStatus::Error,
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .trim()
            .to_string(),
        ),
        Err(error) => timer.finish(SetupStatus::Error, error.to_string()),
    }
}

/// Returns `true` when `unit_text` (the output of `systemctl --user cat
/// <unit>`) has an `ExecStart=` line invoking cortex's agent-command
/// spool-drain path using grammar older than the current canonical `ingest
/// shell agent index`. Anchored to `ExecStart=` specifically — using the
/// same shared basename+argv-shape primitives as
/// `is_agent_command_ingest_spool_invocation` (`src/command_log.rs`, see
/// engineering-review note there) — rather than a raw substring search over
/// the whole unit file, so a `Description=`/comment that merely *mentions*
/// the old grammar can never cause a false positive. Doesn't attempt to
/// judge whether the unit's target host is "correct" (that's an operator
/// decision); it only flags a stale, mechanically-detectable fact.
pub(crate) fn agent_command_unit_uses_stale_grammar(unit_text: &str) -> bool {
    unit_text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ExecStart="))
        .any(exec_start_uses_stale_grammar)
}

fn exec_start_uses_stale_grammar(exec_start: &str) -> bool {
    let tokens: Vec<&str> = exec_start.split_whitespace().collect();
    let Some(program) = tokens.first() else {
        return false;
    };
    if !crate::command_log::cortex_argv_program_matches(program) {
        return false;
    }
    let rest = &tokens[1..];
    let uses_current_grammar = crate::command_log::is_current_shell_agent_index_argv(rest);
    let uses_stale_grammar = crate::command_log::is_grouped_legacy_agent_command_argv(rest)
        || crate::command_log::is_bare_legacy_agent_command_argv(rest);
    uses_stale_grammar && !uses_current_grammar
}

/// Pure gating logic for whether `--fix` should actually disable stale
/// units: requires both `fix` AND `yes`, and only when there's something
/// stale to act on. Extracted so it's unit-testable without any
/// `systemctl` dependency.
fn should_disable(fix: bool, yes: bool, stale: &[String]) -> bool {
    fix && yes && !stale.is_empty()
}

/// Scans `systemctl --user` service/timer units for ones whose `ExecStart=`
/// still invokes the pre-rename `agent-command ingest-spool` grammar, and
/// reports them so an operator can rerun `cortex setup shell agent install`
/// (which regenerates the wrapper) or manually fix the unit. Requires both
/// `fix: true` AND `yes: true` before disabling anything — matching this
/// repo's own `cortex compose down` precedent of refusing destructive action
/// without an explicit `--yes` — since a false positive here would silently
/// kill an unrelated running service. This is synchronous, blocking code:
/// callers MUST run it via `tokio::task::spawn_blocking` (see the async
/// wrapper below) rather than calling it directly from an async context.
fn stale_agent_command_units_scan(fix: bool, yes: bool) -> SetupPhase {
    let timer = PhaseTimer::start("stale-agent-command-units");
    let Some(unit_list) = super::systemd::systemctl_user_state("list-units", "--all") else {
        return timer.finish(SetupStatus::Ok, "systemctl --user unavailable; skipped");
    };
    // Engineering-review fix: no unit-name pre-filter. A name-substring
    // filter here (dropped from an earlier version) could silently exclude
    // a renamed drain unit that doesn't happen to contain "cortex"/"agent"/
    // "command" in its name, producing a false all-clear from `doctor`. A
    // `systemctl --user cat` per `.service`/`.timer` unit is cheap and this
    // scan already runs off the async runtime (see
    // `stale_agent_command_units_phase` below), so there's no correctness
    // reason to skip any candidate unit.
    let unit_names: Vec<&str> = unit_list
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.ends_with(".service") || name.ends_with(".timer"))
        .collect();

    let mut stale = Vec::new();
    for unit in unit_names {
        let Some(unit_text) = super::systemd::systemctl_user_state("cat", unit) else {
            tracing::debug!(
                unit,
                "systemctl --user cat failed; skipping stale-grammar check for this unit \
                 (result is inconclusive, not a confirmed clean bill of health)"
            );
            continue;
        };
        if agent_command_unit_uses_stale_grammar(&unit_text) {
            stale.push(unit.to_string());
        }
    }

    if stale.is_empty() {
        return timer.finish(
            SetupStatus::Ok,
            "no stale agent-command grammar found in systemd --user units",
        );
    }

    if fix && !yes {
        return timer.finish(
            SetupStatus::Warn,
            format!(
                "stale agent-command grammar in: {} — rerun with `cortex doctor --fix --yes` \
                 to disable, or fix/regenerate manually with `cortex setup shell agent install`",
                stale.join(", ")
            ),
        );
    }

    if should_disable(fix, yes, &stale) {
        let mut disabled = Vec::new();
        let mut failed = Vec::new();
        for unit in &stale {
            match super::systemd::run_systemctl_user(&["disable", "--now", unit]) {
                Ok(output) if output.status.success() => disabled.push(unit.clone()),
                Ok(output) => failed.push(format!("{unit} (systemctl exited {})", output.status)),
                Err(error) => failed.push(format!("{unit} ({error})")),
            }
        }
        if failed.is_empty() {
            return timer.finish(
                SetupStatus::Warn,
                format!(
                    "disabled stale agent-command units: {}",
                    disabled.join(", ")
                ),
            );
        }
        return timer.finish(
            SetupStatus::Error,
            format!(
                "disabled {} unit(s) [{}]; FAILED to disable: {}",
                disabled.len(),
                disabled.join(", "),
                failed.join("; ")
            ),
        );
    }

    timer.finish(
        SetupStatus::Warn,
        format!(
            "stale agent-command grammar in: {} — run `cortex setup shell agent install` then \
             `cortex doctor --fix --yes`, or fix/disable manually",
            stale.join(", ")
        ),
    )
}

/// Async entry point: offloads the blocking systemd scan (subprocess spawns
/// via `Command::output()`, no timeout) onto the blocking thread pool so it
/// can never stall a Tokio worker thread, regardless of how many systemd
/// --user units the host has. This codebase has one prior incident
/// (`ai_watcher_process_start_time()` in `src/app/watch_status.rs`) where a
/// blocking `Command::output()` call was made directly inside an `async fn`
/// and stalled a Tokio worker thread; this scan loops over every systemd
/// --user unit and would reproduce the same bug, worse, via fan-out.
pub(crate) async fn stale_agent_command_units_phase(fix: bool, yes: bool) -> SetupPhase {
    let timer = PhaseTimer::start("stale-agent-command-units");
    match tokio::task::spawn_blocking(move || stale_agent_command_units_scan(fix, yes)).await {
        Ok(phase) => phase,
        Err(error) => timer.finish(
            SetupStatus::Error,
            format!("stale agent-command unit scan task panicked: {error}"),
        ),
    }
}

/// ENV-003: Check for deprecated transcript forwarding environment variable
/// configurations and optionally migrate unambiguous files. This is called
/// from `run_setup_doctor` and integrates with the existing fix authorization
/// pattern (both `--fix` AND `--yes` required for mutation).
pub(crate) fn check_transcript_forward_env_migration(
    env_path: &Path,
    fix: bool,
    yes: bool,
) -> SetupPhase {
    use crate::heartbeat_agent::{AI_TRANSCRIPT_FORWARD_ENV, AI_TRANSCRIPT_FORWARD_LEGACY_ENV};

    let timer = PhaseTimer::start("transcript-forward-env-migration");

    // Replacing a symlink would detach it from its target. Refuse before any
    // read or write so an operator can migrate the real file deliberately.
    if std::fs::symlink_metadata(env_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return timer.finish(
            SetupStatus::Error,
            ".env is a symbolic link; migrate its target explicitly",
        );
    }

    if !env_path.exists() {
        return timer.finish(SetupStatus::Ok, ".env file not found; skipped");
    }

    let content = match std::fs::read_to_string(env_path) {
        Ok(content) => content,
        Err(error) => {
            return timer.finish(
                SetupStatus::Error,
                format!("failed to read .env file: {error}"),
            );
        }
    };

    let mut assignments = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        for (key, legacy) in [
            (AI_TRANSCRIPT_FORWARD_ENV, false),
            (AI_TRANSCRIPT_FORWARD_LEGACY_ENV, true),
        ] {
            if let Some(value) = trimmed
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
            {
                assignments.push((line_num, value.to_string(), legacy));
                break;
            }
        }
    }

    if assignments.is_empty() {
        return timer.finish(SetupStatus::Ok, "no transcript forwarding configuration");
    }
    let value = &assignments[0].1;
    if assignments
        .iter()
        .any(|(_, candidate, _)| candidate != value)
    {
        return timer.finish(
            SetupStatus::Error,
            "conflicting duplicate transcript forwarding values; manual resolution required",
        );
    }
    let has_legacy = assignments.iter().any(|(_, _, legacy)| *legacy);
    if !has_legacy && assignments.len() == 1 {
        return timer.finish(
            SetupStatus::Ok,
            "transcript forwarding uses current variable name",
        );
    }
    if !fix || !yes {
        let authorization = if fix { "--yes" } else { "--fix --yes" };
        return timer.finish(
            SetupStatus::Warn,
            format!(
                "deprecated {AI_TRANSCRIPT_FORWARD_LEGACY_ENV} or duplicate transcript forwarding configuration; run with {authorization} to normalize to {AI_TRANSCRIPT_FORWARD_ENV}"
            ),
        );
    }

    normalize_transcript_forward_assignments(env_path, &content, &assignments, timer)
}

fn normalize_transcript_forward_assignments(
    env_path: &Path,
    content: &str,
    assignments: &[(usize, String, bool)],
    timer: PhaseTimer,
) -> SetupPhase {
    let assignment_lines = assignments
        .iter()
        .map(|(line, _, _)| *line)
        .collect::<std::collections::HashSet<_>>();
    let first_line = assignments[0].0;
    let value = &assignments[0].1;
    let rewritten_lines = content
        .lines()
        .enumerate()
        .filter_map(|(line_num, line)| {
            if line_num == first_line {
                let leading = &line[..line.len() - line.trim_start().len()];
                return Some(format!(
                    "{leading}{}={value}",
                    crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV
                ));
            }
            if assignment_lines.contains(&line_num) {
                None
            } else {
                Some(line.to_string())
            }
        })
        .collect::<Vec<_>>();
    let mut new_content = rewritten_lines.join("\n");
    if content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    match atomic_write_env_file(env_path, content, &new_content) {
        Ok(()) => timer.finish(
            SetupStatus::Ok,
            "normalized transcript forwarding configuration",
        ),
        Err(error) => timer.finish(
            SetupStatus::Error,
            format!("failed to migrate .env file: {error}"),
        ),
    }
}

/// Atomic write implementation matching the pattern from firstrun.rs.
/// This ensures the .env file is never in a partially written state.
fn atomic_write_env_file(path: &Path, expected: &str, out: &str) -> io::Result<()> {
    use std::io::Write;

    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "env path must have a parent directory",
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "env path has no file name"))?;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(".{file_name}.tmp.{pid}.{nanos}"));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }

    let write_result: io::Result<()> = (|| {
        let mut file = options.open(&tmp_path)?;
        file.write_all(out.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    if std::fs::read_to_string(path)? != expected {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(io::Error::other(
            "env file changed concurrently; refusing to overwrite it",
        ));
    }

    if let Err(err) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    // fsync parent directory
    if let Some(parent) = path.parent() {
        let dir = std::fs::File::open(parent)?;
        dir.sync_all()?;
    }

    Ok(())
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "doctor_transcript_forward_tests.rs"]
mod transcript_forward_tests;
