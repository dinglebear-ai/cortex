# Syslog Service Module Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the shared log business layer out of the single `src/app.rs` file into a focused `src/app/` module tree and rename `LogService` to `SyslogService`.

**Architecture:** Keep `src/app/` as the shared application boundary used by MCP, CLI, and API. Split models, errors, timestamp helpers, correlation helpers, and service orchestration into separate files while preserving the public `syslog_mcp::app::*` API shape through re-exports. This is a pure organization and naming refactor; MCP tool names, API routes, CLI commands, JSON response shapes, and DB behavior must not change.

**Tech Stack:** Rust 2021, Tokio, Axum, serde, chrono, rusqlite/r2d2, existing sidecar unit-test pattern.

---

## File Structure

Create and modify these files:

- Create: `src/app/mod.rs`
  - Owns public re-exports and `#[cfg(test)]` sidecar inclusion.
- Create: `src/app/error.rs`
  - Owns `ServiceError` and `ServiceResult`.
- Create: `src/app/models.rs`
  - Owns all shared request/response/data transfer structs.
- Create: `src/app/time.rs`
  - Owns RFC3339 parsing and UTC normalization helpers.
- Create: `src/app/correlate.rs`
  - Owns correlation helper functions: severity threshold expansion and BTreeMap grouping.
- Create: `src/app/service.rs`
  - Owns `SyslogService`, DB backpressure, and public service methods.
- Move: `src/app_tests.rs` -> `src/app/tests.rs`
  - Keep the same unit-test behavior, but update imports for the module split and `SyslogService` rename.
- Delete: `src/app.rs`
  - Replaced by `src/app/mod.rs` folder module.
- Modify: `src/lib.rs`
  - `pub mod app;` should continue to resolve to `src/app/mod.rs`; no public module name change.
- Modify: `src/runtime.rs`
  - Replace `LogService` imports/fields/constructors with `SyslogService`.
- Modify: `src/mcp.rs`, `src/mcp/tools.rs`, `src/mcp/*_tests.rs`
  - Replace `LogService` references with `SyslogService`; leave MCP behavior unchanged.
- Modify: `src/api.rs`, `src/api_tests.rs`
  - Replace `LogService` references with `SyslogService`; leave routes/status mapping unchanged.
- Modify: `src/bin/syslog-cli.rs`
  - Continue importing request types from `syslog_mcp::app`; no behavior change expected.
- Modify: `docs/mcp/PATTERNS.md`
  - Replace `LogService` wording with `SyslogService` and mention the split `src/app/` module.

Do not touch:

- `src/db/*` SQL/query internals except imports required by the service implementation.
- `src/syslog/*` ingest/parser behavior.
- MCP protocol envelope handling in `src/mcp/protocol.rs`.
- Existing dirty local files unrelated to this refactor, especially `src/syslog/parser_tests.rs` and `.worktree/`.

---

### Task 1: Add Focused App Module Files

**Files:**
- Create: `src/app/mod.rs`
- Create: `src/app/error.rs`
- Create: `src/app/models.rs`
- Create: `src/app/time.rs`
- Create: `src/app/correlate.rs`
- Create: `src/app/service.rs`
- Modify later: `src/app.rs`

- [ ] **Step 1: Create `src/app/mod.rs` with module declarations and re-exports**

```rust
mod correlate;
mod error;
mod models;
mod service;
mod time;

pub use error::{ServiceError, ServiceResult};
pub use models::{
    CorrelateEventsRequest, CorrelateEventsResponse, CorrelatedHost, DbStats, ErrorSummaryEntry,
    GetErrorsRequest, GetErrorsResponse, HostEntry, ListHostsResponse, LogEntry,
    SearchLogsRequest, SearchLogsResponse, TailLogsRequest,
};
pub use service::SyslogService;
pub use time::parse_optional_timestamp;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
```

- [ ] **Step 2: Create `src/app/error.rs`**

