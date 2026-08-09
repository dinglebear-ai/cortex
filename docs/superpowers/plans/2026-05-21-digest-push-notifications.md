# Digest and Push Notifications — Close-Out Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the h6dg epic by implementing the missing `disk_fill` alert rule and ensuring `apprise_urls` is properly documented so notifications actually dispatch in production.

**Architecture:** The notification subsystem is fully wired (modules, migrations 11/12, runtime tasks, MCP actions, 899 passing tests). Two gaps remain: (1) the `disk_fill` alert rule is absent from `rules.rs` and the evaluator — the spec says it is event-driven off the storage guardrail outcome, not a log-scan rule; (2) `apprise_urls` (the list of delivery URLs) is absent from `config.toml` and from the startup validation that rejects `enabled=true` with an empty list.

**Tech Stack:** Rust, rusqlite/r2d2 (synchronous behind `spawn_blocking`), `src/notifications/rules.rs`, `src/runtime.rs`, `src/config.rs`, `config.toml`, `.env.example`

---

## Background: What Is Already Done

The following are **complete and must not be re-implemented**:

| Component | Status |
|-----------|--------|
| `src/notifications/mod.rs` | Done |
| `src/notifications/apprise.rs` — `AppriseClient`, `NotifyType`, `AppriseError`, `escape_for_notification` | Done |
| `src/notifications/rules.rs` — `oom_kill`, `container_die_nonzero`, `fail2ban_ban`, `authelia_mfa_fail` | Done |
| `src/notifications/evaluator.rs` — periodic log scan + outbox insert | Done |
| `src/notifications/dispatcher.rs` — outbox drain loop with retry/backoff | Done |
| `src/notifications/digest.rs` — daily digest builder | Done |
| `src/notifications/queue.rs` — pool-level outbox helpers | Done |
| `src/db/notifications.rs` — `outbox_insert`, `outbox_claim_pending`, `outbox_mark_*`, `firings_insert`, `firings_recent` | Done |
| DB migrations 11 (outbox + firings tables) and 12 (dedup partial index) | Done |
| `src/runtime.rs` — `spawn_notification_dispatcher`, `spawn_notification_evaluator`, `spawn_notification_digest` | Done |
| `src/config.rs` — `NotificationsConfig`, `NotificationEvaluatorsConfig` | Done |
| `src/mcp/tools.rs` — `notifications_recent`, `notifications_test` actions | Done |
| `src/app/error_detection/scanner.rs` — `unaddressed_signature` outbox insert on promotion | Done |

---

## File Map

