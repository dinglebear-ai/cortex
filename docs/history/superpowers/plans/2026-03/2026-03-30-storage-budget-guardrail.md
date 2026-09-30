# Storage Budget Guardrail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a storage budget guardrail that automatically reclaims space and blocks writes when the SQLite log store threatens to exhaust the host filesystem.

**Architecture:** Extend `StorageConfig` with dual storage limits and recovery targets, add DB-side storage health and cleanup primitives in `src/db.rs`, then wire a periodic enforcement loop plus batch-writer hard stop in `src/main.rs` and `src/syslog.rs`. Surface the new state through `get_stats` and update runtime docs/config comments to match the shipped behavior.

**Tech Stack:** Rust, Tokio, rusqlite/SQLite (WAL + incremental auto-vacuum), r2d2, Axum, serde, tracing

---

## File Map

- Modify: `src/config.rs`
  - Add new storage settings, serde defaults, env parsing, and validation rules.
- Modify: `src/db.rs`
  - Add storage metrics/state structs, DB logical-size calculation, free-space probe seam, emergency cleanup, host reconciliation, incremental auto-vacuum setup/migration, and stats expansion.
- Modify: `src/syslog.rs`
  - Add write-time hard-stop logic around the batch writer using the new DB storage guard checks and thread `StorageConfig` into the writer path.
- Modify: `src/main.rs`
  - Start a periodic storage-enforcement task beside the existing retention task and log storage-guard lifecycle transitions.
- Modify: `src/mcp.rs`
  - Update `AppState` and `get_stats` metadata/response shape expectations so stats can include storage budget config and write-block state.
- Modify: `config.toml`
  - Document new storage settings with sensible defaults.
- Modify: `README.md`
  - Document storage guardrail behavior, defaults, and operational consequences.
- Modify: `CLAUDE.md`
  - Document the new env vars, defaults, and storage-protection behavior for future sessions.
- Create: `docs/superpowers/plans/2026-03-30-storage-budget-guardrail.md`
  - This plan document.

### Task 1: Add Config Surface and Validation

**Files:**
- Modify: `src/config.rs`
- Modify: `config.toml`
- Test: `src/config.rs`

- [ ] **Step 1: Write failing config tests for new defaults and validation**

```rust
#[test]
fn defaults_include_storage_budget_settings() {
    let cfg = Config::default();
    assert_eq!(cfg.storage.max_db_size_mb, 1024);
    assert_eq!(cfg.storage.recovery_db_size_mb, 900);
    assert_eq!(cfg.storage.min_free_disk_mb, 512);
    assert_eq!(cfg.storage.recovery_free_disk_mb, 768);
    assert_eq!(cfg.storage.cleanup_interval_secs, 60);
}

#[test]
#[serial]
fn rejects_invalid_storage_budget_relationships() {
    std::fs::write(
        "config.toml",
        r#"[syslog]
host = "0.0.0.0"
port = 1514

[storage]
db_path = "/tmp/syslog.db"
pool_size = 4
retention_days = 90
wal_mode = true
max_db_size_mb = 100
recovery_db_size_mb = 100
min_free_disk_mb = 512
recovery_free_disk_mb = 768
cleanup_interval_secs = 60

[mcp]
host = "0.0.0.0"
port = 3100
"#,
    ).unwrap();

    let err = Config::load().unwrap_err().to_string();
    assert!(err.contains("recovery_db_size_mb"));
}
```

- [ ] **Step 2: Run the config tests to verify they fail**

Run: `cargo test defaults_include_storage_budget_settings -- --nocapture`
Expected: FAIL because `StorageConfig` does not yet define the new fields or validation.

