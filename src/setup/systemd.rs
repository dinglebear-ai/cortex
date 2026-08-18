use std::io;
use std::io::ErrorKind;
use std::path::PathBuf;

use super::{SetupPhase, SetupStatus};

/// Returns a human-readable message for a failed systemctl invocation.
/// Prefers stdout (where `is-active` and `is-enabled` write the service state)
/// over stderr (where other subcommands write errors).
pub(super) fn systemctl_error_message(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(line) = stdout.lines().find(|l| !l.trim().is_empty()) {
        return line.to_string();
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .next()
        .unwrap_or("systemctl --user failed")
        .to_string()
}

pub(super) fn systemctl_user_phase(args: &[&str]) -> SetupPhase {
    systemctl_user_named_phase("systemctl-user", args)
}

pub(crate) fn systemctl_user_named_phase(name: &'static str, args: &[&str]) -> SetupPhase {
    let timer = super::PhaseTimer::start(name);
    match run_systemctl_user(args) {
        Ok(output) if output.status.success() => timer.finish(
            SetupStatus::Ok,
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("ok")
                .to_string(),
        ),
        Ok(output) => timer.finish(SetupStatus::Warn, systemctl_error_message(&output)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            timer.finish(SetupStatus::Warn, "systemctl not found")
        }
        Err(error) => timer.finish(SetupStatus::Warn, error.to_string()),
    }
}

pub(super) fn systemctl_user_required_phase(args: &[&str]) -> SetupPhase {
    systemctl_user_required_named_phase("systemctl-user", args)
}

pub(crate) fn systemctl_user_required_named_phase(name: &'static str, args: &[&str]) -> SetupPhase {
    let timer = super::PhaseTimer::start(name);
    match run_systemctl_user(args) {
        Ok(output) if output.status.success() => timer.finish(
            SetupStatus::Ok,
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("ok")
                .to_string(),
        ),
        Ok(output) => timer.finish(SetupStatus::Error, systemctl_error_message(&output)),
        Err(error) => timer.finish(SetupStatus::Error, error.to_string()),
    }
}

pub(crate) fn systemctl_user_state(command: &str, unit: &str) -> Option<String> {
    let output = run_systemctl_user(&[command, unit]).ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!stdout.is_empty()).then_some(stdout)
}

pub(crate) fn run_systemctl_user(args: &[&str]) -> io::Result<std::process::Output> {
    let output = crate::env::command("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    if output.status.success() || crate::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("DBUS_SESSION_BUS_ADDRESS") && !stderr.contains("user scope bus") {
        return Ok(output);
    }
    let Some((runtime_dir, bus_address)) = inferred_user_bus_env() else {
        return Ok(output);
    };
    crate::env::command("systemctl")
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .arg("--user")
        .args(args)
        .output()
}

pub(crate) fn inferred_user_bus_env() -> Option<(PathBuf, String)> {
    let uid = super::current_uid_gid().0;
    let runtime_dir = PathBuf::from(format!("/run/user/{uid}"));
    let bus = runtime_dir.join("bus");
    bus.exists()
        .then(|| (runtime_dir, format!("unix:path={}", bus.display())))
}

#[cfg(test)]
#[path = "systemd_tests.rs"]
mod tests;
