# ai watch-status Service Layer Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `ai_watch_status` host probing and `AiWatchStatusReport` from `src/cli/ai_watch.rs` into the service layer so CLI remains a thin adapter.

**Architecture:** Extend `OsAdapter` with `probe_command` (non-zero exit OK) so all systemctl probes are mockable; create `src/app/watch_status.rs` holding an `impl SyslogService` block with `pub async fn ai_watch_status(&self)`; move `AiWatchStatusReport` to `src/app/models.rs`; update CLI to call the service method directly.

**Tech Stack:** Rust 1.86, `tokio`, `anyhow`, `serde`, `libc`, `tracing`, `syslog_mcp::app::{OsAdapter, SyslogService, ServiceError, ServiceResult}`

---

## Background and Constraints

### What must NOT change

- `systemctl_user_output()` stays in `src/cli/ai_watch.rs` — `src/cli/coordination.rs` imports it via `super::ai_watch::systemctl_user_output`. Removing it breaks `compose doctor` and `db status --check-coord`.
- The HTTP guard (`bail!("ai watch-status shells out to systemctl on host; omit --http")`) stays in `src/cli/dispatch_ai.rs` around line 360. Per eng-review rule S4, LOCAL-only commands reject HTTP in the CLI dispatch layer, not in the service.
- `src/cli/ai_watch_tests.rs` tests `smoke_watch_target*` — unrelated to `ai_watch_status`. Leave them alone.
- `src/cli/dispatch_tests.rs` tests that `run_ai_watch_status` bails on HTTP — this test stays valid after the refactor.

### How systemctl probes route through `OsAdapter`

`OsAdapter::run_command` errors on any non-zero exit code. `systemctl --user is-active` exits non-zero AND writes the unit state ("inactive", "failed") to stdout when the unit exists but is not running. We solve this by adding a second trait method, `probe_command`, which returns `ServiceResult<std::process::Output>` and treats non-zero exit as `Ok` — the caller inspects `output.status` and `output.stdout` directly.

This makes all systemctl probes mockable via `MockOsAdapter`. `journalctl` continues to use `run_command` (success required; graceful degradation happens at the call site with `.unwrap_or_default()`).

### Decoupled execution order

All systemctl probes run first (they are OS-only, no DB dependency). The `ai_doctor()` and `ai_indexing_health()` DB calls come after, ensuring the operator still gets systemctl state even if the DB is unavailable during an outage.

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src/app/os_adapter.rs` | **Modify** | Add `probe_command` to trait; implement in `SystemOsAdapter`; promote helpers to `pub(crate)` |
| `src/app/watch_status.rs` | **Create** | `impl SyslogService` block with `pub async fn ai_watch_status(&self)` |
| `src/app/models.rs` | **Modify** | Add `pub struct AiWatchStatusReport` |
| `src/app.rs` | **Modify** | Declare `mod watch_status;`; re-export `AiWatchStatusReport` |
| `src/app/service.rs` | **Modify** | Add `AiWatchStatusReport` to local imports |
| `src/cli/ai_watch.rs` | **Modify** | Remove `AiWatchStatusReport`, `ai_watch_status()`, `command_output()` (keep `systemctl_user_output` and its private helpers) |
| `src/cli/output_ai.rs` | **Modify** | Update `AiWatchStatusReport` import to `syslog_mcp::app::AiWatchStatusReport` |
| `src/cli/dispatch_ai.rs` | **Modify** | Remove `ai_watch_status` import; call `service.ai_watch_status().await` directly |

---

## Task 1: Add `AiWatchStatusReport` to `src/app/models.rs`

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app.rs`
- Modify: `src/cli/ai_watch.rs` (remove struct definition)
- Modify: `src/cli/output_ai.rs` (update import)

- [ ] **Step 1.1: Add struct to `src/app/models.rs`**

The struct needs `pub` visibility and must derive `Serialize` (already imported at top of models.rs). Add it after the `ServiceJournalEntry` block (around line 192). `AiIndexingHealth` is from `syslog_mcp::scanner`.

```rust
// In src/app/models.rs, after ServiceJournalEntry:

#[derive(Debug, Clone, serde::Serialize)]
pub struct AiWatchStatusReport {
    pub service: String,
    pub active: Option<String>,
    pub enabled: Option<String>,
    pub main_pid: Option<u32>,
    pub exec_start: Option<String>,
    pub exec_main_start_timestamp: Option<String>,
    pub process_start_time: Option<String>,
    pub db_path: String,
    pub health: crate::scanner::AiIndexingHealth,
    pub latest_journal: Vec<String>,
}
```

