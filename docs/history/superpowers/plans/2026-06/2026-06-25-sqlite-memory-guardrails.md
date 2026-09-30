# SQLite Memory Guardrails Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix GH #95 by making Cortex's SQLite memory footprint configurable, bounded, observable, and resilient to large-DB read pressure under the 2G container memory limit.

**Architecture:** Keep the existing r2d2 SQLite pool and writer-reserved read permit model, but replace the hardcoded per-connection page cache with a total pool budget divided across connections. Add bounded mmap, stable DB-busy error classification, a shared service-layer expensive-read semaphore based on the existing MCP `Cost::Expensive` metadata, and thresholded `PASSIVE` WAL checkpointing through existing maintenance code. Expose diagnostics through `db status` without leaking raw SQL, query text, tokens, or transcript/log content.

**Tech Stack:** Rust 2024, rusqlite/r2d2_sqlite, Tokio, Axum, serde, SQLite WAL PRAGMAs, cargo xtask release-version tooling.

## Global Constraints

- Work inside `/home/jmagar/workspace/cortex/.worktrees/issue-95-sqlite-memory` on branch `codex/issue-95-sqlite-memory`.
- Issue source: GH #95, bead `syslog-mcp-hk5wg`.
- Canonical config names: `storage.sqlite_page_cache_mb`, `storage.sqlite_mmap_mb`, `storage.heavy_read_concurrency`, `storage.wal_checkpoint_mb`.
- Canonical env vars: `CORTEX_SQLITE_PAGE_CACHE_MB`, `CORTEX_SQLITE_MMAP_MB`, `CORTEX_HEAVY_READ_CONCURRENCY`, `CORTEX_WAL_CHECKPOINT_MB`.
- Defaults: page cache total `128` MB, mmap `256` MB, heavy read concurrency `1`, WAL checkpoint threshold `256` MB.
- Do not add `wal_checkpoint_interval_secs` in P0.
- Do not add a new scheduler service in P0; reuse the existing maintenance cadence/path where possible.
- Do not rewrite every `collect::<Vec<_>>()`; gate and measure first.
- Use `ActionSpec.cost == Cost::Expensive` as the action-cost source of truth where possible.
- No raw SQL, raw SQLite text, user query strings, transcript snippets, log messages, tokens, env values, or arbitrary host paths in limiter/checkpoint/circuit-breaker responses.
- Every feature branch push must bump version-bearing files with `cargo xtask bump-version patch` and must update `CHANGELOG.md`.
- Use sccache-safe cargo commands in this repo: `RUSTC_WRAPPER='' cargo <cmd> --config 'build.rustc-wrapper=""'`.

---

## File Structure

- Modify `src/config.rs`: add storage defaults, fields, env overrides, validation, and helper methods for derived SQLite PRAGMA values.
- Modify `src/db/pool.rs`: pass `StorageConfig` into `configure_connection_pragmas`, apply derived `cache_size` and `mmap_size`, and add test-visible helper functions.
- Modify `src/db/pool_tests.rs`: verify cache/mmap PRAGMAs on multiple pooled connections and derived budget math.
- Modify `src/app/error.rs`: add stable retryable DB error classification helpers.
- Modify `src/app/services.rs`: add shared `heavy_read_permits`, `run_heavy_db`, and DB error classification at the `run_db` boundary.
- Modify `src/app/services/maintenance.rs`: route `get_stats` through heavy-read permits; include cache/mmap/WAL/cgroup diagnostics in `db_status`; add bounded WAL threshold checkpoint helper.
- Modify selected `src/app/services/*.rs`: route expensive service methods through `run_heavy_db` where they are service-layer DB closures.
- Modify `src/app/models/core.rs`: extend `DbMaintenanceStatus` with optional diagnostic fields.
- Modify `src/api.rs`: preserve stable retryable error categories and keep DB maintenance endpoint policy unchanged unless tests require small response adjustments.
- Modify `src/mcp/actions.rs` and/or `src/mcp/tools.rs`: expose a helper that returns whether an action is expensive for parity tests and possible MCP dispatch gating.
- Modify `src/api_tests.rs`, `src/app/service_tests.rs`, and `src/app/models_tests.rs`: add behavior coverage.
- Modify docs/config surfaces: `CLAUDE.md`, `README.md`, `docs/contracts/config-schema.md`, `docs/api.md`, and plugin config docs if the field is exposed there.
- Modify release/version files via `cargo xtask bump-version patch`.