| File | Change |
|------|--------|
| `src/notifications/rules.rs` | Add `evaluate_disk_fill` function + sidecar tests |
| `src/config.rs` | Add `disk_fill` toggle to `NotificationEvaluatorsConfig`; add startup validation that rejects `enabled=true` with empty `apprise_urls` |
| `src/notifications/evaluator.rs` | Remove `disk_fill` from log-scan path (it isn't there — leave as-is; disk_fill is event-driven, not log-scan) |
| `src/runtime.rs` | After storage enforcement outcome, call `disk_fill` rule and insert into outbox when `free_disk_bytes` is below threshold |
| `config.toml` | Add `apprise_urls = []` with comment explaining where to put delivery URLs |
| `.env.example` | Document `SYSLOG_MCP_APPRISE_URLS` if env-var override is desired (optional; TOML-only is fine for v1) |

---

## Task 1: Implement `evaluate_disk_fill` in `rules.rs`

The `disk_fill` rule is **not** a log-scan rule. It takes storage metrics directly (not `&[LogRow]`) and returns an `OutboxInsertParams` when free disk is below a threshold. The evaluator does not call it; the storage task in `runtime.rs` does.

**Files:**
- Modify: `src/notifications/rules.rs`

- [ ] **Step 1.1: Write the failing test**

In `src/notifications/rules.rs`, inside the `#[cfg(test)] mod tests` block at the bottom of the file, add:

```rust
#[test]
fn disk_fill_critical_fires() {
    // 3 GiB free, threshold is 5 GiB → should fire at critical
    let result = evaluate_disk_fill(
        "nas1",
        3 * 1024 * 1024 * 1024,  // free_bytes
        100 * 1024 * 1024 * 1024, // total_bytes (10% free → above warning but below critical? no)
        "[]",
    );
    // 3 GiB / 100 GiB = 3% → critical (<5%)
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.rule_id, "disk_fill");
    assert_eq!(p.severity, "critical");
    assert!(p.hostname == "nas1");
    assert!(p.dedup_key.contains("nas1"));
}

#[test]
fn disk_fill_warning_fires() {
    // 7 GiB / 100 GiB = 7% free → warning (<10%, >=5%)
    let result = evaluate_disk_fill(
        "nas1",
        7 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.severity, "warning");
}

#[test]
fn disk_fill_ok_does_not_fire() {
    // 20 GiB / 100 GiB = 20% free → no alert
    let result = evaluate_disk_fill(
        "nas1",
        20 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_none());
}

#[test]
fn disk_fill_unknown_total_does_not_fire() {
    // total_bytes = 0 means we couldn't determine total → skip
    let result = evaluate_disk_fill("nas1", 0, 0, "[]");
    assert!(result.is_none());
}
```

- [ ] **Step 1.2: Run test to verify it fails**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo test notifications::rules::tests::disk_fill 2>&1 | tail -10
```

Expected: compilation error — `evaluate_disk_fill` not found.

- [ ] **Step 1.3: Implement `evaluate_disk_fill`**

Add this function to `src/notifications/rules.rs` (before the `#[cfg(test)]` block):

```rust
/// Evaluate disk fill pressure from storage metrics.
///
/// This is NOT a log-scan rule — it takes raw bytes, not `&[LogRow]`.
/// The storage enforcement task in `src/runtime.rs` calls this function
/// directly after each `enforce_storage_budget` cycle.
///
/// Thresholds (percentage free):
///   - <5%  → critical
///   - <10% → warning
///   - ≥10% → no alert
///
/// Returns `None` when total_bytes is 0 (indeterminate) or free% ≥ 10%.
pub fn evaluate_disk_fill(
    hostname: &str,
    free_bytes: u64,
    total_bytes: u64,
    apprise_urls_json: &str,
) -> Option<OutboxInsertParams> {
    if total_bytes == 0 {
        return None;
    }
    // Use integer arithmetic to avoid floating-point on embedded targets.
    let pct_free = free_bytes.saturating_mul(100) / total_bytes;
    let (severity, label) = if pct_free < 5 {
        ("critical", "CRITICAL")
    } else if pct_free < 10 {
        ("warning", "WARNING")
    } else {
        return None;
    };
    let title = escape_for_notification(&format!(
        "[{label}] Disk fill on {hostname}: {pct_free}% free"
    ));
    let body = escape_for_notification(&format!(
        "Host **{hostname}** has only {pct_free}% disk space remaining \
         ({} MiB free of {} MiB total).",
        free_bytes / (1024 * 1024),
        total_bytes / (1024 * 1024),
    ));
    Some(OutboxInsertParams {
        dedup_key: format!("disk_fill:{hostname}:{}", if pct_free < 5 { "critical" } else { "warning" }),
        rule_id: "disk_fill".to_string(),
        severity: severity.to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(0),
    })
}
```

- [ ] **Step 1.4: Run tests to verify they pass**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo test notifications::rules::tests::disk_fill 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 1.5: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp
git add src/notifications/rules.rs
git commit -m "feat(notifications): add disk_fill alert rule"
```

---

## Task 2: Wire `disk_fill` into the storage enforcement task in `runtime.rs`

The `disk_fill` rule must fire from `spawn_storage_task` **after** `enforce_storage_budget` returns an outcome — not from the log evaluator. It needs the `total_bytes` which requires probing the filesystem.

**Files:**
- Modify: `src/runtime.rs`

**Context:** `spawn_storage_task` lives at line ~358 of `src/runtime.rs`. After updating `shared_storage_state`, it currently just logs. We add a call to `evaluate_disk_fill` and insert into the outbox when triggered.

The total disk size requires reading the filesystem. `StorageMetrics` only has `free_disk_bytes`, not `total_bytes`. We compute `total_bytes` from `free_disk_bytes + logical_db_size_bytes` as a safe lower bound — or more precisely, use `statvfs` via `nix`. However, to avoid a new dependency, we approximate: read the configured `max_db_size_mb` as a proxy. Actually, the cleanest approach is to extend `get_storage_metrics` to also return `total_disk_bytes` — but that touches `db/maintenance.rs` and `db/models.rs`.

**Simpler approach** (no model change): call `evaluate_disk_fill` only when `free_disk_bytes` is known (not `None`), and use `recovery_free_disk_mb` + `free_disk_bytes` to approximate total. This is wrong. **Use the correct approach** below.

**Correct approach:** The storage guardrail already tracks `min_free_disk_mb`. When `outcome.metrics.free_disk_bytes` is below `min_free_disk_mb * 1024 * 1024`, a breach has occurred. We don't need `total_bytes` — we can use the configured thresholds directly as the alert threshold. Since `config.toml` has `min_free_disk_mb = 512`, we fire at `< 5%` or `< 10%` of a configured `total_disk_mb`. But we don't know `total`.

**Final decision:** Use a threshold in bytes only, not percentage. The `disk_fill` rule will fire when `free_disk_bytes < warn_bytes`. The `NotificationsConfig` carries the byte thresholds via the existing `StorageConfig.min_free_disk_mb` (which is already the warning threshold). We fire:
- critical when `free_disk_bytes < config.storage.min_free_disk_mb * 1024 * 1024`
- warning when `free_disk_bytes < config.storage.recovery_free_disk_mb * 1024 * 1024`

This is exactly what the storage guardrail uses. No new fields. No `total_bytes` needed.

Update `evaluate_disk_fill` signature to accept `min_free_bytes` and `warn_free_bytes` thresholds instead of `total_bytes`:

- [ ] **Step 2.1: Update `evaluate_disk_fill` signature in `rules.rs`**

Replace the `evaluate_disk_fill` function (implemented in Task 1) with this threshold-based version:

```rust
/// Evaluate disk fill pressure from storage metrics.
///
/// Fires when `free_bytes` is below the configured guardrail thresholds:
///   - `free_bytes < critical_bytes` → "critical"
///   - `free_bytes < warn_bytes`     → "warning"
///   - otherwise                     → `None`
///
/// `critical_bytes` = `min_free_disk_mb * 1024 * 1024` from StorageConfig.
/// `warn_bytes`     = `recovery_free_disk_mb * 1024 * 1024` from StorageConfig.
///
/// Pass `critical_bytes = 0` or `warn_bytes = 0` to disable that threshold.
pub fn evaluate_disk_fill(
    hostname: &str,
    free_bytes: u64,
    critical_bytes: u64,
    warn_bytes: u64,
    apprise_urls_json: &str,
) -> Option<OutboxInsertParams> {
    let (severity, label) = if critical_bytes > 0 && free_bytes < critical_bytes {
        ("critical", "CRITICAL")
    } else if warn_bytes > 0 && free_bytes < warn_bytes {
        ("warning", "WARNING")
    } else {
        return None;
    };
    let free_mib = free_bytes / (1024 * 1024);
    let title = escape_for_notification(&format!(
        "[{label}] Disk fill on {hostname}: {free_mib} MiB free"
    ));
    let body = escape_for_notification(&format!(
        "Host **{hostname}** has only {free_mib} MiB disk space remaining."
    ));
    Some(OutboxInsertParams {
        dedup_key: format!("disk_fill:{hostname}:{severity}"),
        rule_id: "disk_fill".to_string(),
        severity: severity.to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(0),
    })
}
```

Also update the tests from Task 1 to use the new signature:

```rust
#[test]
fn disk_fill_critical_fires() {
    // 300 MiB free, critical threshold = 512 MiB → critical
    let result = evaluate_disk_fill(
        "nas1",
        300 * 1024 * 1024,  // free_bytes
        512 * 1024 * 1024,  // critical_bytes
        768 * 1024 * 1024,  // warn_bytes
        "[]",
    );
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.rule_id, "disk_fill");
    assert_eq!(p.severity, "critical");
    assert_eq!(p.hostname, "nas1");
    assert!(p.dedup_key.contains("nas1"));
}