```rust
use std::fmt;

#[derive(Debug)]
pub enum ServiceError {
    InvalidInput(String),
    Busy(String),
    Internal(anyhow::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) | Self::Busy(msg) => f.write_str(msg),
            Self::Internal(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(err) => err.source(),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for ServiceError {
    fn from(value: anyhow::Error) -> Self {
        Self::Internal(value)
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;
```

- [ ] **Step 3: Create `src/app/models.rs`**

```rust
use serde::{Deserialize, Serialize};

use crate::db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: String,
    pub hostname: String,
    pub facility: Option<String>,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub message: String,
    pub received_at: String,
    pub source_ip: String,
}

impl From<db::LogEntry> for LogEntry {
    fn from(value: db::LogEntry) -> Self {
        Self {
            id: value.id,
            timestamp: value.timestamp,
            hostname: value.hostname,
            facility: value.facility,
            severity: value.severity,
            app_name: value.app_name,
            process_id: value.process_id,
            message: value.message,
            received_at: value.received_at,
            source_ip: value.source_ip,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchLogsRequest {
    pub query: Option<String>,
    pub hostname: Option<String>,
    pub source_ip: Option<String>,
    pub severity: Option<String>,
    pub app_name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchLogsResponse {
    pub count: usize,
    pub logs: Vec<LogEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailLogsRequest {
    pub hostname: Option<String>,
    pub source_ip: Option<String>,
    pub app_name: Option<String>,
    pub n: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSummaryEntry {
    pub hostname: String,
    pub severity: String,
    pub count: i64,
}

impl From<db::ErrorSummaryEntry> for ErrorSummaryEntry {
    fn from(value: db::ErrorSummaryEntry) -> Self {
        Self {
            hostname: value.hostname,
            severity: value.severity,
            count: value.count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetErrorsRequest {
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetErrorsResponse {
    pub summary: Vec<ErrorSummaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub log_count: i64,
}

impl From<db::HostEntry> for HostEntry {
    fn from(value: db::HostEntry) -> Self {
        Self {
            hostname: value.hostname,
            first_seen: value.first_seen,
            last_seen: value.last_seen,
            log_count: value.log_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHostsResponse {
    pub hosts: Vec<HostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelateEventsRequest {
    pub reference_time: String,
    pub window_minutes: Option<u32>,
    pub severity_min: Option<String>,
    pub hostname: Option<String>,
    pub source_ip: Option<String>,
    pub query: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelatedHost {
    pub hostname: String,
    pub event_count: usize,
    pub events: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelateEventsResponse {
    pub reference_time: String,
    pub window_minutes: u32,
    pub window_from: String,
    pub window_to: String,
    pub severity_min: String,
    pub total_events: usize,
    pub truncated: bool,
    pub hosts_count: usize,
    pub hosts: Vec<CorrelatedHost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub phantom_fts_rows: i64,
}

impl From<db::DbStats> for DbStats {
    fn from(value: db::DbStats) -> Self {
        Self {
            total_logs: value.total_logs,
            total_hosts: value.total_hosts,
            oldest_log: value.oldest_log,
            newest_log: value.newest_log,
            logical_db_size_mb: value.logical_db_size_mb,
            physical_db_size_mb: value.physical_db_size_mb,
            free_disk_mb: value.free_disk_mb,
            max_db_size_mb: value.max_db_size_mb,
            min_free_disk_mb: value.min_free_disk_mb,
            write_blocked: value.write_blocked,
            phantom_fts_rows: value.phantom_fts_rows,
        }
    }
}
```

- [ ] **Step 4: Create `src/app/time.rs`**

```rust
use chrono::{DateTime, Utc};

use super::{ServiceError, ServiceResult};

pub fn parse_optional_timestamp(raw: Option<&str>, field: &str) -> ServiceResult<Option<String>> {
    raw.map(|value| {
        DateTime::parse_from_rfc3339(value)
            .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
            .map_err(|e| ServiceError::InvalidInput(format!("Invalid {field} '{value}': {e}")))
    })
    .transpose()
}
```

- [ ] **Step 5: Create `src/app/correlate.rs`**