- [ ] **Step 3: Add storage-budget fields, serde defaults, env parsing, and validation**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub db_path: PathBuf,
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_true")]
    pub wal_mode: bool,
    #[serde(default = "default_max_db_size_mb")]
    pub max_db_size_mb: u64,
    #[serde(default = "default_recovery_db_size_mb")]
    pub recovery_db_size_mb: u64,
    #[serde(default = "default_min_free_disk_mb")]
    pub min_free_disk_mb: u64,
    #[serde(default = "default_recovery_free_disk_mb")]
    pub recovery_free_disk_mb: u64,
    #[serde(default = "default_cleanup_interval_secs")]
    pub cleanup_interval_secs: u64,
}
```

- [ ] **Step 4: Add matching `SYSLOG_MCP_*` env overrides and explicit validation**

```rust
env_override_parse("SYSLOG_MCP_MAX_DB_SIZE_MB", &mut config.storage.max_db_size_mb)?;
env_override_parse(
    "SYSLOG_MCP_RECOVERY_DB_SIZE_MB",
    &mut config.storage.recovery_db_size_mb,
)?;
env_override_parse("SYSLOG_MCP_MIN_FREE_DISK_MB", &mut config.storage.min_free_disk_mb)?;
env_override_parse(
    "SYSLOG_MCP_RECOVERY_FREE_DISK_MB",
    &mut config.storage.recovery_free_disk_mb,
)?;
env_override_parse(
    "SYSLOG_MCP_CLEANUP_INTERVAL_SECS",
    &mut config.storage.cleanup_interval_secs,
)?;
```

- [ ] **Step 5: Re-run the config tests to verify they pass**

Run: `cargo test config::tests -- --nocapture`
Expected: PASS for the new defaults/validation tests and existing config tests.

- [ ] **Step 6: Update `config.toml` defaults/comments**

```toml
[storage]
db_path = "/data/syslog.db"
pool_size = 4
retention_days = 90
wal_mode = true
max_db_size_mb = 1024
recovery_db_size_mb = 900
min_free_disk_mb = 512
recovery_free_disk_mb = 768
cleanup_interval_secs = 60
```

- [ ] **Step 7: Commit**

```bash
git add src/config.rs config.toml
git commit -m "feat: add storage budget config"
```

### Task 2: Add DB Storage Metrics and Auto-Vacuum Initialization

**Files:**
- Modify: `src/db.rs`
- Test: `src/db.rs`

- [ ] **Step 1: Write failing DB tests for logical-size accounting and auto-vacuum setup**

```rust
#[test]
fn test_storage_metrics_report_logical_size() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(&pool, &[make_entry("2026-01-01T00:00:01Z", "host-a", "info", "hello")]).unwrap();

    let metrics = get_storage_metrics(&pool).unwrap();
    assert!(metrics.logical_db_size_bytes > 0);
    assert!(metrics.physical_db_size_bytes >= metrics.logical_db_size_bytes);
}

#[test]
fn test_init_pool_enables_incremental_auto_vacuum() {
    let (pool, _dir) = test_pool();
    let conn = pool.get().unwrap();
    let mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, 2);
}

#[test]
fn test_init_pool_migrates_existing_db_to_incremental_auto_vacuum() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("legacy.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("PRAGMA auto_vacuum=NONE; CREATE TABLE logs(id INTEGER PRIMARY KEY);").unwrap();
    drop(conn);

    let pool = init_pool(&StorageConfig {
        db_path,
        pool_size: 1,
        retention_days: 90,
        wal_mode: false,
        max_db_size_mb: 1024,
        recovery_db_size_mb: 900,
        min_free_disk_mb: 512,
        recovery_free_disk_mb: 768,
        cleanup_interval_secs: 60,
    }).unwrap();
    let conn = pool.get().unwrap();
    let mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, 2);
}
```

- [ ] **Step 2: Run the DB tests to verify they fail**

Run: `cargo test test_storage_metrics_report_logical_size -- --nocapture`
Expected: FAIL because the helpers do not exist yet.

- [ ] **Step 3: Run the migration test to verify it fails**

Run: `cargo test test_init_pool_migrates_existing_db_to_incremental_auto_vacuum -- --nocapture`
Expected: FAIL because the helpers and auto-vacuum behavior do not exist yet.

- [ ] **Step 4: Add storage metric/state structs and metric queries that accept storage config**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub logical_db_size_bytes: u64,
    pub physical_db_size_bytes: u64,
    pub free_disk_bytes: Option<u64>,
}

pub fn get_storage_metrics(pool: &DbPool, config: &StorageConfig) -> Result<StorageMetrics> {
    let conn = pool.get()?;
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let db_dir = config.db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    // plus filesystem size/free-space probe
}
```

- [ ] **Step 5: Update `init_pool` to configure incremental auto-vacuum and migrate existing DBs**

