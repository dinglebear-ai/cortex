use std::io::{self, ErrorKind};
use std::path::Path;

use super::systemd::{
    systemctl_user_phase, systemctl_user_required_named_phase, systemctl_user_required_phase,
    systemctl_user_state,
};
use super::{PhaseTimer, SetupPhase, SetupStatus, check_file_phase, setup_path_value};

/// One named health condition checked by the periodic sessions-watch
/// doctor alert. `unhealthy=true` means this condition should trigger a
/// notification. Bead .2 (route-scoped CORTEX_INGEST_TOKEN) appends its own
/// "using CORTEX_API_TOKEN fallback" condition to the Vec this function
/// returns — do not rename the fields without updating that bead's plan.
#[derive(Debug, Clone)]
pub(crate) struct HealthCondition {
    pub name: &'static str,
    pub unhealthy: bool,
    pub detail: String,
}

/// Collect all health conditions relevant to `cortex-sessions-watch.service`.
/// Returns one entry per condition regardless of health, so callers can log
/// or notify on the full set, not just the unhealthy ones.
pub(crate) fn sessions_watch_health_conditions() -> Vec<HealthCondition> {
    let active = systemctl_user_state("is-active", "cortex-sessions-watch.service");
    let is_failed = active.as_deref() == Some("failed");
    vec![HealthCondition {
        name: "sessions-watch-service-failed",
        unhealthy: is_failed,
        detail: format!("cortex-sessions-watch.service is-active={active:?}"),
    }]
}

/// Check all sessions-watch health conditions; if any are unhealthy, send
/// one Apprise notification summarizing them. Returns a SetupPhase so this
/// composes with the rest of the setup-report machinery.
///
/// When `apprise_urls` is empty (notifications disabled/unconfigured), this
/// still returns `SetupStatus::Error` on unhealthy conditions so `--json`
/// output and `cortex setup doctor` are never silent about it — only the
/// push notification itself is skipped.
pub(crate) async fn run_sessions_watch_health_check_and_notify(
    apprise_base_url: &str,
    apprise_urls: &[String],
) -> SetupPhase {
    let timer = PhaseTimer::start("sessions-watch-health-check");
    let conditions = sessions_watch_health_conditions();
    let unhealthy: Vec<&HealthCondition> = conditions.iter().filter(|c| c.unhealthy).collect();

    if unhealthy.is_empty() {
        return timer.finish(
            SetupStatus::Ok,
            "all sessions-watch health conditions healthy",
        );
    }

    let body = unhealthy
        .iter()
        .map(|c| format!("- {}: {}", c.name, c.detail))
        .collect::<Vec<_>>()
        .join("\n");

    if apprise_urls.is_empty() {
        tracing::warn!(
            conditions = %body,
            "sessions-watch health alert: notifications disabled/unconfigured, no Apprise notify sent"
        );
    } else {
        let client = crate::notifications::apprise::AppriseClient::new(apprise_base_url);
        if let Err(error) = client
            .notify(
                apprise_urls,
                "cortex sessions-watch unhealthy",
                &body,
                crate::notifications::apprise::NotifyType::Warning,
            )
            .await
        {
            tracing::warn!(error = %error, "sessions-watch health alert: Apprise notify failed");
        }
    }

    timer.finish(
        SetupStatus::Error,
        format!("unhealthy conditions detected:\n{body}"),
    )
}

/// Generate the `cortex-sessions-watch-doctor.service` unit content.
///
/// Templated the same way `ai_watch_service_unit` templates its sibling —
/// `ExecStart` uses the resolved `cortex_bin` (not a hardcoded `%h/.local/bin`
/// literal) so it works regardless of install layout, and the hardening
/// directives match the watch service it doctors (`NoNewPrivileges`,
/// `PrivateTmp`, `ProtectSystem=strict`, `ProtectHome=read-only`, `UMask`).
/// No `BindPaths`/`ReadWritePaths` are needed — this unit only reads config
/// and makes an outbound HTTP call, it never touches the DB or state dirs
/// directly.
fn doctor_service_unit(cortex_bin: &Path) -> io::Result<String> {
    let cortex_bin = setup_path_value(cortex_bin)?;
    Ok(format!(
        "[Unit]\nDescription=cortex sessions-watch periodic health check\nDocumentation=https://github.com/dinglebear-ai/cortex\n\n[Service]\nType=oneshot\nExecStart={cortex_bin} setup sessionshealth --json\nUMask=0077\nNoNewPrivileges=true\nPrivateTmp=true\nProtectSystem=strict\nProtectHome=read-only\n\n[Install]\nWantedBy=default.target\n"
    ))
}

/// Generate the `cortex-sessions-watch-doctor.timer` unit content.
///
/// Inlined as a Rust string literal — like `doctor_service_unit` and the
/// sibling `ai_watch_service_unit` — rather than `include_str!`'d from a
/// checked-in file, because the Docker build context only `COPY`s specific
/// directories (see `config/Dockerfile`) and does not include `config/systemd/`,
/// so an `include_str!` reference to that path fails to compile in the
/// container image build even though it compiles fine locally.
fn doctor_timer_unit() -> &'static str {
    "[Unit]\nDescription=Periodic cortex-sessions-watch.service health check\n\n[Timer]\nOnCalendar=*:0/15\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
}