- [ ] **Step 1.2: Re-export from `src/app.rs`**

Find the `pub use models::{` block in `src/app.rs` and add `AiWatchStatusReport` to it, keeping the list alphabetically sorted:

```rust
// In the pub use models::{ block, add:
AiWatchStatusReport,
```

- [ ] **Step 1.3: Remove the `pub(crate)` struct from `src/cli/ai_watch.rs`**

Delete the `AiWatchStatusReport` struct definition (lines 6-18). The struct now lives in `src/app/models.rs`.

- [ ] **Step 1.4: Update `src/cli/output_ai.rs` import**

Change:
```rust
use super::ai_watch::{AiSmokeWatchReport, AiWatchStatusReport};
```
To:
```rust
use super::ai_watch::AiSmokeWatchReport;
use syslog_mcp::app::AiWatchStatusReport;
```

- [ ] **Step 1.5: Compile check**

```bash
rtk cargo check 2>&1 | grep "error\[" | head -20
```

Expected: 0 errors. Fix any type-path errors before proceeding.

- [ ] **Step 1.6: Commit**

```bash
rtk git add src/app/models.rs src/app.rs src/cli/ai_watch.rs src/cli/output_ai.rs
rtk git commit -m "refactor: promote AiWatchStatusReport to app/models.rs"
```

---

## Task 2: Extend `OsAdapter` with `probe_command`

**Files:**
- Modify: `src/app/os_adapter.rs`

This task adds the `probe_command` trait method (non-zero exit is `Ok`) and promotes the private D-Bus helpers to `pub(crate)` so `watch_status.rs` can use them without a 5th private copy.

- [ ] **Step 2.1: Promote `inferred_user_bus_env` and `current_uid` to `pub(crate)`**

In `src/app/os_adapter.rs`, change both function signatures from private to `pub(crate)`:

```rust
// Change:
fn inferred_user_bus_env() -> Option<(PathBuf, String)> {
// To:
pub(crate) fn inferred_user_bus_env() -> Option<(PathBuf, String)> {
```

```rust
// Change:
fn current_uid() -> u32 {
// To:
pub(crate) fn current_uid() -> u32 {
```

- [ ] **Step 2.2: Add `probe_command` to the `OsAdapter` trait**

After the `run_command` declaration in the trait block, add:

```rust
    /// Run `program` with `args` and return the raw `Output`.
    ///
    /// Unlike [`run_command`], a non-zero exit code is **not** an error —
    /// the caller inspects `output.status` and `output.stdout` directly.
    /// Use this for commands like `systemctl is-active` that write meaningful
    /// output to stdout even when they exit non-zero.
    ///
    /// Implementations must apply a reasonable execution timeout.
    fn probe_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ServiceResult<std::process::Output>> + Send + 'a>,
    >;
```

- [ ] **Step 2.3: Implement `probe_command` in `SystemOsAdapter`**

After the `run_command` `impl` block for `SystemOsAdapter`, add the `probe_command` implementation. It mirrors `run_command` but skips the non-zero exit check and applies D-Bus env for all callers (systemctl --user needs it regardless of program name):

```rust
    fn probe_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ServiceResult<std::process::Output>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut command = Command::new(program);
            command.args(args).kill_on_drop(true);

            if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
                if let Some((runtime_dir, bus_address)) = inferred_user_bus_env() {
                    command
                        .env("XDG_RUNTIME_DIR", runtime_dir)
                        .env("DBUS_SESSION_BUS_ADDRESS", bus_address);
                }
            }

            tokio::time::timeout(COMMAND_TIMEOUT, command.output())
                .await
                .map_err(|_| {
                    ServiceError::Internal(anyhow::anyhow!(
                        "{} {} timed out after {}s",
                        program,
                        args.join(" "),
                        COMMAND_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(anyhow::Error::from)
                .map_err(ServiceError::Internal)
        })
    }
```

- [ ] **Step 2.4: Compile check**

```bash
rtk cargo check 2>&1 | grep "error\[" | head -20
```

Expected: 0 errors.

- [ ] **Step 2.5: Commit**

```bash
rtk git add src/app/os_adapter.rs
rtk git commit -m "feat: add OsAdapter::probe_command for non-zero-ok shell probes"
```