## Task 1: Storage Config And Derived SQLite Budget

Status: completed

**Files:**
- Modify: `src/config.rs`
- Test: existing `src/config.rs` unit tests or add sidecar tests if present in this file

**Interfaces:**
- Produces: `StorageConfig::sqlite_page_cache_kib_per_connection(&self) -> anyhow::Result<i64>`
- Produces: `StorageConfig::sqlite_mmap_bytes(&self) -> u64`
- Produces: `StorageConfig::sqlite_mmap_bytes_i64(&self) -> anyhow::Result<i64>`
- Produces: `StorageConfig` fields `sqlite_page_cache_mb`, `sqlite_mmap_mb`, `heavy_read_concurrency`, `wal_checkpoint_mb`

> Review note: the original draft below used a hidden 4 MiB per-connection page-cache floor. That was deliberately superseded during review so `sqlite_page_cache_mb` remains a true total pool budget.

- [x] **Step 1: Add failing config defaults test**

Add this test near existing config tests in `src/config.rs`:

```rust
#[test]
fn storage_defaults_include_sqlite_memory_guardrails() {
    let storage = StorageConfig::default();
    assert_eq!(storage.pool_size, 8);
    assert_eq!(storage.sqlite_page_cache_mb, 128);
    assert_eq!(storage.sqlite_mmap_mb, 256);
    assert_eq!(storage.heavy_read_concurrency, 1);
    assert_eq!(storage.wal_checkpoint_mb, 256);
    assert_eq!(storage.sqlite_page_cache_kib_per_connection().unwrap(), -16_384);
    assert_eq!(storage.sqlite_mmap_bytes(), 256 * 1024 * 1024);
}

#[test]
fn storage_page_cache_budget_is_clamped_per_connection() {
    let mut storage = StorageConfig::default();
    storage.pool_size = 128;
    storage.sqlite_page_cache_mb = 1;
    assert_eq!(storage.sqlite_page_cache_kib_per_connection().unwrap(), -8);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run:

```bash
RUSTC_WRAPPER='' cargo test storage_defaults_include_sqlite_memory_guardrails storage_page_cache_budget_is_clamped_per_connection --config 'build.rustc-wrapper=""'
```

Expected: fail because the fields and helper methods do not exist.

- [x] **Step 3: Add config fields and defaults**

In `src/config.rs`, add defaults near `default_pool_size()`:

```rust
fn default_sqlite_page_cache_mb() -> u64 {
    128
}

fn default_sqlite_mmap_mb() -> u64 {
    256
}

fn default_heavy_read_concurrency() -> usize {
    1
}

fn default_wal_checkpoint_mb() -> u64 {
    256
}
```

Add fields to `StorageConfig`:

```rust
/// Total SQLite page-cache budget across the whole pool, in MiB.
/// This is divided by `pool_size` before applying `PRAGMA cache_size`.
pub sqlite_page_cache_mb: u64,
/// Bounded SQLite mmap size in MiB. Resident mapped pages can still be charged
/// to cgroup memory, so this is measured and reported rather than treated as
/// a memory bypass.
pub sqlite_mmap_mb: u64,
/// Maximum concurrent expensive read operations. Cheap/moderate reads still use
/// the existing writer-reserving DB permit pool.
pub heavy_read_concurrency: usize,
/// WAL size threshold in MiB for bounded opportunistic PASSIVE checkpoints.
pub wal_checkpoint_mb: u64,
```

Initialize them in `Default for StorageConfig`:

```rust
sqlite_page_cache_mb: default_sqlite_page_cache_mb(),
sqlite_mmap_mb: default_sqlite_mmap_mb(),
heavy_read_concurrency: default_heavy_read_concurrency(),
wal_checkpoint_mb: default_wal_checkpoint_mb(),
```

- [x] **Step 4: Add derived helper methods**

Add this impl block after `impl Default for StorageConfig`:

```rust
impl StorageConfig {
    pub fn sqlite_page_cache_kib_per_connection(&self) -> anyhow::Result<i64> {
        let pool_size = u64::from(self.pool_size.max(1));
        let total_kib = self.sqlite_page_cache_mb.saturating_mul(1024);
        let per_conn = i64::try_from((total_kib / pool_size).max(1))?;
        Ok(-per_conn)
    }