fn install_health_check_timer_files(
    systemd_dir: &Path,
    cortex_bin: &Path,
) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("sessions-watch-doctor-timer-files");
    let service_content = doctor_service_unit(cortex_bin)?;
    let timer_content = doctor_timer_unit();
    std::fs::create_dir_all(systemd_dir)?;
    std::fs::write(
        systemd_dir.join("cortex-sessions-watch-doctor.service"),
        service_content,
    )?;
    std::fs::write(
        systemd_dir.join("cortex-sessions-watch-doctor.timer"),
        timer_content,
    )?;
    Ok(timer.finish(
        SetupStatus::Ok,
        "wrote cortex-sessions-watch-doctor service+timer",
    ))
}

/// Verify the installed doctor timer/service files match the expected
/// generated content. Mirrors `check_ai_watch_service_content_phase` for the
/// sibling watch service — without this, `cortex setup sessionswatch
/// check`/`cortex setup doctor` could report healthy while the alerting
/// mechanism itself is silently missing or stale, reintroducing a milder
/// version of the blind spot this feature exists to close.
fn check_doctor_service_content_phase(systemd_dir: &Path, cortex_bin: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("sessions-watch-doctor-service-content");
    let service_path = systemd_dir.join("cortex-sessions-watch-doctor.service");
    let timer_path = systemd_dir.join("cortex-sessions-watch-doctor.timer");
    let expected_service = match doctor_service_unit(cortex_bin) {
        Ok(content) => content,
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    let expected_timer = doctor_timer_unit();
    let current_service = match std::fs::read_to_string(&service_path) {
        Ok(raw) => raw,
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    let current_timer = match std::fs::read_to_string(&timer_path) {
        Ok(raw) => raw,
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    if current_service != expected_service {
        return timer.finish(
            SetupStatus::Error,
            format!(
                "{} does not match generated doctor service unit",
                service_path.display()
            ),
        );
    }
    if current_timer != expected_timer {
        return timer.finish(
            SetupStatus::Error,
            format!(
                "{} does not match generated doctor timer unit",
                timer_path.display()
            ),
        );
    }
    timer.finish(
        SetupStatus::Ok,
        "sessions-watch-doctor service+timer files match generated content",
    )
}

/// All `SessionsWatchServiceAction::Check` phases for the doctor timer:
/// file presence, generated-content match, and systemd enabled/active
/// state. Mirrors the equivalent checks the watch service already has.
pub(super) fn check_doctor_timer_phases(systemd_dir: &Path, cortex_bin: &Path) -> Vec<SetupPhase> {
    vec![
        check_file_phase(
            "sessions-watch-doctor-service",
            &systemd_dir.join("cortex-sessions-watch-doctor.service"),
            "run cortex setup sessionswatch install",
        ),
        check_file_phase(
            "sessions-watch-doctor-timer",
            &systemd_dir.join("cortex-sessions-watch-doctor.timer"),
            "run cortex setup sessionswatch install",
        ),
        check_doctor_service_content_phase(systemd_dir, cortex_bin),
        systemctl_user_required_named_phase(
            "sessions-watch-doctor-timer-enabled",
            &["is-enabled", "cortex-sessions-watch-doctor.timer"],
        ),
        systemctl_user_required_named_phase(
            "sessions-watch-doctor-timer-active",
            &["is-active", "cortex-sessions-watch-doctor.timer"],
        ),
    ]
}

/// Write the doctor timer/service files and enable the timer. Called from
/// `SessionsWatchServiceAction::Install` after the watch service itself is
/// enabled+active.
pub(super) fn install_and_enable_doctor_timer(
    systemd_dir: &Path,
    cortex_bin: &Path,
) -> io::Result<Vec<SetupPhase>> {
    Ok(vec![
        install_health_check_timer_files(systemd_dir, cortex_bin)?,
        systemctl_user_phase(&["daemon-reload"]),
        systemctl_user_required_phase(&["enable", "--now", "cortex-sessions-watch-doctor.timer"]),
    ])
}

/// Disable the doctor timer and remove its files. Called from
/// `SessionsWatchServiceAction::Remove`.
pub(super) fn disable_and_remove_doctor_timer(systemd_dir: &Path) -> io::Result<Vec<SetupPhase>> {
    Ok(vec![
        systemctl_user_phase(&["disable", "--now", "cortex-sessions-watch-doctor.timer"]),
        remove_health_check_timer_files(systemd_dir)?,
    ])
}

/// Resolve notification config and run the health check + alert. Called
/// from `SessionsWatchServiceAction::HealthCheck`.
pub(super) async fn build_and_run_health_check() -> SetupPhase {
    let notifications = match crate::config::Config::load() {
        Ok(config) => config.notifications,
        Err(error) => {
            // Do not fall through to the "notifications disabled" path
            // silently -- a config-load failure (malformed config.toml, a
            // bad CORTEX_* env var) is a distinct, actionable operator
            // error, not the same thing as "notifications intentionally
            // turned off". Collapsing the two here would reproduce the
            // exact silent-failure shape this feature exists to close.
            tracing::error!(
                error = %error,
                "sessions-watch health check: failed to load config, cannot resolve notification settings"
            );
            crate::config::NotificationsConfig::default()
        }
    };
    let apprise_urls: Vec<String> = if notifications.enabled {
        notifications.apprise_urls.clone()
    } else {
        Vec::new()
    };
    run_sessions_watch_health_check_and_notify(&notifications.apprise_url, &apprise_urls).await
}

fn remove_health_check_timer_files(systemd_dir: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("sessions-watch-doctor-timer-files");
    for name in [
        "cortex-sessions-watch-doctor.service",
        "cortex-sessions-watch-doctor.timer",
    ] {
        match std::fs::remove_file(systemd_dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(timer.finish(
        SetupStatus::Ok,
        "removed cortex-sessions-watch-doctor service+timer files",
    ))
}

#[cfg(test)]
#[path = "sessions_watch_health_tests.rs"]
mod tests;
