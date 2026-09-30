# cortex-sessions-watch.service Crash-Loop Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `cortex-sessions-watch.service` so a burst of transient crashes (e.g. SQLite lock contention) no longer leaves it permanently `failed` for days with zero alerting, and build a reusable periodic health-alert mechanism other beads in the parent epic can plug additional conditions into.

**Architecture:** Widen the systemd `StartLimitBurst`/`StartLimitIntervalSec` so a contention burst doesn't exhaust the restart budget. Add a new `sessions-watch-health-check` setup action that checks the service's systemd state and, if `failed`, sends an Apprise notification — driven by a new `cortex-sessions-watch-doctor.timer` installed alongside the watch service itself. The health-check function returns a list of named conditions (not just one hardcoded check) so bead `.2` (route-scoped ingest token fallback) can later add a second condition without new notification plumbing.

**Tech Stack:** Rust (existing `cortex` binary), systemd user units, the existing `AppriseClient` (`src/notifications/apprise.rs`), existing `systemctl_user_state`/`systemctl_user_phase` helpers (`src/setup/systemd.rs`).

## Global Constraints

- This bead has no dependencies on the rest of its parent epic (bead `syslog-mcp-8kkcn`, wave 1) and must not introduce any.
- Follow existing test conventions in `src/setup/sessions_watch_tests.rs`: fake `systemctl`/`curl`-style executables written to a temp dir and prepended to `PATH` via the existing `EnvGuard`/`write_executable`/`path_with_prepended` helpers already defined in that file — do not invent a new mocking approach.
- `cargo clippy --workspace --all-targets -- -D warnings` must pass after every task.
- No secrets in code or logs — Apprise URLs may contain credentials; never log them directly (existing `AppriseError::Transient` already calls `.without_url()` for this reason — follow that pattern).

---

### Task 1: Widen systemd restart-limit tolerance

**Files:**
- Modify: `src/setup/sessions_watch.rs:305-332` (`ai_watch_service_unit` function, the `StartLimitIntervalSec=300\nStartLimitBurst=5` line inside the format string)
- Test: `src/setup/sessions_watch_tests.rs` (new test, alongside existing `ai_watch_service_unit`-adjacent tests)

**Interfaces:**
- Consumes: nothing new
- Produces: `ai_watch_service_unit(...)` still returns `String` with the same signature; only the `StartLimitIntervalSec`/`StartLimitBurst` values embedded in the generated text change.

- [ ] **Step 1: Write the failing test**

Add to `src/setup/sessions_watch_tests.rs` (near the top-level tests, after the existing imports):

```rust
#[test]
fn ai_watch_service_unit_tolerates_contention_burst() {
    let cortex_bin = std::path::Path::new("/home/user/.local/bin/cortex");
    let env_path = std::path::Path::new("/home/user/.config/cortex/sessions-watch.env");
    let db_path = std::path::Path::new("/home/user/.cortex/data/cortex.db");
    let state_dir = std::path::Path::new("/home/user/.local/state/cortex");
    let user_home = std::path::Path::new("/home/user");

    let unit = ai_watch_service_unit(cortex_bin, env_path, db_path, state_dir, user_home);

    // A short 5-crash budget over 300s is exactly what caused the 2026-06-29
    // incident: a burst of transient lock-contention crashes exhausted the
    // limit and the unit stayed `failed` for 3 days with no auto-restart.
    // Widen the budget so a contention burst doesn't trip permanent failure.
    assert!(
        unit.contains("StartLimitBurst=20"),
        "expected StartLimitBurst=20, got unit:\n{unit}"
    );
    assert!(
        unit.contains("StartLimitIntervalSec=600"),
        "expected StartLimitIntervalSec=600, got unit:\n{unit}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib setup::sessions_watch::tests::ai_watch_service_unit_tolerates_contention_burst -- --nocapture`
Expected: FAIL — the current unit string contains `StartLimitBurst=5` and `StartLimitIntervalSec=300`, not the new values.

- [ ] **Step 3: Update the format string**