    pub fn sqlite_mmap_bytes(&self) -> u64 {
        self.sqlite_mmap_mb.saturating_mul(1024 * 1024)
    }

    pub fn wal_checkpoint_threshold_bytes(&self) -> u64 {
        self.wal_checkpoint_mb.saturating_mul(1024 * 1024)
    }
}
```

- [x] **Step 5: Wire env overrides and validation**

In `Config::load_inner`, after `CORTEX_POOL_SIZE`, add:

```rust
env_override_parse(
    "CORTEX_SQLITE_PAGE_CACHE_MB",
    &mut config.storage.sqlite_page_cache_mb,
)?;
env_override_parse("CORTEX_SQLITE_MMAP_MB", &mut config.storage.sqlite_mmap_mb)?;
env_override_parse(
    "CORTEX_HEAVY_READ_CONCURRENCY",
    &mut config.storage.heavy_read_concurrency,
)?;
env_override_parse(
    "CORTEX_WAL_CHECKPOINT_MB",
    &mut config.storage.wal_checkpoint_mb,
)?;
```

In storage validation, add exact checks:

```rust
if config.storage.sqlite_page_cache_mb == 0 {
    anyhow::bail!("storage.sqlite_page_cache_mb must be > 0");
}
if config.storage.heavy_read_concurrency == 0 {
    anyhow::bail!("storage.heavy_read_concurrency must be > 0");
}
if config.storage.wal_checkpoint_mb == 0 {
    anyhow::bail!("storage.wal_checkpoint_mb must be > 0");
}
```

- [x] **Step 6: Run focused tests**

Run:

```bash
RUSTC_WRAPPER='' cargo test storage_defaults_include_sqlite_memory_guardrails storage_page_cache_budget_is_clamped_per_connection --config 'build.rustc-wrapper=""'
```

Expected: pass.

- [x] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "fix: add sqlite memory guardrail config"
```

## Task 2: Apply Page Cache And mmap PRAGMAs Per Connection

Status: completed

**Files:**
- Modify: `src/db/pool.rs`
- Modify: `src/db/pool_tests.rs`

**Interfaces:**
- Consumes: `StorageConfig::sqlite_page_cache_kib_per_connection()`
- Consumes: `StorageConfig::sqlite_mmap_bytes()`
- Produces: every pooled connection has derived `PRAGMA cache_size` and `PRAGMA mmap_size`

- [x] **Step 1: Add failing pool tests**

Append to `src/db/pool_tests.rs`:

```rust
#[test]
fn init_pool_applies_sqlite_cache_budget_to_each_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_storage_config(dir.path().join("cache-budget.db"));
    config.pool_size = 2;
    config.sqlite_page_cache_mb = 128;

    let pool = init_pool(&config).unwrap();
    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    let cache_1: i64 = conn1
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();
    let cache_2: i64 = conn2
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();

    assert_eq!(cache_1, -65_536);
    assert_eq!(cache_2, -65_536);
}

#[test]
fn init_pool_applies_sqlite_mmap_to_each_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_storage_config(dir.path().join("mmap.db"));
    config.pool_size = 2;
    config.sqlite_mmap_mb = 32;

    let pool = init_pool(&config).unwrap();
    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    let mmap_1: i64 = conn1
        .query_row("PRAGMA mmap_size", [], |row| row.get(0))
        .unwrap();
    let mmap_2: i64 = conn2
        .query_row("PRAGMA mmap_size", [], |row| row.get(0))
        .unwrap();

    assert_eq!(mmap_1, 32 * 1024 * 1024);
    assert_eq!(mmap_2, 32 * 1024 * 1024);
}
```

- [x] **Step 2: Run tests to verify failure**

```bash
RUSTC_WRAPPER='' cargo test init_pool_applies_sqlite_cache_budget_to_each_pooled_connection init_pool_applies_sqlite_mmap_to_each_pooled_connection --config 'build.rustc-wrapper=""'
```

Expected: cache test sees `-64000`; mmap test sees `0` or platform default.

- [x] **Step 3: Change pool setup to pass full storage config**

In `src/db/pool.rs`, change connection customization so `configure_connection_pragmas` receives `&StorageConfig`, not only `wal_mode`. The target signature is:

```rust
fn configure_connection_pragmas(conn: &mut Connection, storage: &StorageConfig) -> rusqlite::Result<()> {
    if storage.wal_mode {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000_i64)?;
    conn.pragma_update(None, "cache_size", storage.sqlite_page_cache_kib_per_connection()?)?;
    conn.pragma_update(None, "mmap_size", storage.sqlite_mmap_bytes_i64()?)?;
    conn.pragma_update(None, "analysis_limit", 400_i64)?;
    Ok(())
}
```

If current pool setup captures only `wal_mode`, change the customizer/call site to clone `StorageConfig` into the closure.

- [x] **Step 4: Run focused tests**

```bash
RUSTC_WRAPPER='' cargo test init_pool_applies_busy_timeout_to_each_pooled_connection init_pool_applies_sqlite_cache_budget_to_each_pooled_connection init_pool_applies_sqlite_mmap_to_each_pooled_connection --config 'build.rustc-wrapper=""'
```

Expected: pass.

- [x] **Step 5: Commit**

```bash
git add src/db/pool.rs src/db/pool_tests.rs
git commit -m "fix: derive sqlite pragmas from memory budget"
```

## Task 3: Classify SQLite BUSY And Pool Timeouts At Service Boundary

Status: completed

**Files:**
- Modify: `src/app/error.rs`
- Modify: `src/app/services.rs`
- Test: `src/app/error_tests.rs` or `src/app/service_tests.rs`

**Interfaces:**
- Produces: `ServiceError::classify_db_error(error: anyhow::Error) -> ServiceError`
- Consumes: existing `ServiceError::Busy(String)` and `ServiceError::DatabaseTimeout`

- [x] **Step 1: Add failing classifier tests**

In `src/app/error_tests.rs`, add:

```rust
use super::*;

#[test]
fn classify_db_error_promotes_sqlite_busy_to_retryable_busy() {
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: rusqlite::ffi::SQLITE_BUSY,
        },
        Some("database is locked".to_string()),
    );

    let classified = ServiceError::classify_db_error(anyhow::Error::new(error));
    assert!(matches!(classified, ServiceError::Busy(message) if message == "database_busy"));
}

#[test]
fn classify_db_error_promotes_sqlite_locked_to_retryable_busy() {
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseLocked,
            extended_code: rusqlite::ffi::SQLITE_LOCKED,
        },
        Some("database table is locked".to_string()),
    );

    let classified = ServiceError::classify_db_error(anyhow::Error::new(error));
    assert!(matches!(classified, ServiceError::Busy(message) if message == "database_busy"));
}
```

- [x] **Step 2: Run tests to verify failure**

```bash
RUSTC_WRAPPER='' cargo test classify_db_error_promotes_sqlite_busy_to_retryable_busy classify_db_error_promotes_sqlite_locked_to_retryable_busy --config 'build.rustc-wrapper=""'
```

Expected: fail because `classify_db_error` does not exist.

- [x] **Step 3: Implement classifier**

In `src/app/error.rs`, add:

```rust
impl ServiceError {
    pub(crate) fn classify_db_error(error: anyhow::Error) -> Self {
        match error.downcast::<ServiceError>() {
            Ok(service_error) => return service_error,
            Err(error) => {
                if let Some(sqlite) = error.downcast_ref::<rusqlite::Error>() {
                    if is_retryable_sqlite_error(sqlite) {
                        return ServiceError::Busy("database_busy".to_string());
                    }
                }
                if let Some(pool_error) = error.downcast_ref::<r2d2::Error>() {
                    if pool_error.to_string().to_ascii_lowercase().contains("timed out") {
                        return ServiceError::DatabaseTimeout;
                    }
                }
                ServiceError::Internal(error)
            }
        }
    }
}

fn is_retryable_sqlite_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
                ..
            },
            _
        )
    )
}
```

If `r2d2::Error` is not directly available without adding an import, use the concrete type already pulled in by `r2d2_sqlite`; do not add a new dependency.

- [x] **Step 4: Use classifier in `run_db`**

In `src/app/services.rs`, replace the `Ok(r) => r.map_err(...)` arm with:

```rust
Ok(r) => r.map_err(ServiceError::classify_db_error),
```