```rust
use std::collections::BTreeMap;

use super::{CorrelatedHost, LogEntry, ServiceError, ServiceResult};
use crate::db;

pub(super) fn severity_threshold_levels(severity_min: &str) -> ServiceResult<Vec<String>> {
    let threshold = db::severity_to_num(severity_min).ok_or_else(|| {
        ServiceError::InvalidInput(format!(
            "Invalid severity_min '{}'. Must be one of: emerg, alert, crit, err, warning, notice, info, debug",
            severity_min
        ))
    })?;

    Ok(db::SEVERITY_LEVELS[..=threshold as usize]
        .iter()
        .map(|&s| s.to_string())
        .collect())
}

pub(super) fn group_by_host(logs: &[LogEntry]) -> Vec<CorrelatedHost> {
    let mut by_host: BTreeMap<String, Vec<LogEntry>> = BTreeMap::new();
    for log in logs {
        by_host
            .entry(log.hostname.clone())
            .or_default()
            .push(log.clone());
    }

    by_host
        .into_iter()
        .map(|(hostname, events)| CorrelatedHost {
            event_count: events.len(),
            hostname,
            events,
        })
        .collect()
}
```

- [ ] **Step 6: Run format to verify the new files parse**

Run:

```bash
cargo fmt --all
```

Expected: exits 0.

- [ ] **Step 7: Commit module scaffolding**

```bash
git add src/app/mod.rs src/app/error.rs src/app/models.rs src/app/time.rs src/app/correlate.rs
git commit -m "refactor: scaffold app service modules"
```

---

### Task 2: Move Service Implementation and Rename `LogService`

**Files:**
- Create/modify: `src/app/service.rs`
- Delete: `src/app.rs`
- Modify: `src/app/mod.rs`
- Move: `src/app_tests.rs` -> `src/app/tests.rs`

- [ ] **Step 1: Write the compile target before moving code**

Run:

```bash
cargo check --all-targets
```

Expected: fails before this task is complete because both `src/app.rs` and `src/app/mod.rs` cannot define the same module once both exist.

- [ ] **Step 2: Move the service implementation into `src/app/service.rs`**

Use this structure in `src/app/service.rs`. Copy method bodies from the current `impl LogService` in `src/app.rs`, changing the type name to `SyslogService` and imports to module-local paths:

```rust
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use tokio::sync::Semaphore;

use super::correlate::{group_by_host, severity_threshold_levels};
use super::models::{
    CorrelateEventsRequest, CorrelateEventsResponse, DbStats, GetErrorsRequest,
    GetErrorsResponse, ListHostsResponse, SearchLogsRequest, SearchLogsResponse, TailLogsRequest,
};
use super::time::parse_optional_timestamp;
use super::{LogEntry, ServiceError, ServiceResult};
use crate::config::StorageConfig;
use crate::db::{self, DbPool, SearchParams};

const DB_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct SyslogService {
    pool: Arc<DbPool>,
    storage: StorageConfig,
    db_permits: Arc<Semaphore>,
    acquire_timeout: Duration,
}

impl SyslogService {
    pub(crate) fn new(pool: Arc<DbPool>, storage: StorageConfig) -> Self {
        let permits = storage.pool_size.max(1) as usize;
        Self {
            pool,
            storage,
            db_permits: Arc::new(Semaphore::new(permits)),
            acquire_timeout: DB_ACQUIRE_TIMEOUT,
        }
    }

    async fn run_db<F, T>(&self, f: F) -> ServiceResult<T>
    where
        F: FnOnce(&DbPool) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = tokio::time::timeout(
            self.acquire_timeout,
            Arc::clone(&self.db_permits).acquire_owned(),
        )
        .await
        .map_err(|_| ServiceError::Busy("database worker limit reached".into()))?
        .map_err(|_| ServiceError::Busy("database worker limit closed".into()))?;
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(&pool)
        })
        .await
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("Task join error: {e}")))?
        .map_err(ServiceError::Internal)
    }

    pub async fn health_check(&self) -> ServiceResult<()> {
        self.run_db(|pool| {
            let conn = pool.get()?;
            conn.query_row("SELECT 1", [], |_| Ok(()))?;
            Ok(())
        })
        .await
    }

    pub async fn search_logs(&self, req: SearchLogsRequest) -> ServiceResult<SearchLogsResponse> {
        let params = SearchParams {
            query: req.query,
            hostname: req.hostname,
            source_ip: req.source_ip,
            severity: req.severity,
            severity_in: None,
            app_name: req.app_name,
            from: parse_optional_timestamp(req.from.as_deref(), "from")?,
            to: parse_optional_timestamp(req.to.as_deref(), "to")?,
            limit: req.limit,
        };
        let logs = self.run_db(move |pool| db::search_logs(pool, &params)).await?;
        let logs: Vec<LogEntry> = logs.into_iter().map(Into::into).collect();
        Ok(SearchLogsResponse {
            count: logs.len(),
            logs,
        })
    }

    pub async fn tail_logs(&self, req: TailLogsRequest) -> ServiceResult<SearchLogsResponse> {
        let logs = self
            .run_db(move |pool| {
                db::tail_logs(
                    pool,
                    req.hostname.as_deref(),
                    req.source_ip.as_deref(),
                    req.app_name.as_deref(),
                    req.n.unwrap_or(50),
                )
            })
            .await?;
        let logs: Vec<LogEntry> = logs.into_iter().map(Into::into).collect();
        Ok(SearchLogsResponse {
            count: logs.len(),
            logs,
        })
    }

    pub async fn get_errors(&self, req: GetErrorsRequest) -> ServiceResult<GetErrorsResponse> {
        let from = parse_optional_timestamp(req.from.as_deref(), "from")?;
        let to = parse_optional_timestamp(req.to.as_deref(), "to")?;
        let rows = self
            .run_db(move |pool| db::get_error_summary(pool, from.as_deref(), to.as_deref()))
            .await?;
        Ok(GetErrorsResponse {
            summary: rows.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn list_hosts(&self) -> ServiceResult<ListHostsResponse> {
        let rows = self.run_db(db::list_hosts).await?;
        Ok(ListHostsResponse {
            hosts: rows.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn correlate_events(
        &self,
        req: CorrelateEventsRequest,
    ) -> ServiceResult<CorrelateEventsResponse> {
        let reference = chrono::DateTime::parse_from_rfc3339(&req.reference_time)
            .map_err(|e| {
                ServiceError::InvalidInput(format!(
                    "Invalid reference_time '{}': {e}",
                    req.reference_time
                ))
            })?
            .with_timezone(&chrono::Utc);
        let window_minutes = req.window_minutes.unwrap_or(5).min(60);
        let delta = TimeDelta::try_minutes(window_minutes as i64)
            .ok_or_else(|| ServiceError::InvalidInput("duration overflow".into()))?;
        let from = (reference - delta).to_rfc3339();
        let to = (reference + delta).to_rfc3339();
        let severity_min = req.severity_min.unwrap_or_else(|| "warning".into());
        let severity_levels = severity_threshold_levels(&severity_min)?;
        let limit = req.limit.unwrap_or(500).min(999);

        let params = SearchParams {
            query: req.query,
            hostname: req.hostname,
            source_ip: req.source_ip,
            severity: None,
            severity_in: Some(severity_levels),
            app_name: None,
            from: Some(from.clone()),
            to: Some(to.clone()),
            limit: Some(limit + 1),
        };

        let mut logs: Vec<LogEntry> = self
            .run_db(move |pool| db::search_logs(pool, &params))
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        let truncated = logs.len() > limit as usize;
        logs.truncate(limit as usize);
        let hosts = group_by_host(&logs);

        Ok(CorrelateEventsResponse {
            reference_time: reference.to_rfc3339(),
            window_minutes,
            window_from: from,
            window_to: to,
            severity_min,
            total_events: logs.len(),
            truncated,
            hosts_count: hosts.len(),
            hosts,
        })
    }

    pub async fn get_stats(&self) -> ServiceResult<DbStats> {
        let storage = self.storage.clone();
        let stats = self.run_db(move |pool| db::get_stats(pool, &storage)).await?;
        Ok(stats.into())
    }
}
```