In `src/setup/sessions_watch.rs`, inside `ai_watch_service_unit` (around line 330), change:

```rust
        "[Unit]\nDescription=cortex real-time local AI transcript watch\nDocumentation=https://github.com/jmagar/cortex\nAfter=default.target\nStartLimitIntervalSec=300\nStartLimitBurst=5\n\n[Service]\n..."
```

to:

```rust
        "[Unit]\nDescription=cortex real-time local AI transcript watch\nDocumentation=https://github.com/jmagar/cortex\nAfter=default.target\nStartLimitIntervalSec=600\nStartLimitBurst=20\n\n[Service]\n..."
```

(Only the two numeric values change; everything else in the format string stays identical — keep the rest of the giant format string exactly as-is, just edit those two substrings in place.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib setup::sessions_watch::tests::ai_watch_service_unit_tolerates_contention_burst -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run the full existing sessions_watch test module to check for regressions**

Run: `cargo test --lib setup::sessions_watch::tests`
Expected: PASS — check specifically for any other test that asserts on the literal `StartLimitBurst=5`/`StartLimitIntervalSec=300` substrings (search first: `grep -n "StartLimitBurst\|StartLimitIntervalSec" src/setup/sessions_watch_tests.rs src/setup_tests.rs`). If any exist, update them to the new values — do not leave a second test contradicting Step 1's assertion.

- [ ] **Step 6: Commit**

```bash
git add src/setup/sessions_watch.rs src/setup/sessions_watch_tests.rs
git commit -m "fix: widen cortex-sessions-watch.service restart-limit tolerance

StartLimitBurst=5/StartLimitIntervalSec=300 caused the service to
exhaust its restart budget during the 2026-06-29 lock-contention
crash burst and stay permanently \`failed\` for 3 days with no
auto-restart. Widen to burst=20/interval=600s so transient
contention bursts self-heal instead of going silently dead."
```

---

### Task 2: Extract a reusable, multi-condition health-check phase

**Files:**
- Modify: `src/setup/sessions_watch.rs` (add new function)
- Test: `src/setup/sessions_watch_tests.rs`

**Interfaces:**
- Consumes: `systemctl_user_state(subcommand: &str, unit: &str) -> Option<String>` (already exists in `src/setup/systemd.rs`, already imported in `sessions_watch.rs` — see the `use super::systemd::{...}` block at the top of the file).
- Produces: `pub(crate) struct HealthCondition { pub name: &'static str, pub unhealthy: bool, pub detail: String }` and `pub(crate) fn sessions_watch_health_conditions() -> Vec<HealthCondition>`. Bead `.2` (route-scoped ingest token) will later append its own `HealthCondition` entries to this same `Vec` — the return type and field names defined here are load-bearing for that future bead, do not rename them without updating the epic's bead `.2` description.

- [ ] **Step 1: Write the failing test**

Add to `src/setup/sessions_watch_tests.rs`:

```rust
#[cfg(unix)]
#[test]
#[serial]
fn sessions_watch_health_conditions_flags_failed_service() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-sessions-watch.service*) printf 'failed\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let conditions = sessions_watch_health_conditions();

    let watch_condition = conditions
        .iter()
        .find(|c| c.name == "sessions-watch-service-failed")
        .expect("sessions-watch-service-failed condition present");
    assert!(watch_condition.unhealthy, "expected unhealthy: {watch_condition:?}");
    assert!(watch_condition.detail.contains("failed"));
}

#[cfg(unix)]
#[test]
#[serial]
fn sessions_watch_health_conditions_reports_healthy_when_active() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-sessions-watch.service*) printf 'active\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let conditions = sessions_watch_health_conditions();

    let watch_condition = conditions
        .iter()
        .find(|c| c.name == "sessions-watch-service-failed")
        .expect("sessions-watch-service-failed condition present");
    assert!(!watch_condition.unhealthy, "expected healthy: {watch_condition:?}");
}
```

Note: `HealthCondition` needs `#[derive(Debug)]` for the `{watch_condition:?}` formatting used in the test assertions above — include that derive when you define the struct in Step 3.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib setup::sessions_watch::tests::sessions_watch_health_conditions -- --nocapture`
Expected: FAIL with "cannot find function `sessions_watch_health_conditions`" / "cannot find type `HealthCondition`" (does not compile yet).

- [ ] **Step 3: Implement the health-condition types and function**

Add to `src/setup/sessions_watch.rs`, after `ai_index_timer_disabled_phase` (around line 217):

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib setup::sessions_watch::tests::sessions_watch_health_conditions -- --nocapture`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add src/setup/sessions_watch.rs src/setup/sessions_watch_tests.rs
git commit -m "feat: extract reusable multi-condition health-check for sessions-watch

HealthCondition/sessions_watch_health_conditions() checks
cortex-sessions-watch.service's systemd state and returns a
structured list of named conditions rather than a single bool, so
a later bead (route-scoped ingest token fallback) can append its
own condition without new notification plumbing."
```

---

### Task 3: Send an Apprise notification when a health condition is unhealthy

**Files:**
- Modify: `src/setup/sessions_watch.rs` (add new async function)
- Test: `src/setup/sessions_watch_tests.rs`

**Interfaces:**
- Consumes: `HealthCondition`/`sessions_watch_health_conditions()` from Task 2; `crate::notifications::apprise::{AppriseClient, NotifyType}` (existing, `AppriseClient::new(base_url).notify(urls, title, body, notify_type) -> Result<NotifyResponse, AppriseError>`).
- Produces: `pub(crate) async fn run_sessions_watch_health_check_and_notify(apprise_base_url: &str, apprise_urls: &[String]) -> SetupPhase` — the function bead `.2` and the new systemd timer (Task 4) both call.

- [ ] **Step 1: Write the failing test**

Add to `src/setup/sessions_watch_tests.rs`. This test needs a fake Apprise HTTP server — use a minimal `tokio::net::TcpListener` + `axum` router (already a dependency of this crate per `src/api.rs`) bound to `127.0.0.1:0`, matching how other async tests in this codebase likely spin up throwaway HTTP servers. If an existing test-server helper already exists elsewhere in the crate (check `grep -rn "TcpListener::bind" src --include=*_tests.rs` first), reuse it instead of writing a new one — otherwise use this minimal version:

```rust
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn health_check_and_notify_sends_apprise_alert_when_unhealthy() {
    use axum::{Router, routing::post};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-sessions-watch.service*) printf 'failed\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let notified = Arc::new(AtomicBool::new(false));
    let notified_clone = notified.clone();
    let app = Router::new().route(
        "/notify/",
        post(move || {
            let notified = notified_clone.clone();
            async move {
                notified.store(true, Ordering::SeqCst);
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://{addr}");
    let phase = run_sessions_watch_health_check_and_notify(
        &base_url,
        &["gotify://example.invalid/token".to_string()],
    )
    .await;

    assert_eq!(phase.status, SetupStatus::Error);
    assert!(notified.load(Ordering::SeqCst), "expected Apprise notify to fire");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn health_check_and_notify_skips_apprise_when_healthy() {
    use axum::{Router, routing::post};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-sessions-watch.service*) printf 'active\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let notified = Arc::new(AtomicBool::new(false));
    let notified_clone = notified.clone();
    let app = Router::new().route(
        "/notify/",
        post(move || {
            let notified = notified_clone.clone();
            async move {
                notified.store(true, Ordering::SeqCst);
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://{addr}");
    let phase = run_sessions_watch_health_check_and_notify(
        &base_url,
        &["gotify://example.invalid/token".to_string()],
    )
    .await;

    assert_eq!(phase.status, SetupStatus::Ok);
    assert!(!notified.load(Ordering::SeqCst), "expected no Apprise notify when healthy");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib setup::sessions_watch::tests::health_check_and_notify -- --nocapture`
Expected: FAIL with "cannot find function `run_sessions_watch_health_check_and_notify`" (does not compile yet).

- [ ] **Step 3: Implement the notify function**

Add to `src/setup/sessions_watch.rs`, after `sessions_watch_health_conditions` from Task 2:

```rust
/// Check all sessions-watch health conditions; if any are unhealthy, send
/// one Apprise notification summarizing them. Returns a SetupPhase so this
/// composes with the rest of the setup-report machinery (and so `cortex
/// setup doctor` can surface it alongside other checks).
pub(crate) async fn run_sessions_watch_health_check_and_notify(
    apprise_base_url: &str,
    apprise_urls: &[String],
) -> SetupPhase {
    let timer = PhaseTimer::start("sessions-watch-health-check");
    let conditions = sessions_watch_health_conditions();
    let unhealthy: Vec<&HealthCondition> = conditions.iter().filter(|c| c.unhealthy).collect();

    if unhealthy.is_empty() {
        return timer.finish(SetupStatus::Ok, "all sessions-watch health conditions healthy");
    }

    let body = unhealthy
        .iter()
        .map(|c| format!("- {}: {}", c.name, c.detail))
        .collect::<Vec<_>>()
        .join("\n");

    if !apprise_urls.is_empty() {
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib setup::sessions_watch::tests::health_check_and_notify -- --nocapture`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add src/setup/sessions_watch.rs src/setup/sessions_watch_tests.rs
git commit -m "feat: send Apprise alert when sessions-watch health check is unhealthy

run_sessions_watch_health_check_and_notify() checks all conditions
from sessions_watch_health_conditions() and fires one Apprise
notification (Warning severity) summarizing any unhealthy ones,
using the existing AppriseClient. No-ops silently when healthy."
```

---

### Task 4: Wire a periodic systemd timer that runs the health check, and auto-install it

**Files:**
- Create: `config/systemd/cortex-sessions-watch-doctor.service`
- Create: `config/systemd/cortex-sessions-watch-doctor.timer`
- Modify: `src/setup.rs` (add `SessionsWatchServiceAction::HealthCheck` variant)
- Modify: `src/main.rs` (wire the new CLI subcommand)
- Modify: `src/setup/sessions_watch.rs` (dispatch `HealthCheck` action; auto-enable the new timer during `Install`)
- Test: `src/setup/sessions_watch_tests.rs`

**Interfaces:**
- Consumes: `run_sessions_watch_health_check_and_notify` (Task 3), `NotificationsConfig { enabled: bool, apprise_url: String, apprise_urls: Vec<String> }` (existing, `src/config.rs:87-93`)
- Produces: `cortex setup sessions-watch-health-check [--json]` CLI command; `cortex-sessions-watch-doctor.timer` systemd unit auto-installed alongside `cortex-sessions-watch.service`.

- [ ] **Step 1: Write the failing test for the new setup action dispatch**

Add to `src/setup/sessions_watch_tests.rs`:

```rust
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn health_check_action_returns_ok_report_when_service_active() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-sessions-watch.service*) printf 'active\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));
    // No CORTEX_NOTIFICATIONS_APPRISE_URL(S) set — health check must not
    // require notifications config to run, only to fire an alert.
    let _enabled = EnvGuard::remove("CORTEX_NOTIFICATIONS_ENABLED");

    let report =
        run_sessions_watch_service_setup(SessionsWatchServiceAction::HealthCheck)
            .await
            .unwrap();

    assert!(!report.has_errors, "expected no errors, got: {report:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib setup::sessions_watch::tests::health_check_action_returns_ok_report -- --nocapture`
Expected: FAIL to compile — `SessionsWatchServiceAction::HealthCheck` does not exist yet.

- [ ] **Step 3: Add the `HealthCheck` variant and `as_str` arm**

In `src/setup.rs`, change (around line 76):

```rust
pub enum SessionsWatchServiceAction {
    Install,
    Remove,
    Check,
}
```

to:

```rust
pub enum SessionsWatchServiceAction {
    Install,
    Remove,
    Check,
    HealthCheck,
}
```

And update `impl SessionsWatchServiceAction` (around line 130):

```rust
impl SessionsWatchServiceAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "sessions-watch-service-install",
            Self::Remove => "sessions-watch-service-remove",
            Self::Check => "sessions-watch-service-check",
            Self::HealthCheck => "sessions-watch-service-health-check",
        }
    }
}
```

- [ ] **Step 4: Dispatch `HealthCheck` inside `run_sessions_watch_service_setup`**

In `src/setup/sessions_watch.rs`, add a new match arm inside `run_sessions_watch_service_setup` (after the existing `Check` arm, before the closing `}` of the `match action` block, around line 153):

```rust
        SessionsWatchServiceAction::HealthCheck => {
            let notifications = crate::config::Config::load()
                .map(|config| config.notifications)
                .unwrap_or_default();
            let apprise_urls: Vec<String> = if notifications.enabled {
                let mut urls = notifications.apprise_urls.clone();
                if !notifications.apprise_url.is_empty() {
                    urls.push(notifications.apprise_url.clone());
                }
                urls
            } else {
                Vec::new()
            };
            phases.push(
                run_sessions_watch_health_check_and_notify(
                    "http://127.0.0.1:8000",
                    &apprise_urls,
                )
                .await,
            );
        }
```

**Note for the implementer:** the `"http://127.0.0.1:8000"` Apprise base URL and the exact `Config::load()`/`config.notifications` field-access shape are placeholders for you to verify against the actual current signatures in `src/config.rs` before merging — this plan was written by reading `src/config.rs`'s `NotificationsConfig` struct fields (`enabled`, `apprise_url`, `apprise_urls`) but did NOT verify the exact Apprise *base URL* config field name/location, since that lives in a different config section not read during this planning pass. Grep `src/config.rs` for `apprise` case-insensitively and confirm the base-URL field before writing this code for real; do not ship the hardcoded `127.0.0.1:8000` literal.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib setup::sessions_watch::tests::health_check_action_returns_ok_report -- --nocapture`
Expected: PASS

- [ ] **Step 6: Wire the CLI subcommand**

In `src/main.rs`, find the `sessions-watch-service` subcommand block (around line 716-729) and duplicate the pattern for a new top-level `sessions-watch-health-check` subcommand. Insert immediately after that block:

```rust
    if matches!(
        iter.clone().next().map(String::as_str),
        Some("sessions-watch-health-check")
    ) {
        let _ = iter.next();
        let (_action, json) = parse_setup_subcommand_args("sessions-watch-health-check", iter)?;
        return Ok(SetupCommand {
            kind: SetupCommandKind::SessionsWatchService(
                cortex::setup::SessionsWatchServiceAction::HealthCheck,
            ),
            json,
        });
    }
```

**Note for the implementer:** verify `parse_setup_subcommand_args`'s exact signature (it's called with `"sessions-watch-service"` and `"sessions-index-timer"` elsewhere in this same function per the code read during planning — confirm it accepts an arbitrary `&str` label and doesn't validate against a fixed action-name list) before assuming this compiles as written; this plan did not trace that function's body.

- [ ] **Step 7: Create the systemd timer and oneshot service files**

Create `config/systemd/cortex-sessions-watch-doctor.service`:

```ini
# cortex-sessions-watch-doctor.service — periodic health check + Apprise
# alert for cortex-sessions-watch.service.
#
# Installed automatically by `cortex setup sessions-watch-service install`
# alongside cortex-sessions-watch.service itself. Prevents a repeat of the
# 2026-06-29 incident, where the watch service crashed into `failed` state
# and sat there for 3 days with zero alerting.

[Unit]
Description=cortex sessions-watch periodic health check

[Service]
Type=oneshot
ExecStart=%h/.local/bin/cortex setup sessions-watch-health-check --json

[Install]
WantedBy=default.target
```

Create `config/systemd/cortex-sessions-watch-doctor.timer`:

```ini
# cortex-sessions-watch-doctor.timer — runs the sessions-watch health
# check every 15 minutes.

[Unit]
Description=Periodic cortex-sessions-watch.service health check

[Timer]
OnCalendar=*:0/15
Persistent=true

[Install]
WantedBy=timers.target
```

- [ ] **Step 8: Auto-install the doctor timer during `sessions-watch-service install`**

In `src/setup/sessions_watch.rs`, inside `run_sessions_watch_service_setup`'s `SessionsWatchServiceAction::Install` arm, after the existing `phases.push(systemctl_user_required_named_phase(AI_WATCH_SERVICE_ACTIVE_PHASE, ...))` call (the last line of the `Install` arm, around line 101), add:

```rust
            phases.push(install_health_check_timer_files(&systemd_dir)?);
            phases.push(systemctl_user_phase(&["daemon-reload"]));
            phases.push(super::systemd::systemctl_user_required_phase(&[
                "enable",
                "--now",
                "cortex-sessions-watch-doctor.timer",
            ]));
```

And add the new helper function (near `install_ai_watch_service_files`, around line 243):

```rust
fn install_health_check_timer_files(systemd_dir: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("sessions-watch-doctor-timer-files");
    let service_content = include_str!("../../config/systemd/cortex-sessions-watch-doctor.service");
    let timer_content = include_str!("../../config/systemd/cortex-sessions-watch-doctor.timer");
    std::fs::write(
        systemd_dir.join("cortex-sessions-watch-doctor.service"),
        service_content,
    )?;
    std::fs::write(
        systemd_dir.join("cortex-sessions-watch-doctor.timer"),
        timer_content,
    )?;
    Ok(timer.finish(SetupStatus::Ok, "wrote cortex-sessions-watch-doctor service+timer"))
}
```

**Note for the implementer:** verify the `include_str!` relative path resolves correctly from `src/setup/sessions_watch.rs`'s location (`../../config/systemd/...`) — this plan computed it from the file's path (`src/setup/sessions_watch.rs` → up two levels reaches the repo root → `config/systemd/`) but did not compile-check it. Also add corresponding removal logic to the `SessionsWatchServiceAction::Remove` arm (disable+remove the doctor timer/service files, mirroring the existing `remove_ai_watch_service_files` pattern) — this plan's Step 8 only covers install; do not ship without a matching Remove path, write that as an additional sub-step here following the exact same pattern as `remove_ai_watch_service_files`.

- [ ] **Step 9: Run the full test suite for this module**

Run: `cargo test --lib setup::sessions_watch`
Expected: PASS, no regressions.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 10: Commit**

```bash
git add config/systemd/cortex-sessions-watch-doctor.service \
        config/systemd/cortex-sessions-watch-doctor.timer \
        src/setup.rs src/setup/sessions_watch.rs src/main.rs \
        src/setup/sessions_watch_tests.rs
git commit -m "feat: install periodic health-check timer for sessions-watch

Adds cortex-sessions-watch-doctor.{service,timer}, auto-installed
alongside cortex-sessions-watch.service, running
'cortex setup sessions-watch-health-check' every 15 minutes and
alerting via Apprise if the watch service is found failed. Closes
the observability gap from the 2026-06-29 incident where the
service died silently for 3 days."
```

---

## Post-plan note for the epic

This plan intentionally leaves two things for the implementer to verify against live code rather than guessing (flagged inline in Task 4, Steps 4/6/8) — the exact Apprise base-URL config field, `parse_setup_subcommand_args`'s signature, and the `include_str!` relative path. Everything else in this plan was verified against the actual current source during planning (read `src/setup/sessions_watch.rs`, `src/setup.rs`, `src/main.rs`, `src/notifications/apprise.rs`, `config/systemd/cortex-backup.{service,timer}`, and `src/setup/sessions_watch_tests.rs`'s existing test-mocking conventions in full before writing this plan).

Once this bead ships, bead `syslog-mcp-8kkcn.2` (route-scoped `CORTEX_INGEST_TOKEN` + TLS requirement) should append a second `HealthCondition` to `sessions_watch_health_conditions()` for "ingest client is using the CORTEX_API_TOKEN fallback" rather than inventing separate notification plumbing — that's the reuse this plan's Task 2 was designed to enable.