---

## Task 3: Create `src/app/watch_status.rs` with `impl SyslogService`

**Files:**
- Create: `src/app/watch_status.rs`
- Modify: `src/app.rs` (declare module)
- Modify: `src/app/service.rs` (add import)

The method lives in an `impl SyslogService` block in `watch_status.rs` — not as a free function — so it accesses `self.os` and `self.storage.db_path` directly without inverted ownership. Systemctl probes run before DB calls so operator gets host state even if the DB is down.

- [ ] **Step 3.1: Create `src/app/watch_status.rs`**

```rust
//! ai watch-status host probing — service-layer implementation.
//!
//! All systemctl probes route through `self.os.probe_command()` and are
//! mockable in tests via `SyslogService::with_os_adapter()`. The D-Bus env
//! setup lives in `SystemOsAdapter::probe_command` so this module stays clean.
//!
//! journalctl uses `self.os.run_command()` (success required); failures
//! degrade to an empty vec — `.unwrap_or_default()` semantics preserved.
//!
//! Execution order: systemctl probes first (OS-only), then DB calls.
//! This ensures the operator receives host state even during DB outages.

use tracing::warn;

use super::models::AiWatchStatusReport;
use super::{ServiceError, ServiceResult};
use crate::app::service::SyslogService;

const SERVICE: &str = "syslog-ai-watch.service";

impl SyslogService {
    /// Collect the ai watch-status report.
    ///
    /// # Errors
    ///
    /// Returns `ServiceError` only if `ai_indexing_health` fails. Systemctl
    /// and journalctl failures degrade gracefully (fields become `None` / empty
    /// vec). `ai_doctor` failure is logged and treated as a degraded state.
    pub async fn ai_watch_status(&self) -> ServiceResult<AiWatchStatusReport> {
        // --- Systemctl probes (OS-only, no DB dependency) ---
        let active = self.probe_systemctl(&["is-active", SERVICE]).await;
        let enabled = self.probe_systemctl(&["is-enabled", SERVICE]).await;
        let main_pid = self
            .probe_systemctl(&["show", "-p", "MainPID", "--value", SERVICE])
            .await
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&pid| pid > 0);
        let exec_start =
            self.probe_systemctl(&["show", "-p", "ExecStart", "--value", SERVICE])
                .await;
        let exec_main_start_timestamp = self
            .probe_systemctl(&[
                "show",
                "-p",
                "ExecMainStartTimestamp",
                "--value",
                SERVICE,
            ])
            .await;

        // --- Process start time (procfs, no DB) ---
        let process_start_time = crate::doctor::ai_watcher_process_start_time();

        // --- DB calls (after OS probes so a DB outage doesn't block host info) ---
        let health = self
            .ai_indexing_health(process_start_time.clone())
            .await
            .map_err(|e| {
                warn!(error = %e, "ai_indexing_health failed; propagating error");
                e
            })?;

        // --- journalctl via run_command (degrade to empty on failure) ---
        let journal_args: Vec<String> = [
            "--user", "-u", SERVICE, "-n", "10", "--no-pager", "--output", "short-iso",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let latest_journal = match self.os.run_command("journalctl", &journal_args).await {
            Ok(raw) => raw.lines().map(str::to_string).collect(),
            Err(e) => {
                warn!(service = SERVICE, error = %e, "journalctl probe failed; latest_journal will be empty");
                Vec::new()
            }
        };

        let db_path = self.storage.db_path.display().to_string();

        Ok(AiWatchStatusReport {
            service: SERVICE.to_string(),
            active,
            enabled,
            main_pid,
            exec_start,
            exec_main_start_timestamp,
            process_start_time,
            db_path,
            health,
            latest_journal,
        })
    }

    /// Call `systemctl --user <args>` via `probe_command` and return trimmed
    /// stdout, or `None` when the command fails and stdout is empty.
    /// Non-zero exit with non-empty stdout (e.g., "inactive") is treated as
    /// success — that is the systemctl is-active contract.
    async fn probe_systemctl(&self, args: &[&str]) -> Option<String> {
        let args_owned: Vec<String> = std::iter::once("--user")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect();

        match self.os.probe_command("systemctl", &args_owned).await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() || !stdout.is_empty() {
                    Some(stdout)
                } else {
                    let stderr =
                        String::from_utf8_lossy(&output.stderr).trim().to_string();
                    warn!(
                        args = ?args,
                        stderr = %stderr,
                        "systemctl probe failed with empty stdout"
                    );
                    None
                }
            }
            Err(e) => {
                warn!(args = ?args, error = %e, "systemctl probe_command error");
                None
            }
        }
    }
}
```