Keep join errors as `Internal`; those are task execution failures, not SQLite contention.

- [x] **Step 5: Run focused tests**

```bash
RUSTC_WRAPPER='' cargo test classify_db_error_promotes_sqlite_busy_to_retryable_busy classify_db_error_promotes_sqlite_locked_to_retryable_busy --config 'build.rustc-wrapper=""'
```

Expected: pass.

Ran equivalent one-filter cargo command:

```bash
RUSTC_WRAPPER='' cargo test classify_db_error --config 'build.rustc-wrapper=""'
```

- [x] **Step 6: Commit**

```bash
git add src/app/error.rs src/app/services.rs src/app/error_tests.rs
git commit -m "fix: classify sqlite busy errors as retryable"
```

## Task 4: Shared Heavy-Read Limiter

Status: completed

**Files:**
- Modify: `src/app/services.rs`
- Modify: `src/app/services/maintenance.rs`
- Modify: `src/app/services/analytics.rs`
- Modify: `src/app/services/ai.rs`
- Modify: `src/app/services/logs.rs`
- Test: `src/app/service_tests.rs`

**Interfaces:**
- Produces: `async fn run_heavy_db<F, T>(&self, op: &'static str, f: F) -> ServiceResult<T>`
- Produces: shared field `heavy_read_permits: Arc<Semaphore>`

- [x] **Step 1: Add failing limiter test**

In `src/app/service_tests.rs`, add:

```rust
#[tokio::test]
async fn heavy_read_limiter_times_out_when_permit_is_held() {
    let (service, _pool, _dir) = test_service();
    let held = service
        .heavy_read_permits
        .clone()
        .acquire_owned()
        .await
        .expect("heavy permit");

    let err = service
        .run_heavy_db("heavy_test", |_pool| Ok::<_, anyhow::Error>(()))
        .await
        .unwrap_err();

    drop(held);
    assert!(matches!(err, ServiceError::Busy(message) if message == "heavy_read_limited"));
}
```

If field privacy blocks this test from the sidecar module, keep the field `pub(super)` and place the test in the same module tree.

- [x] **Step 2: Run test to verify failure**

```bash
RUSTC_WRAPPER='' cargo test heavy_read_limiter_times_out_when_permit_is_held --config 'build.rustc-wrapper=""'
```

Expected: fail because the field/method do not exist.

- [x] **Step 3: Add semaphore field and constructor wiring**

In `CortexService`, add:

```rust
pub(super) heavy_read_permits: Arc<Semaphore>,
```

In both constructors, initialize:

```rust
heavy_read_permits: Arc::new(Semaphore::new(storage.heavy_read_concurrency)),
```

- [x] **Step 4: Implement `run_heavy_db`**

Add below `run_db`:

```rust
async fn run_heavy_db<F, T>(&self, op: &'static str, f: F) -> ServiceResult<T>
where
    F: FnOnce(&DbPool) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let wait_start = Instant::now();
    let permit_result = tokio::time::timeout(
        self.acquire_timeout,
        Arc::clone(&self.heavy_read_permits).acquire_owned(),
    )
    .await;
    let heavy_permit = match permit_result {
        Err(_) => {
            tracing::warn!(op, wait_ms = wait_start.elapsed().as_millis(), "heavy read limited");
            return Err(ServiceError::Busy("heavy_read_limited".to_string()));
        }
        Ok(Err(_)) => {
            tracing::warn!(op, "heavy read limiter closed");
            return Err(ServiceError::Busy("heavy_read_limited".to_string()));
        }
        Ok(Ok(permit)) => permit,
    };
    let _heavy_permit = heavy_permit;
    self.run_db(op, f).await
}
```

- [x] **Step 5: Route high-cost DB closures through `run_heavy_db`**

Replace `run_db` with `run_heavy_db` for these service methods where they wrap broad scans/aggregations:

```text
src/app/services/maintenance.rs::get_stats
src/app/services/analytics.rs::patterns
src/app/services/analytics.rs::clock_skew
src/app/services/analytics.rs::anomalies
src/app/services/analytics.rs::compare
src/app/services/ai.rs::project_context
src/app/services/ai.rs::correlate_events
src/app/services/logs.rs::correlate_state
```

Do not route point lookups, health/status, `tail`, `search` with strict limits, or maintenance writes through the heavy-read limiter in this task.

