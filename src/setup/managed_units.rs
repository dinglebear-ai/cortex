use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use super::{PhaseTimer, SetupPhase, SetupStatus};

pub(crate) fn rewrite_stale_managed_unit_commands(systemd_dir: &Path) -> io::Result<SetupPhase> {
    const MANAGED_UNITS: &[&str] = &[
        "cortex-sessions-watch.service",
        "cortex-sessions-watch-doctor.service",
        "cortex-sessions-index.service",
        "cortex-heartbeat-agent.service",
    ];
    const COMMAND_REWRITES: &[(&str, &str)] = &[(
        " setup sessions-watch-health-check ",
        " setup sessionshealth ",
    )];

    let timer = PhaseTimer::start("managed-unit-command-repair");
    let mut rewritten = 0_usize;
    for name in MANAGED_UNITS {
        let path = systemd_dir.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("managed unit must be a regular file: {}", path.display()),
            ));
        }

        let current = std::fs::read_to_string(&path)?;
        let mut changed = false;
        let repaired = current
            .lines()
            .map(|line| {
                if !line.trim_start().starts_with("ExecStart=") {
                    return line.to_string();
                }
                let mut repaired = line.to_string();
                for (stale, canonical) in COMMAND_REWRITES {
                    if repaired.contains(stale) {
                        repaired = repaired.replace(stale, canonical);
                        changed = true;
                    }
                }
                repaired
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !changed {
            continue;
        }

        let repaired = if current.ends_with('\n') {
            format!("{repaired}\n")
        } else {
            repaired
        };
        let temp = path.with_extension("service.repair.tmp");
        std::fs::write(&temp, repaired)?;
        std::fs::set_permissions(&temp, metadata.permissions())?;
        std::fs::rename(&temp, &path)?;
        rewritten += 1;
    }

    Ok(timer.finish(
        SetupStatus::Ok,
        format!("rewrote {rewritten} managed unit command(s)"),
    ))
}

pub(crate) fn cleanup_legacy_systemd() -> SetupPhase {
    let timer = PhaseTimer::start("legacy-systemd");
    let home = match crate::env::var("HOME") {
        Ok(home) => PathBuf::from(home),
        Err(_) => {
            return timer.finish(SetupStatus::Skipped, "HOME unset");
        }
    };
    let mut failures = Vec::new();
    for unit in ["cortex.service", "mnemo-index.service", "mnemo-index.timer"] {
        match super::systemd::run_systemctl_user(&["disable", "--now", unit]) {
            Ok(output) if output.status.success() => {}
            Ok(output) => failures.push(format!(
                "systemctl disable --now {unit}: {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("failed")
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("systemctl disable --now {unit}: {error}")),
        }
    }
    for name in ["cortex.service", "mnemo-index.service", "mnemo-index.timer"] {
        let unit = home.join(".config/systemd/user").join(name);
        let dropins = home.join(".config/systemd/user").join(format!("{name}.d"));
        if let Err(error) = std::fs::remove_file(&unit)
            && error.kind() != ErrorKind::NotFound
        {
            failures.push(format!("remove {}: {error}", unit.display()));
        }
        if let Err(error) = std::fs::remove_dir_all(&dropins)
            && error.kind() != ErrorKind::NotFound
        {
            failures.push(format!("remove {}: {error}", dropins.display()));
        }
    }
    match super::systemd::run_systemctl_user(&["daemon-reload"]) {
        Ok(output) if output.status.success() => {}
        Ok(output) => failures.push(format!(
            "systemctl daemon-reload: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("failed")
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => failures.push(format!("systemctl daemon-reload: {error}")),
    }
    if !failures.is_empty() {
        return timer.finish(SetupStatus::Warn, failures.join("; "));
    }
    timer.finish(
        SetupStatus::Ok,
        "removed stale cortex and mnemo-index user units/drop-ins if present",
    )
}