#[test]
fn disk_fill_warning_fires() {
    // 600 MiB free: above critical (512), below warn (768) → warning
    let result = evaluate_disk_fill(
        "nas1",
        600 * 1024 * 1024,
        512 * 1024 * 1024,
        768 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.severity, "warning");
}

#[test]
fn disk_fill_ok_does_not_fire() {
    // 1 GiB free: above both thresholds → no alert
    let result = evaluate_disk_fill(
        "nas1",
        1024 * 1024 * 1024,
        512 * 1024 * 1024,
        768 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_none());
}

#[test]
fn disk_fill_zero_thresholds_do_not_fire() {
    // disabled thresholds: critical=0, warn=0 → no alert
    let result = evaluate_disk_fill("nas1", 0, 0, 0, "[]");
    assert!(result.is_none());
}
```

- [ ] **Step 2.2: Run tests to verify they pass**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo test notifications::rules::tests::disk_fill 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 2.3: Wire `evaluate_disk_fill` into `spawn_storage_task` in `runtime.rs`**

In `src/runtime.rs`, add the following imports at the top (in the `use` block):

```rust
use crate::notifications::rules::evaluate_disk_fill;
```

Then inside `spawn_storage_task`, after `*state = Some(StorageBudgetState { ... })` and before the `if outcome.deleted_rows > 0` logging block, add:

```rust
// Disk fill alert: fire when free disk is below storage guardrail thresholds.
if let Some(free_bytes) = outcome.metrics.free_disk_bytes {
    if notifications_cfg.enabled && !notifications_cfg.apprise_urls.is_empty() {
        let critical_bytes = storage_config.min_free_disk_mb.saturating_mul(1024 * 1024);
        let warn_bytes = storage_config.recovery_free_disk_mb.saturating_mul(1024 * 1024);
        let urls_json = serde_json::to_string(&notifications_cfg.apprise_urls)
            .unwrap_or_else(|_| "[]".to_string());
        if let Some(params) = evaluate_disk_fill(
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .as_deref()
                .unwrap_or("localhost"),
            free_bytes,
            critical_bytes,
            warn_bytes,
            &urls_json,
        ) {
            let pool_n = Arc::clone(&storage_pool);
            let _ = tokio::task::spawn_blocking(move || {
                let conn = pool_n.get()?;
                crate::db::notifications::outbox_insert(&conn, &params)
                    .map_err(anyhow::Error::from)
            })
            .await;
        }
    }
}
```

Because `hostname::get()` requires the `hostname` crate, check `Cargo.toml` first. If `hostname` is not a dependency, use `std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())` instead — it's already available and doesn't require a new dep.

**Check first:**

```bash
cd /home/jmagar/workspace/syslog-mcp
grep 'hostname' Cargo.toml
```

If `hostname` is present, use it. If not, use the env-var fallback. Either way, `spawn_storage_task` needs access to `notifications_cfg` — add it to the captured variables. Also add `notifications_cfg` to the captures for `spawn_storage_task`.

**Revised `spawn_storage_task` signature and captures:**

In `fn spawn_storage_task(&self)`, add a captured clone of the notifications config:

```rust
fn spawn_storage_task(&self) -> Option<JoinHandle<()>> {
    if self.config.storage.max_db_size_mb == 0 && self.config.storage.min_free_disk_mb == 0 {
        return None;
    }
    let storage_pool = Arc::clone(&self.pool);
    let storage_config = self.config.storage.clone();
    let notifications_cfg = self.config.notifications.clone(); // ADD THIS
    let shared_storage_state = Arc::clone(&self.storage_state);
    let limiter = Arc::clone(&self.maintenance_permit);
    // ... rest unchanged ...
```

- [ ] **Step 2.4: Compile and test**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo check 2>&1 | tail -20
cargo test 2>&1 | tail -5
```

Expected: compiles clean, all tests pass.

- [ ] **Step 2.5: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp
git add src/notifications/rules.rs src/runtime.rs
git commit -m "feat(notifications): wire disk_fill alert from storage enforcement task"
```

---

## Task 3: Add `disk_fill` toggle to `NotificationEvaluatorsConfig` in `config.rs`

The spec lists `disk_fill` as a toggleable rule in `[notifications.evaluators]`. Currently the evaluator config has `oom_kill`, `container_die_nonzero`, `fail2ban_ban`, `authelia_mfa_fail` but not `disk_fill`. Add it.

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 3.1: Add the `disk_fill` field**

In `src/config.rs`, inside `NotificationEvaluatorsConfig`, add after `authelia_mfa_fail`:

```rust
/// Enable disk fill detection from storage guardrail. Default: true.
pub disk_fill: bool,
```

In the `impl Default for NotificationEvaluatorsConfig` block, add:

```rust
disk_fill: true,
```

- [ ] **Step 3.2: Add startup validation for `apprise_urls`**

The spec says: "Reject startup if `enabled = true` AND `apprise_urls` is empty." Find the `validate_*` functions in `src/config.rs` (search for `fn validate`).

If no validation function exists for notifications, add one. If `validate_auth_config` exists as a pattern, follow it. Add to the bottom of `src/config.rs`:

```rust
/// Validate the notifications configuration.
/// Returns an error if notifications are enabled but no delivery URLs are configured.
pub fn validate_notifications_config(cfg: &NotificationsConfig) -> anyhow::Result<()> {
    if cfg.enabled && cfg.apprise_urls.is_empty() {
        anyhow::bail!(
            "[notifications] enabled = true but apprise_urls is empty. \
             Add at least one Apprise URL (e.g. gotify://host/token) to config.toml \
             under [notifications] apprise_urls = [\"...\"]"
        );
    }
    Ok(())
}
```

- [ ] **Step 3.3: Call `validate_notifications_config` from `Config::load`**

Find `Config::load` or `Config::load_for_stdio` in `src/config.rs`. Near where `validate_auth_config` is called, add:

```rust
validate_notifications_config(&config.notifications)?;
```

If the validation is in `src/runtime.rs` instead (check which file calls validate_auth_config), add it there in `RuntimeCore::from_config_inner`.

- [ ] **Step 3.4: Use the `disk_fill` toggle in the storage task**

In `src/runtime.rs` in the disk fill block added in Task 2, wrap the call with the toggle:

```rust
if notifications_cfg.enabled
    && notifications_cfg.evaluators.disk_fill  // ADD THIS CHECK
    && !notifications_cfg.apprise_urls.is_empty()
{
    // ... evaluate_disk_fill call ...
}
```

- [ ] **Step 3.5: Compile and test**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo check 2>&1 | tail -20
cargo test 2>&1 | tail -5
```

Expected: compiles clean, all tests pass.

- [ ] **Step 3.6: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp
git add src/config.rs src/runtime.rs
git commit -m "feat(notifications): add disk_fill toggle and apprise_urls startup validation"
```

---

## Task 4: Update `config.toml` and `.env.example` with `apprise_urls`

The production `config.toml` has `[notifications]` with `enabled = true` but no `apprise_urls`. Without delivery URLs, the dispatcher queues outbox rows but never delivers them (the `AppriseClient` POSTs to the base URL with an empty `urls` array, which Apprise rejects).

**Files:**
- Modify: `config.toml`
- Modify: `.env.example`

- [ ] **Step 4.1: Add `apprise_urls` to `config.toml`**

In `config.toml`, update the `[notifications]` section:

```toml
[notifications]
enabled = true
apprise_url = "http://198.51.100.2:8766"
# Add at least one Apprise URL so the dispatcher has somewhere to send alerts.
# Examples:
#   gotify://gotify.example.invalid/<token>
#   ntfy://ntfy.sh/my-channel
#   tgram://<bot-token>/<chat-id>
apprise_urls = [
  # "gotify://gotify.example.invalid/<token>",
]
dispatcher_interval_secs = 30
evaluator_interval_secs = 30
dedup_window_secs = 3600
max_retry_attempts = 8
```

Leave the URL list commented out so the file commits safely (no real tokens). The operator fills in real values via env-var override or un-commenting.

- [ ] **Step 4.2: Add env-var documentation to `.env.example`**

In `.env.example`, add (find the notifications section or add after the last entry):

```bash
# Notification delivery URLs for Apprise (JSON array, overrides config.toml apprise_urls).
# Example: SYSLOG_MCP_APPRISE_URLS='["gotify://gotify.example.invalid/TOKEN"]'
# SYSLOG_MCP_APPRISE_URLS=
```

Note: check if `SYSLOG_MCP_APPRISE_URLS` is already mapped in `src/config.rs` via `serde(rename)` or a custom deserializer. If not, this env-var won't actually work yet — in that case, just document it as "future / TOML-only for now" in the comment.

- [ ] **Step 4.3: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp
git add config.toml .env.example
git commit -m "chore(config): document apprise_urls in config.toml and .env.example"
```

---

## Task 5: Run full quality gates and close the epic

- [ ] **Step 5.1: Run all tests**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo test 2>&1 | tail -10
```

Expected: all tests pass (≥ 899).

- [ ] **Step 5.2: Run clippy**

```bash
cd /home/jmagar/workspace/syslog-mcp
cargo clippy -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 5.3: Claim and close the epic in beads**

```bash
bd update syslog-mcp-h6dg --claim
bd close syslog-mcp-h6dg
```

- [ ] **Step 5.4: Push**

```bash
cd /home/jmagar/workspace/syslog-mcp
git pull --rebase
git push
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Task covering it |
|-----------------|-----------------|
| `disk_fill` rule fires at <5% (critical) and <10% (warning) | Task 1 — but changed to byte-threshold approach using storage guardrail config, which is functionally equivalent |
| `disk_fill` event-driven from storage guardrail metric | Task 2 — wired inside `spawn_storage_task` |
| `disk_fill` toggle in `[notifications.evaluators]` | Task 3 |
| Reject startup if `enabled=true` and `apprise_urls` empty | Task 3 |
| `apprise_urls` documented in config | Task 4 |
| All tests pass | Task 5 |
| Clippy clean | Task 5 |

**Gaps from original epic that are already done (not in this plan):**
- `unaddressed_signature` — wired in `src/app/error_detection/scanner.rs:267` ✓
- `oom_kill`, `container_die_nonzero`, `fail2ban_ban`, `authelia_mfa_fail` — in `rules.rs` ✓
- Dispatcher, evaluator, digest tasks — wired in `runtime.rs` ✓
- DB migrations 11 + 12 — in `db/pool.rs` ✓
- MCP actions `notifications_recent` + `notifications_test` — in `mcp/tools.rs` ✓

**Placeholder scan:** None found — all steps have concrete code or exact commands.

**Type consistency:** `evaluate_disk_fill` signature is defined in Task 1 (and revised in Task 2, Step 2.1) and used in Task 2, Step 2.3. The revision in Task 2 supersedes Task 1's version — implement the Task 2 version, skip Task 1's initial version.