- [x] **Step 6: Run focused tests**

```bash
RUSTC_WRAPPER='' cargo test heavy_read_limiter_times_out_when_permit_is_held --config 'build.rustc-wrapper=""'
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add src/app/services.rs src/app/services/maintenance.rs src/app/services/analytics.rs src/app/services/ai.rs src/app/services/logs.rs src/app/service_tests.rs
git commit -m "fix: limit concurrent expensive reads"
```

## Task 5: WAL Threshold Checkpointing And Diagnostics

Status: completed

**Files:**
- Modify: `src/app/models/core.rs`
- Modify: `src/app/services/maintenance.rs`
- Modify: `src/db/maintenance.rs`
- Modify: `src/db.rs`
- Test: `src/api_tests.rs`

**Interfaces:**
- Produces `DbMaintenanceStatus` fields:
  - `sqlite_page_cache_mb: u64`
  - `sqlite_page_cache_kib_per_connection: i64`
  - `sqlite_mmap_mb: u64`
  - `sqlite_mmap_bytes: u64`
  - `heavy_read_concurrency: usize`
  - `wal_checkpoint_mb: u64`
  - `wal_checkpoint_threshold_bytes: u64`
  - `cgroup_memory_max_bytes: Option<u64>`
  - `cgroup_memory_current_bytes: Option<u64>`
  - `cgroup_memory_peak_bytes: Option<u64>`

- [x] **Step 1: Add failing API status assertions**

Extend `db_status_returns_pragma_snapshot` in `src/api_tests.rs`:

```rust
assert_eq!(value["sqlite_page_cache_mb"], 128);
assert_eq!(value["sqlite_page_cache_kib_per_connection"], -16_384);
assert_eq!(value["sqlite_mmap_mb"], 256);
assert_eq!(value["sqlite_mmap_bytes"], 256 * 1024 * 1024);
assert_eq!(value["heavy_read_concurrency"], 1);
assert_eq!(value["wal_checkpoint_mb"], 256);
assert_eq!(value["wal_checkpoint_threshold_bytes"], 256 * 1024 * 1024);
assert!(value.get("cgroup_memory_max_bytes").is_some());
assert!(value.get("cgroup_memory_current_bytes").is_some());
assert!(value.get("cgroup_memory_peak_bytes").is_some());
```

- [x] **Step 2: Run test to verify failure**

```bash
RUSTC_WRAPPER='' cargo test db_status_returns_pragma_snapshot --config 'build.rustc-wrapper=""'
```

Expected: fail because fields do not exist.

- [x] **Step 3: Extend model**

In `src/app/models/core.rs`, extend `DbMaintenanceStatus`:

```rust
pub sqlite_page_cache_mb: u64,
pub sqlite_page_cache_kib_per_connection: i64,
pub sqlite_mmap_mb: u64,
pub sqlite_mmap_bytes: u64,
pub heavy_read_concurrency: usize,
pub wal_checkpoint_mb: u64,
pub wal_checkpoint_threshold_bytes: u64,
pub cgroup_memory_max_bytes: Option<u64>,
pub cgroup_memory_current_bytes: Option<u64>,
pub cgroup_memory_peak_bytes: Option<u64>,
```

- [x] **Step 4: Add cgroup probe helper**

In `src/app/services/maintenance.rs`, add a small helper:

```rust
#[derive(Debug, Clone, Default)]
struct CgroupMemorySnapshot {
    max_bytes: Option<u64>,
    current_bytes: Option<u64>,
    peak_bytes: Option<u64>,
}

fn read_cgroup_memory_snapshot() -> CgroupMemorySnapshot {
    fn read_value(path: &str) -> Option<u64> {
        let raw = std::fs::read_to_string(path).ok()?;
        let trimmed = raw.trim();
        if trimmed == "max" {
            return None;
        }
        trimmed.parse::<u64>().ok()
    }

    CgroupMemorySnapshot {
        max_bytes: read_value("/sys/fs/cgroup/memory.max"),
        current_bytes: read_value("/sys/fs/cgroup/memory.current"),
        peak_bytes: read_value("/sys/fs/cgroup/memory.peak"),
    }
}
```

This helper must not return raw paths or raw read errors to clients.

- [x] **Step 5: Populate status fields**