```rust
conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")?;
let auto_vacuum_mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
if auto_vacuum_mode != 2 {
    conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
}
```

- [ ] **Step 6: Re-run the logical-size test**

Run: `cargo test test_storage_metrics_report_logical_size -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Re-run the fresh-db auto-vacuum test**

Run: `cargo test test_init_pool_enables_incremental_auto_vacuum -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Re-run the legacy-db migration test**

Run: `cargo test test_init_pool_migrates_existing_db_to_incremental_auto_vacuum -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/db.rs
git commit -m "feat: add storage metrics and vacuum setup"
```

### Task 3: Implement Emergency Cleanup and Host Reconciliation

**Files:**
- Modify: `src/db.rs`
- Test: `src/db.rs`

- [ ] **Step 1: Write failing tests for emergency cleanup, host reconciliation, and disabled limits**

```rust
#[test]
fn test_enforce_storage_budget_deletes_by_received_at_until_recovery_target() {
    let (pool, _dir) = test_pool();
    seed_logs_for_cleanup(&pool);

    let outcome = enforce_storage_budget(&pool, &storage_config_for_test()).unwrap();

    assert!(outcome.deleted_rows > 0);
    assert!(outcome.metrics.logical_db_size_bytes <= outcome.targets.recovery_db_size_bytes);
}

#[test]
fn test_enforce_storage_budget_reconciles_hosts_after_deletes() {
    let (pool, _dir) = test_pool();
    seed_logs_for_cleanup(&pool);

    enforce_storage_budget(&pool, &storage_config_for_test()).unwrap();

    let hosts = list_hosts(&pool).unwrap();
    assert!(hosts.iter().all(|host| host.hostname != "deleted-host"));
    let surviving = hosts.iter().find(|host| host.hostname == "surviving-host").unwrap();
    assert_eq!(surviving.log_count, 1);
    assert_eq!(surviving.first_seen, "2026-01-02T00:00:00Z");
    assert_eq!(surviving.last_seen, "2026-01-02T00:00:00Z");
}

#[test]
fn test_enforce_storage_budget_is_noop_when_limits_disabled() {
    let (pool, _dir) = test_pool();
    let outcome = enforce_storage_budget(&pool, &disabled_storage_budget()).unwrap();
    assert_eq!(outcome.deleted_rows, 0);
    assert!(!outcome.write_blocked);
}

#[test]
fn test_enforce_storage_budget_recovers_when_free_disk_threshold_is_breached() {
    let (pool, _dir) = test_pool();
    seed_logs_for_cleanup(&pool);
    let probe = FakeDiskSpaceProbe::new()
        .with_free_bytes(64 * 1024 * 1024)
        .with_recovered_bytes(900 * 1024 * 1024);

    let outcome = enforce_storage_budget_with_probe(
        &pool,
        &free_disk_pressure_config_for_test(),
        &probe,
    ).unwrap();

    assert!(outcome.deleted_rows > 0);
    assert!(outcome.metrics.free_disk_bytes.unwrap() >= outcome.targets.recovery_free_disk_bytes);
}
```

- [ ] **Step 2: Run the delete-order / recovery-target test to verify it fails**

Run: `cargo test test_enforce_storage_budget_deletes_by_received_at_until_recovery_target -- --nocapture`
Expected: FAIL because the enforcement API does not exist.

- [ ] **Step 3: Run the host-reconciliation test to verify it fails**

Run: `cargo test test_enforce_storage_budget_reconciles_hosts_after_deletes -- --nocapture`
Expected: FAIL because the enforcement API and reconciliation do not exist.

- [ ] **Step 4: Run the free-disk cleanup test to verify it fails**

Run: `cargo test test_enforce_storage_budget_recovers_when_free_disk_threshold_is_breached -- --nocapture`
Expected: FAIL because the free-space probe seam and cleanup path do not exist.

- [ ] **Step 5: Implement chunked emergency cleanup and host reconciliation**