- [ ] **Step 3.2: Declare the module in `src/app.rs`**

Add `mod watch_status;` after `mod service;`:

```rust
mod watch_status;
```

- [ ] **Step 3.3: Add `AiWatchStatusReport` to service.rs imports**

Find the `use super::models::{` block in `src/app/service.rs` and add `AiWatchStatusReport`:

```rust
// In the use super::models::{ block, add:
AiWatchStatusReport,
```

(This is needed if any other code in service.rs references the type. If the import block doesn't exist, this is a no-op — `watch_status.rs` imports it directly from `super::models`.)

- [ ] **Step 3.4: Compile check**

```bash
rtk cargo check 2>&1 | grep "error\[" | head -20
```

Expected: 0 errors. Common issues:
- `self.storage.db_path` — verify the field name against `StorageConfig`. If it's a different name (e.g. `path` or `db`), update accordingly.
- `crate::doctor::ai_watcher_process_start_time` — verify this function exists and returns `Option<String>`.
- Circular import: `watch_status.rs` imports from `crate::app::service::SyslogService`. If this causes a cycle, change to `use super::service::SyslogService;`.

- [ ] **Step 3.5: Write service-layer tests**

Add `src/app/watch_status_tests.rs` (sidecar test file). These tests verify the `MockOsAdapter` path — no real systemctl or journalctl invoked.

```rust
// src/app/watch_status_tests.rs
use std::process::{ExitStatus, Output};
use std::sync::Arc;

use crate::app::os_adapter::OsAdapter;
use crate::app::{ServiceError, ServiceResult};
use crate::app::service::SyslogService;
use crate::config::StorageConfig;
use crate::db::init_pool;

// Mock that tracks which commands were called and returns configured outputs.
struct MockProbeOs {
    journal_output: String,
    probe_stdout: String,
    probe_success: bool,
}

impl OsAdapter for MockProbeOs {
    fn run_command<'a>(
        &'a self,
        _program: &'a str,
        _args: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ServiceResult<String>> + Send + 'a>>
    {
        let output = self.journal_output.clone();
        Box::pin(async move { Ok(output) })
    }

    fn probe_command<'a>(
        &'a self,
        _program: &'a str,
        _args: &'a [String],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = ServiceResult<Output>> + Send + 'a,
        >,
    > {
        let stdout = self.probe_stdout.as_bytes().to_vec();
        let success = self.probe_success;
        Box::pin(async move {
            Ok(Output {
                status: if success {
                    std::process::Command::new("true").status().unwrap()
                } else {
                    std::process::Command::new("false").status().unwrap()
                },
                stdout,
                stderr: vec![],
            })
        })
    }
}

struct FailingJournalOs;

impl OsAdapter for FailingJournalOs {
    fn run_command<'a>(
        &'a self,
        _program: &'a str,
        _args: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ServiceResult<String>> + Send + 'a>>
    {
        Box::pin(async move {
            Err(ServiceError::Internal(anyhow::anyhow!("journalctl not found")))
        })
    }

    fn probe_command<'a>(
        &'a self,
        _program: &'a str,
        _args: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ServiceResult<std::process::Output>> + Send + 'a>,
    > {
        Box::pin(async move {
            Ok(std::process::Output {
                status: std::process::Command::new("true").status().unwrap(),
                stdout: b"inactive\n".to_vec(),
                stderr: vec![],
            })
        })
    }
}

#[tokio::test]
async fn ai_watch_status_returns_journal_lines_from_os_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    let os = Arc::new(MockProbeOs {
        journal_output: "May 26 10:00:00 host syslog-ai-watch[123]: started\nMay 26 10:01:00 host syslog-ai-watch[123]: indexed 5 files\n".to_string(),
        probe_stdout: "active\n".to_string(),
        probe_success: true,
    });
    let service = SyslogService::with_os_adapter(pool, storage, os);

    let report = service.ai_watch_status().await.unwrap();

    assert_eq!(report.service, "syslog-ai-watch.service");
    assert_eq!(report.latest_journal.len(), 2);
    assert!(report.latest_journal[0].contains("started"));
    assert_eq!(report.active.as_deref(), Some("active"));
}

#[tokio::test]
async fn ai_watch_status_degrades_gracefully_when_journalctl_fails() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    let os = Arc::new(FailingJournalOs);
    let service = SyslogService::with_os_adapter(pool, storage, os);

    let report = service.ai_watch_status().await.unwrap();

    // journalctl failure degrades to empty vec, not a hard error
    assert!(report.latest_journal.is_empty());
    assert_eq!(report.service, "syslog-ai-watch.service");
    // systemctl probe still returns "inactive" from the mock
    assert_eq!(report.active.as_deref(), Some("inactive"));
}
```

- [ ] **Step 3.6: Wire sidecar test file**

Add the sidecar test declaration to the bottom of `src/app/watch_status.rs`:

```rust
#[cfg(test)]
#[path = "../app/watch_status_tests.rs"]
mod tests;
```

- [ ] **Step 3.7: Run the new tests**

```bash
rtk cargo test watch_status 2>&1 | tail -15
```

Expected: `2 passed, 0 failed`. If `ai_indexing_health` panics on empty DB, wrap in `.unwrap_or_else(|_| AiIndexingHealth::default())` — check if `AiIndexingHealth` derives `Default` first.

- [ ] **Step 3.8: Commit**

```bash
rtk git add src/app/watch_status.rs src/app/watch_status_tests.rs src/app.rs src/app/service.rs
rtk git commit -m "feat: add SyslogService::ai_watch_status() in app/watch_status.rs"
```

---

## Task 4: Update CLI dispatch to call the service method

**Files:**
- Modify: `src/cli/dispatch_ai.rs`
- Modify: `src/cli/ai_watch.rs`

- [ ] **Step 4.1: Update `run_ai_watch_status` in `src/cli/dispatch_ai.rs`**

Current (around line 360):
```rust
use super::ai_watch::{ai_smoke_watch, ai_watch_status};
// ...
pub(crate) async fn run_ai_watch_status(mode: &CliMode, args: OutputArgs) -> Result<()> {
    if matches!(mode, CliMode::Http(_)) {
        bail!("ai watch-status shells out to systemctl on host; omit --http");
    }
    let CliMode::Local(service) = mode else {
        unreachable!("http mode returned above");
    };
    let response = ai_watch_status(service).await?;
    print_ai_watch_status_response(&response, args.json)
}
```

Change to (remove `ai_watch_status` from the import, call service method directly):
```rust
use super::ai_watch::ai_smoke_watch;
// ...
pub(crate) async fn run_ai_watch_status(mode: &CliMode, args: OutputArgs) -> Result<()> {
    if matches!(mode, CliMode::Http(_)) {
        bail!("ai watch-status shells out to systemctl on host; omit --http");
    }
    let CliMode::Local(service) = mode else {
        unreachable!("http mode returned above");
    };
    let response = service.ai_watch_status().await?;
    print_ai_watch_status_response(&response, args.json)
}
```

- [ ] **Step 4.2: Remove `ai_watch_status` and `command_output` from `src/cli/ai_watch.rs`**

Delete:
- The `pub(crate) async fn ai_watch_status` function (lines 174-217)
- The private `fn command_output` function (lines 277-297) — only called by `ai_watch_status`

**Keep in place:**
- `AiSmokeWatchReport` struct — still used by `ai_smoke_watch`
- `AiSmokeWatchTarget` struct — still used
- `ai_smoke_watch` function — still used
- `smoke_watch_target` function — still used
- `systemctl_user_output` function (lines 219-252) — imported by `coordination.rs`, MUST stay
- `systemctl_needs_user_bus_fallback` — called by `systemctl_user_output`, keep
- `inferred_user_bus_env` — called by `systemctl_user_output`, keep (this is a separate copy from `os_adapter.rs`; the CLI copy is kept because it's used by `systemctl_user_output` which must remain in this file)
- `current_uid` — called by `inferred_user_bus_env`, keep

- [ ] **Step 4.3: Compile check**

```bash
rtk cargo check 2>&1 | grep "error\[" | head -20
```

Expected: 0 errors. Common issues:
- `AiWatchStatusReport` no longer in `cli::ai_watch` — fix any remaining import that still references `super::ai_watch::AiWatchStatusReport` (should be `syslog_mcp::app::AiWatchStatusReport`)
- `command_output` removed — search and fix any other callers (there should be none)

- [ ] **Step 4.4: Run the full test suite**

```bash
rtk cargo test 2>&1 | tail -5
```

Expected: same pass count as before, 0 new failures. The 6 pre-existing compose live-target tests fail by environment (worktree path mismatch), not by code — they are acceptable. The `run_ai_watch_status_http_bails_with_inline_message` test in `dispatch_tests.rs` must still pass.

- [ ] **Step 4.5: Commit**

```bash
rtk git add src/cli/dispatch_ai.rs src/cli/ai_watch.rs
rtk git commit -m "refactor: cli dispatch calls service.ai_watch_status() directly"
```

---

## Task 5: Check `ai watch-status` MCP exposure

**Files:**
- Conditionally modify: `src/mcp/tools.rs`

- [ ] **Step 5.1: Check whether MCP exposes `ai watch-status`**

```bash
grep -n "watch.status\|watch_status\|WatchStatus" src/mcp/tools.rs | head -10
```

If no results: skip this task entirely.

If results found: verify the rejection is explicit and surfaces as a `ServiceError::InvalidInput` or documented `ActionError`. If the action currently panics or silently fails, add a guard:

```rust
// In the match arm for "ai watch-status" or equivalent in src/mcp/tools.rs:
return Err(ActionError::invalid_input(
    "ai watch-status requires direct host access; use the CLI instead of the MCP transport"
));
```

- [ ] **Step 5.2: Commit (if changes were made)**

```bash
rtk git add src/mcp/tools.rs
rtk git commit -m "fix: document ai watch-status MCP rejection explicitly"
```

---

## Task 6: Final verification and session close

- [ ] **Step 6.1: Run the full test suite**

```bash
rtk cargo test 2>&1 | tail -10
```

Expected: same test count as main branch, 0 new failures.

- [ ] **Step 6.2: Lint check**

```bash
rtk cargo clippy -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: 0 errors.

- [ ] **Step 6.3: Verify `systemctl_user_output` is still accessible to `coordination.rs`**

```bash
rtk cargo check --package syslog-mcp 2>&1 | grep "coordination" | head -5
```

Expected: no errors mentioning `coordination.rs` or `systemctl_user_output`.

- [ ] **Step 6.4: Close the bead and push**

```bash
bd close syslog-mcp-5gcn --reason="ai_watch_status moved to app/watch_status.rs as impl SyslogService; all systemctl probes route through OsAdapter::probe_command (mockable); AiWatchStatusReport in app/models.rs; CLI is now a thin adapter"
rtk git pull --rebase
bd dolt push
rtk git push
rtk git status
```

Expected: `git status` shows "Your branch is up to date with 'origin/main'".

---

## Self-Review

### Spec coverage check

| Requirement from syslog-mcp-5gcn | Task that covers it |
|---|---|
| `AiWatchStatusReport` moved out of CLI | Task 1 |
| `ai_watch_status()` moved out of CLI | Tasks 3, 4 |
| `systemctl`/`journalctl` probing moved to service | Task 3 |
| CLI limited to parsing and rendering | Task 4 |
| Focused service-layer tests for report assembly | Task 3.5 |
| Keep CLI parser/output tests narrow | Verified in Task 4.4 — existing tests pass unchanged |
| Preserve existing JSON shape | `AiWatchStatusReport` fields identical, `Serialize` derives unchanged |
| `systemctl_user_output` stays for `coordination.rs` | Task 4.2 explicitly preserves it |
| All systemctl probes testable via mock | Task 2 (`probe_command` on trait) |
| No 5th copy of `inferred_user_bus_env`/`current_uid` | Task 2 (promotes to `pub(crate)`); `watch_status.rs` delegates to `SystemOsAdapter::probe_command` |
| DB calls decoupled from OS probes | Task 3 (systemctl probes run first) |
| journalctl graceful degradation preserved | Task 3 (`.unwrap_or_default()` semantics via `Vec::new()` fallback) |

### Placeholder scan

No TBDs, TODOs, or "implement later" found. All code blocks are complete.

### Type consistency

- `AiWatchStatusReport` defined in Task 1, used in Tasks 3, 4 with the same field names
- `probe_command` added to trait in Task 2, used in Task 3 via `self.os.probe_command(...)`
- `SyslogService::ai_watch_status()` defined in Task 3, called in Task 4 as `service.ai_watch_status().await?`
- `ServiceResult<AiWatchStatusReport>` return type consistent throughout
- `probe_systemctl` is a private method on `SyslogService` — not a free function, no ownership inversion