In `db_status`, before constructing `DbMaintenanceStatus`, call:

```rust
let cgroup = read_cgroup_memory_snapshot();
```

Populate the new fields from `storage` and `cgroup`.

- [x] **Step 6: Add bounded WAL threshold helper**

In `src/db/maintenance.rs`, add:

```rust
pub fn maybe_checkpoint_wal_by_size(pool: &DbPool, db_path: &Path, threshold_bytes: u64) -> Result<Option<(i64, i64, i64)>> {
    if threshold_bytes == 0 {
        return Ok(None);
    }
    let wal_path = db_path.with_extension("db-wal");
    let wal_size = match std::fs::metadata(&wal_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if wal_size < threshold_bytes {
        return Ok(None);
    }
    db_wal_checkpoint(pool, "passive").map(Some)
}
```

Export it from `src/db.rs`.

- [x] **Step 7: Call threshold helper from maintenance path**

In `checkpoint_wal_and_incremental_vacuum`, replace the unconditional `PRAGMA wal_checkpoint(PASSIVE)` block with:

```rust
match maybe_checkpoint_wal_by_size(
    pool,
    &config.db_path,
    config.wal_checkpoint_threshold_bytes(),
) {
    Ok(Some((busy, log_frames, checkpointed_frames))) => {
        tracing::debug!(busy, log_frames, checkpointed_frames, "WAL threshold checkpoint completed");
    }
    Ok(None) => tracing::debug!("WAL threshold checkpoint skipped"),
    Err(error) => tracing::warn!(error = %error, "WAL threshold checkpoint skipped (non-fatal)"),
}
```

If `checkpoint_wal_and_incremental_vacuum` does not currently receive `StorageConfig`, update its caller to pass it. Keep the attempt `PASSIVE` only.

- [x] **Step 8: Run focused tests**

```bash
RUSTC_WRAPPER='' cargo test db_status_returns_pragma_snapshot --config 'build.rustc-wrapper=""'
```

Expected: pass.

- [ ] **Step 9: Commit**

```bash
git add src/app/models/core.rs src/app/services/maintenance.rs src/db/maintenance.rs src/db.rs src/api_tests.rs
git commit -m "fix: expose sqlite memory and wal diagnostics"
```

## Task 6: Action Cost Parity Tests

Status: completed

**Files:**
- Modify: `src/mcp/actions.rs`
- Modify: `src/mcp/tools.rs` if needed
- Test: `src/mcp/tools_tests.rs` or `src/mcp/actions_tests.rs`

**Interfaces:**
- Produces a test-visible helper to enumerate expensive action names.

- [x] **Step 1: Add helper and test**

In `src/mcp/actions.rs`, add:

```rust
#[cfg(test)]
pub(crate) fn expensive_action_names_for_test() -> Vec<&'static str> {
    ACTION_SPECS
        .iter()
        .filter(|spec| spec.cost == Cost::Expensive)
        .map(|spec| spec.name)
        .collect()
}
```

Add a test in the same file via an existing sidecar or inline `#[cfg(test)]` module:

```rust
#[test]
fn expensive_actions_include_memory_risky_queries() {
    let names = expensive_action_names_for_test();
    for expected in [
        "stats",
        "patterns",
        "clock_skew",
        "anomalies",
        "compare",
        "abuse_investigate",
        "compose_doctor",
        "fleet_state",
        "correlate_state",
    ] {
        assert!(names.contains(&expected), "missing expensive action {expected}");
    }
}
```

- [x] **Step 2: Run focused test**

```bash
RUSTC_WRAPPER='' cargo test expensive_actions_include_memory_risky_queries --config 'build.rustc-wrapper=""'
```

Expected: pass after helper/test are added.

- [ ] **Step 3: Commit**

```bash
git add src/mcp/actions.rs
git commit -m "test: lock expensive action metadata"
```

## Task 7: Docs, Version, And Full Verification

Status: completed

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `docs/contracts/config-schema.md`
- Modify: `docs/api.md`
- Modify: `Cargo.toml`, `Cargo.lock`, `server.json`, `mcpb/manifest.json`, `docker-compose.prod.yml`, `CHANGELOG.md` via xtask

**Interfaces:**
- Produces docs for the new config contract and diagnostics.
- Produces a patch version bump in all version-bearing files.