- [ ] **Step 3: Move tests**

Run:

```bash
git mv src/app_tests.rs src/app/tests.rs
```

Then update the helper in `src/app/tests.rs`:

```rust
fn test_service() -> (SyslogService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("app-test.db"));
    let pool = Arc::new(db::init_pool(&storage).unwrap());
    (SyslogService::new(pool, storage), dir)
}
```

- [ ] **Step 4: Delete `src/app.rs`**

Run:

```bash
git rm src/app.rs
```

- [ ] **Step 5: Run app-focused tests**

Run:

```bash
cargo test app::tests
```

Expected: all app tests pass.

- [ ] **Step 6: Commit service split**

```bash
git add src/app src/app.rs
git commit -m "refactor: split syslog app service module"
```

---

### Task 3: Update Callers from `LogService` to `SyslogService`

**Files:**
- Modify: `src/runtime.rs`
- Modify: `src/mcp.rs`
- Modify: `src/api.rs`
- Modify: `src/mcp/tools_tests.rs`
- Modify: `src/mcp/protocol_tests.rs`
- Modify: `src/mcp/routes_tests.rs`
- Modify: `src/api_tests.rs`

- [ ] **Step 1: Update `src/runtime.rs` imports and fields**

Replace:

```rust
use crate::app::LogService;
```

with:

```rust
use crate::app::SyslogService;
```

Replace:

```rust
service: LogService,
```

with:

```rust
service: SyslogService,
```

Replace:

```rust
let service = LogService::new(Arc::clone(&pool), config.storage.clone());
```

with:

```rust
let service = SyslogService::new(Arc::clone(&pool), config.storage.clone());
```

Replace:

```rust
pub fn service(&self) -> LogService {
```

with:

```rust
pub fn service(&self) -> SyslogService {
```

- [ ] **Step 2: Update `src/mcp.rs`**

Replace:

```rust
use crate::app::LogService;
```

with:

```rust
use crate::app::SyslogService;
```

Replace:

```rust
pub service: LogService,
```

with:

```rust
pub service: SyslogService,
```

- [ ] **Step 3: Update `src/api.rs`**

Replace:

```rust
use crate::app::{
    CorrelateEventsRequest, GetErrorsRequest, LogService, SearchLogsRequest, TailLogsRequest,
};
```

with:

```rust
use crate::app::{
    CorrelateEventsRequest, GetErrorsRequest, SearchLogsRequest, SyslogService, TailLogsRequest,
};
```

Replace:

```rust
pub service: LogService,
```

with:

```rust
pub service: SyslogService,
```

- [ ] **Step 4: Update test constructors**

In `src/mcp/tools_tests.rs`, `src/mcp/protocol_tests.rs`, `src/mcp/routes_tests.rs`, and `src/api_tests.rs`, replace:

```rust
use crate::app::LogService;
```

with:

```rust
use crate::app::SyslogService;
```

Replace constructor calls:

```rust
LogService::new(pool, storage.clone())
```

with:

```rust
SyslogService::new(pool, storage.clone())
```

Replace:

```rust
crate::app::LogService::new(Arc::clone(&pool), storage)
```

with:

```rust
crate::app::SyslogService::new(Arc::clone(&pool), storage)
```

- [ ] **Step 5: Run a repository-wide stale name check**

Run:

```bash
rg -n "LogService" src docs README.md
```

Expected: only docs text not yet updated, or no matches in `src/`.

- [ ] **Step 6: Run compile verification**

Run:

```bash
cargo check --all-targets
```

Expected: exits 0.

- [ ] **Step 7: Commit caller rename**

```bash
git add src/runtime.rs src/mcp.rs src/api.rs src/mcp/*_tests.rs src/api_tests.rs
git commit -m "refactor: rename shared service to syslog service"
```