```rust
pub fn enforce_storage_budget(pool: &DbPool, config: &StorageConfig) -> Result<StorageEnforcementOutcome> {
    let mut total_deleted = 0usize;
    loop {
        let health = evaluate_storage_health(pool, config)?;
        if health.is_within_recovery_targets() {
            return Ok(StorageEnforcementOutcome::healthy(health, total_deleted));
        }

        let deleted_hostnames = delete_oldest_logs_chunk(pool, 10_000)?;
        total_deleted += deleted_hostnames.deleted_rows;
        reconcile_hosts(pool, &deleted_hostnames.hostnames)?;
        checkpoint_wal_and_incremental_vacuum(pool)?;
    }
}
```

- [ ] **Step 6: Add a test seam for free-space probes and bounded incremental vacuum**

```rust
trait DiskSpaceProbe {
    fn free_bytes(&self, path: &Path) -> Result<u64>;
}
```

- [ ] **Step 7: Re-run the delete-order / recovery-target test**

Run: `cargo test test_enforce_storage_budget_deletes_by_received_at_until_recovery_target -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Re-run the host-reconciliation test**

Run: `cargo test test_enforce_storage_budget_reconciles_hosts_after_deletes -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Re-run the free-disk cleanup test**

Run: `cargo test test_enforce_storage_budget_recovers_when_free_disk_threshold_is_breached -- --nocapture`
Expected: PASS.

- [ ] **Step 10: Re-run the disabled-limit test**

Run: `cargo test test_enforce_storage_budget_is_noop_when_limits_disabled -- --nocapture`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add src/db.rs
git commit -m "feat: add storage budget cleanup engine"
```

### Task 4: Wire Periodic Enforcement and Batch-Writer Hard Stop

**Files:**
- Modify: `src/main.rs`
- Modify: `src/syslog.rs`
- Modify: `src/config.rs`
- Modify: `src/db.rs`
- Test: `src/db.rs`
- Test: `src/syslog.rs`

- [ ] **Step 1: Write failing tests for write blocking and write resumption**

```rust
#[tokio::test]
async fn flush_batch_retains_entries_while_storage_is_write_blocked() {
    let (pool, _dir) = test_pool();
    let pool = Arc::new(pool);
    let mut batch = vec![make_entry("2026-01-01T00:00:01Z", "host-a", "err", "disk full")];

    flush_batch_with_storage_guard(&pool, &mut batch).await;

    assert_eq!(batch.len(), 1);
}

#[tokio::test]
async fn flush_batch_resumes_after_storage_recovers() {
    let (pool, _dir) = test_pool();
    let pool = Arc::new(pool);
    let mut batch = vec![make_entry("2026-01-01T00:00:01Z", "host-a", "info", "recovered")];

    mark_storage_healthy_for_test(&pool);
    flush_batch_with_storage_guard(&pool, &mut batch).await;

    assert!(batch.is_empty());
}
```

- [ ] **Step 2: Run the blocked-write test to verify it fails**

Run: `cargo test flush_batch_retains_entries_while_storage_is_write_blocked -- --nocapture`
Expected: FAIL because there is no write-block path yet.

- [ ] **Step 3: Add periodic storage enforcement task in `main.rs`**

```rust
let storage_handle = {
    let pool = pool.clone();
    let storage = config.storage.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(storage.cleanup_interval_secs));
        loop {
            interval.tick().await;
            let pool = Arc::clone(&pool);
            let storage = storage.clone();
            let _ = tokio::task::spawn_blocking(move || db::enforce_storage_budget(&pool, &storage)).await;
        }
    })
};
```

- [ ] **Step 4: Thread `StorageConfig` into `syslog::start`, `batch_writer`, and `flush_batch`**

```rust
pub async fn start(
    config: SyslogConfig,
    storage: StorageConfig,
    pool: Arc<DbPool>,
) -> Result<()> {
    tokio::spawn(async move {
        batch_writer(rx, writer_pool, storage, batch_size, flush_interval).await;
    });
}
```

- [ ] **Step 5: Guard `flush_batch` with storage-health checks and state-transition logging**

```rust
match db::check_write_budget(&pool, &storage_config) {
    Ok(guard) if guard.write_blocked => {
        error!(logical_db_size = guard.metrics.logical_db_size_bytes, "storage budget exceeded; retaining batch");
        *batch = failed_batch;
        return;
    }
    Ok(_) => { /* insert normally */ }
    Err(e) => { /* fail closed and retain batch */ }
}
```

- [ ] **Step 6: Re-run the blocked-write test**

Run: `cargo test flush_batch_retains_entries_while_storage_is_write_blocked -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Re-run the resumed-write test**