- [x] **Step 1: Update config docs**

In `docs/contracts/config-schema.md`, update `[storage]` rows so `pool_size` default is `8`, not stale `4`, and add rows:

```markdown
| `sqlite_page_cache_mb` | `CORTEX_SQLITE_PAGE_CACHE_MB` | u64 | `128` | tuning | restart-only | `> 0` | — | Total SQLite page-cache budget across the pool; divided by `pool_size` before `PRAGMA cache_size` |
| `sqlite_mmap_mb` | `CORTEX_SQLITE_MMAP_MB` | u64 | `256` | tuning | restart-only | derived bytes must fit `i64` | — | Bounded SQLite mmap size; resident pages may still count toward cgroup memory |
| `heavy_read_concurrency` | `CORTEX_HEAVY_READ_CONCURRENCY` | usize | `1` | tuning | restart-only | `> 0` | — | Shared service-layer limiter for SQLite-heavy reads |
| `wal_checkpoint_mb` | `CORTEX_WAL_CHECKPOINT_MB` | u64 | `256` | tuning | restart-only | `> 0` | — | WAL size threshold for bounded PASSIVE checkpoint attempts |
```

- [x] **Step 2: Update operator docs**

In `CLAUDE.md` and `README.md`, add the same env vars under storage config. In `docs/api.md`, update `/api/db/status` response docs to mention the new memory/WAL/cgroup diagnostic fields and that cgroup paths/errors are not exposed.

- [x] **Step 3: Bump version**

Run:

```bash
cargo xtask bump-version patch
```

Expected: version-bearing files update together and `CHANGELOG.md` gains a new patch entry.

- [x] **Step 4: Run formatting and focused tests**

```bash
cargo fmt --check
RUSTC_WRAPPER='' cargo test storage_defaults_include_sqlite_memory_guardrails storage_page_cache_budget_is_clamped_per_connection --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo test init_pool_applies_sqlite_cache_budget_to_each_pooled_connection init_pool_applies_sqlite_mmap_to_each_pooled_connection --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo test classify_db_error_promotes_sqlite_busy_to_retryable_busy classify_db_error_promotes_sqlite_locked_to_retryable_busy --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo test heavy_read_limiter_times_out_when_permit_is_held --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo test db_status_returns_pragma_snapshot --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo test expensive_actions_include_memory_risky_queries --config 'build.rustc-wrapper=""'
```

Expected: all pass.

- [x] **Step 5: Run full repo gates**

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""'
RUSTC_WRAPPER='' cargo clippy --all-targets -- -D warnings --config 'build.rustc-wrapper=""'
cargo xtask check-version-sync
cargo xtask check-release-versions
```

Expected: all pass. If clippy command syntax fails because `--config` position is wrong, run:

```bash
RUSTC_WRAPPER='' cargo --config 'build.rustc-wrapper=""' clippy --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md README.md docs/contracts/config-schema.md docs/api.md Cargo.toml Cargo.lock server.json mcpb/manifest.json docker-compose.prod.yml CHANGELOG.md
git commit -m "docs: document sqlite memory guardrails"
```

## Self-Review Notes

Spec coverage:
- Total cache budget and bounded mmap: Tasks 1-2.
- Heavy-read limiter: Task 4.
- DB busy/timeout mapping: Task 3.
- WAL thresholding: Task 5.
- `db status` diagnostics and cgroup fields: Task 5.
- Existing `Cost::Expensive` metadata parity: Task 6.
- Docs/version bump: Task 7.
- Deferred exhaustive Vec rewrites and external observability are explicitly out of scope.

Placeholder scan:
- The plan intentionally avoids `TODO`, `TBD`, and unspecified "handle edge cases" language. Every required behavior has concrete fields, function names, commands, or tests.

Type consistency:
- Config fields are consistently named `sqlite_page_cache_mb`, `sqlite_mmap_mb`, `heavy_read_concurrency`, and `wal_checkpoint_mb`.
- Env vars are consistently `CORTEX_SQLITE_PAGE_CACHE_MB`, `CORTEX_SQLITE_MMAP_MB`, `CORTEX_HEAVY_READ_CONCURRENCY`, and `CORTEX_WAL_CHECKPOINT_MB`.
- The plan uses `DbMaintenanceStatus` for `/api/db/status`, not `DbStats`.