---

### Task 4: Update Docs and Surface References

**Files:**
- Modify: `docs/mcp/PATTERNS.md`
- Optional modify if search finds matches: `docs/plans/*.md`

- [ ] **Step 1: Update `docs/mcp/PATTERNS.md` shared boundary text**

Replace the section headed:

```markdown
## Shared LogService boundary
```

with:

```markdown
## Shared SyslogService boundary

MCP tools are adapters over the shared application layer in `src/app/`. Transport code extracts JSON arguments, calls `SyslogService`, and serializes typed responses back into MCP content envelopes:

```rust
let response = state
    .service
    .search_logs(SearchLogsRequest {
        query: string_arg(&args, "query"),
        hostname: string_arg(&args, "hostname"),
        source_ip: string_arg(&args, "source_ip"),
        severity: string_arg(&args, "severity"),
        app_name: string_arg(&args, "app_name"),
        from: string_arg(&args, "from"),
        to: string_arg(&args, "to"),
        limit: u32_arg(&args, "limit")?,
    })
    .await?;
```

`SyslogService` owns timestamp normalization, defaults, severity threshold expansion, correlation grouping, and bounded blocking DB execution. MCP should not call `DbPool` directly for log use cases.
```

- [ ] **Step 2: Decide whether historical plans should be changed**

Run:

```bash
rg -n "LogService" docs README.md
```

If matches are in historical dated plans under `docs/plans/`, leave them unchanged because they document prior planning context. If matches are in current operating docs, replace `LogService` with `SyslogService`.

- [ ] **Step 3: Run docs search**

Run:

```bash
rg -n "LogService|Shared LogService" docs README.md
```

Expected: no matches in current docs except historical dated plans, if any.

- [ ] **Step 4: Commit docs update**

```bash
git add docs/mcp/PATTERNS.md
git commit -m "docs: update shared syslog service boundary"
```

---

### Task 5: Full Verification

**Files:**
- No source edits expected.

- [ ] **Step 1: Format all Rust code**

Run:

```bash
cargo fmt --all
```

Expected: exits 0.

- [ ] **Step 2: Run full test suite**

Run:

```bash
cargo test
```

Expected: all tests pass. Current baseline after PR #7 was 120 library tests plus 1 CLI test.

- [ ] **Step 3: Run clippy**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: exits 0 with no warnings.

- [ ] **Step 4: Run version sync check**

Run:

```bash
bin/check-version-sync.sh .
```

Expected: exits 0. This refactor should not require a version bump unless the branch is being pushed under this repo's branch-push policy.

- [ ] **Step 5: Verify no accidental behavior/API rename leaked**

Run:

```bash
rg -n "search_logs|tail_logs|get_errors|list_hosts|correlate_events|get_stats|/api/search|/api/tail|/api/errors|/api/hosts|/api/correlate|/api/stats" src/mcp src/api src/bin
```

Expected: all existing MCP tool names, API paths, and CLI commands are still present.

- [ ] **Step 6: Verify local worktree cleanliness for files touched by this plan**

Run:

```bash
git status --short
```

Expected: only unrelated pre-existing dirty files remain, if any. At plan creation time those were `src/syslog/parser_tests.rs` and `.worktree/`; do not include them in commits for this refactor.

---

## Self-Review

**Spec coverage:** This plan covers the requested physical organization change by moving the business layer from `src/app.rs` into `src/app/`, and it covers the naming concern by renaming `LogService` to `SyslogService`. It preserves MCP/API/CLI behavior while keeping all surfaces pointed at the shared layer.

**Placeholder scan:** No task uses TBD/TODO/fill-in language. Each edit step names exact files, symbols, replacement code, and commands.

**Type consistency:** The new public type is consistently `SyslogService`. Existing request/response type names remain unchanged because they describe the existing log operations and are already part of the shared app API. `ServiceError` and `ServiceResult` remain unchanged to avoid unnecessary downstream churn.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-05-syslog-service-module-refactor.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