Run: `cargo test flush_batch_resumes_after_storage_recovers -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Run broader DB regression coverage**

Run: `cargo test db::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Run broader syslog regression coverage**

Run: `cargo test syslog::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add src/main.rs src/syslog.rs src/config.rs src/db.rs
git commit -m "feat: enforce storage budget during writes"
```

### Task 5: Expose Storage State via MCP Stats

**Files:**
- Modify: `src/db.rs`
- Modify: `src/mcp.rs`
- Modify: `src/config.rs`
- Test: `src/db.rs`
- Test: `src/mcp.rs`

- [ ] **Step 1: Write failing tests for the expanded stats payload**

```rust
#[test]
fn test_get_stats_includes_logical_and_physical_sizes() {
    let (pool, _dir) = test_pool();
    let stats = get_stats(&pool, &storage_config_for_test()).unwrap();
    assert!(!stats.logical_db_size_mb.is_empty());
    assert!(!stats.physical_db_size_mb.is_empty());
}

#[tokio::test]
async fn tool_get_stats_returns_storage_guard_fields() {
    let state = AppState {
        pool: {
            let (pool, _dir) = test_pool();
            Arc::new(pool)
        },
        config: McpConfig {
            host: "0.0.0.0".into(),
            port: 3100,
            server_name: "syslog-mcp".into(),
            api_token: None,
        },
        storage: storage_config_for_test(),
    };
    let value = tool_get_stats(&state, serde_json::json!({})).await.unwrap();
    assert!(value.get("logical_db_size_mb").is_some());
    assert!(value.get("write_blocked").is_some());
}
```

- [ ] **Step 2: Run the stats tests to verify they fail**

Run: `cargo test get_stats -- --nocapture`
Expected: FAIL because the current stats object only returns `db_size_mb`.

- [ ] **Step 3: Expand `DbStats`, `get_stats`, and MCP metadata**

```rust
pub struct DbStats {
    pub total_logs: i64,
    pub total_hosts: i64,
    pub oldest_log: Option<String>,
    pub newest_log: Option<String>,
    pub logical_db_size_mb: String,
    pub physical_db_size_mb: String,
    pub free_disk_mb: Option<String>,
    pub max_db_size_mb: u64,
    pub min_free_disk_mb: u64,
    pub write_blocked: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<DbPool>,
    pub config: McpConfig,
    pub storage: StorageConfig,
}
```

- [ ] **Step 4: Re-run the stats tests**

Run: `cargo test get_stats -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Wire `StorageConfig` through `AppState` and `tool_get_stats`**

```rust
async fn tool_get_stats(state: &AppState, _args: Value) -> anyhow::Result<Value> {
    let storage = state.storage.clone();
    let stats = run_db(&state.pool, move |pool| db::get_stats(pool, &storage)).await?;
    Ok(serde_json::to_value(&stats)?)
}
```

- [ ] **Step 6: Re-run the MCP stats test**

Run: `cargo test tool_get_stats_returns_storage_guard_fields -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/db.rs src/mcp.rs src/config.rs
git commit -m "feat: expose storage guard stats"
```

### Task 6: Update Documentation and Run Final Verification

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `config.toml`

- [ ] **Step 1: Update operator-facing docs for the new storage guardrail**

```md
- `SYSLOG_MCP_MAX_DB_SIZE_MB=1024`
- `SYSLOG_MCP_RECOVERY_DB_SIZE_MB=900`
- `SYSLOG_MCP_MIN_FREE_DISK_MB=512`
- `SYSLOG_MCP_RECOVERY_FREE_DISK_MB=768`
- `SYSLOG_MCP_CLEANUP_INTERVAL_SECS=60`

Emergency cleanup permanently deletes oldest logs by `received_at`.
If cleanup cannot restore budget, syslog writes are blocked until the store is healthy again.
```

- [ ] **Step 2: Run formatting**

Run: `cargo fmt`
Expected: no output; code formatted.

- [ ] **Step 3: Run full tests**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Run lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md config.toml src/config.rs src/db.rs src/main.rs src/mcp.rs src/syslog.rs
git commit -m "docs: document storage budget guardrail"
```
