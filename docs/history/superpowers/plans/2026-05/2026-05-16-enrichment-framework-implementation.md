# Enrichment Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a parser-dispatch pipeline that extracts structured fields from 6 known log sources (kernel, docker events, Authelia, SWAG/nginx, AdGuard, fail2ban) into 4 indexed columns plus per-source namespaces in `metadata_json`, so MCP queries can answer "show me every 5xx from SWAG this hour" with SQL instead of FTS scans.

**Architecture:** A `Parser` trait dispatched from a static table keyed on `(source_kind, app_name, container_name)`. Runs in-band on the writer hot path between AI-message scrubbing and SQL insert. Parser failure writes the row raw with a diagnostic in a `parse_error` column — never drops data.

**Tech Stack:** Rust 2021 + Tokio, `rusqlite` + `r2d2`, `serde_json`, `thiserror`, `regex`, `tracing`. Sidecar `*_tests.rs` per source file (existing convention).

**Spec:** `docs/superpowers/specs/2026-05-16-enrichment-framework-design.md`
**Contracts:** `docs/contracts/parser-trait.rs`, `docs/contracts/db-additions.sql`, `docs/contracts/source-kinds.md`, `docs/contracts/metadata-json-shape.md`, `docs/contracts/severity-mappings.md`

---

## File Structure

**New files:**
- `src/enrich/mod.rs` — module root, re-exports
- `src/enrich/parser.rs` — `Parser` trait + shared types (port of `docs/contracts/parser-trait.rs`)
- `src/enrich/parser_tests.rs` — trait/type sanity tests
- `src/enrich/dispatch.rs` — `EnrichmentPipeline`, `container_to_canonical` map, LRU debug
- `src/enrich/dispatch_tests.rs` — dispatch precedence + unknown-source tests
- `src/enrich/output.rs` — merge logic (ParserOutput → LogBatchEntry)
- `src/enrich/output_tests.rs` — merge edge cases
- `src/enrich/parsers/mod.rs` — parsers submodule root
- `src/enrich/parsers/kernel.rs` + `kernel_tests.rs`
- `src/enrich/parsers/docker_event.rs` + `docker_event_tests.rs`
- `src/enrich/parsers/authelia.rs` + `authelia_tests.rs`
- `src/enrich/parsers/swag.rs` + `swag_tests.rs`
- `src/enrich/parsers/adguard.rs` + `adguard_tests.rs`
- `src/enrich/parsers/fail2ban.rs` + `fail2ban_tests.rs`
- `tests/enrich_pipeline.rs` — end-to-end integration test
- `tests/fixtures/parsers/kernel/{oom_killed,link_up,link_down,mac_collision,unknown_kern}.txt`
- `tests/fixtures/parsers/docker_event/{die,oom,start,health_unhealthy,rename}.txt`
- `tests/fixtures/parsers/authelia/{1fa_success,1fa_failure,totp_success,totp_failure,health_probe}.json`, `text_mode_legacy.txt`
- `tests/fixtures/parsers/swag/{access_combined,access_combined_upstream,access_ipv6,access_escaped_quote,error_upstream_timeout,error_no_upstream}.txt`
- `tests/fixtures/parsers/adguard/{block,allow,rewrite,dnssec_failure,cached_hit,legacy_camelcase,api_poller_normalised}.json`, `truncated_invalid.txt`
- `tests/fixtures/parsers/fail2ban/{ban,unban,found,restore_ban,multi_ip_ban,error_line}.txt`

**Modified files:**
- `src/db/pool.rs` — add migration 10 block
- `src/db/models.rs` — extend `LogBatchEntry` with 5 new fields
- `src/db/ingest.rs` — INSERT SQL gains 5 placeholders
- `src/syslog/writer.rs` — call `EnrichmentPipeline::dispatch` after `enrich_entry`
- `src/syslog/parser.rs` — populate `source_kind` (kebab-case) on listener path
- `src/docker_ingest/parser.rs` — populate `source_kind = "docker-stream"` / `"docker-event"`
- `src/otlp.rs` — populate `source_kind = "otlp"`
- `src/lib.rs` — add `pub mod enrich;`
- `Cargo.toml` — add `thiserror = "1"` (not present today)
- `scripts/smoke-test.sh` — add synthetic SWAG line + new-column assertion

**Total:** 14 new files + 9 modified.

---

## Phase 1 — Schema and ingest path

### Task 1: Add `thiserror` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Grep Cargo.toml to confirm `thiserror` is not already present.**

```bash
grep -n '^thiserror' Cargo.toml || echo NOT_PRESENT
```

Expected: `NOT_PRESENT`.

- [ ] **Step 2: Add `thiserror` to `[dependencies]`.**

Open `Cargo.toml`. Under the `[dependencies]` table, in alphabetical position (after `tempfile` or wherever the `t` block sits), insert:

```toml
thiserror = "1"
```

- [ ] **Step 3: Verify it resolves.**

```bash
cargo check --offline 2>&1 | tail -20 || cargo check 2>&1 | tail -20
```

Expected: no error mentioning `thiserror`. If `--offline` fails with "registry index not found", drop the flag.

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add thiserror for enrichment-framework ParserError"
```

---

### Task 2: Migration 10 — add 5 columns + 4 partial indexes

**Files:**
- Modify: `src/db/pool.rs` (append after migration 9 block, around line 358-376)
- Reference: `docs/contracts/db-additions.sql` Epic B section

- [ ] **Step 1: Write a failing test asserting the columns exist after init_pool.**

Open `src/db/pool_tests.rs`. Append:

```rust
#[test]
fn migration_10_adds_enrichment_columns() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    let pool = init_pool(&config).expect("init_pool ok");
    let conn = pool.get().unwrap();

    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(logs)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    for expected in ["http_status", "auth_outcome", "dns_blocked", "event_action", "parse_error"] {
        assert!(cols.contains(&expected.to_string()), "missing column {expected}");
    }

    let indices: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='logs'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    for expected in [
        "idx_logs_http_status_time",
        "idx_logs_auth_outcome_time",
        "idx_logs_dns_blocked_time",
        "idx_logs_event_action_time",
    ] {
        assert!(indices.contains(&expected.to_string()), "missing index {expected}");
    }

    let version_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations WHERE version = 10", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version_count, 1, "migration 10 row not recorded");
}
```

- [ ] **Step 2: Run the test to confirm failure.**

```bash
cargo test --lib db::pool_tests::migration_10_adds_enrichment_columns
```

Expected: FAIL with "missing column http_status".

- [ ] **Step 3: Add migration 10 to `init_pool`.**

In `src/db/pool.rs`, find the migration 9 block (search for `INSERT INTO schema_migrations (version) VALUES (9);`). Immediately after it, add:

```rust
    // Migration 10: enrichment-framework columns + partial indexes.
    // Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md §5
    // Contract: docs/contracts/db-additions.sql Epic B section
    let already_applied_10: i64 = conn.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = 10",
        [],
        |r| r.get(0),
    )?;
    if already_applied_10 == 0 {
        conn.execute_batch(
            "ALTER TABLE logs ADD COLUMN http_status  INTEGER;
             ALTER TABLE logs ADD COLUMN auth_outcome TEXT;
             ALTER TABLE logs ADD COLUMN dns_blocked  INTEGER;
             ALTER TABLE logs ADD COLUMN event_action TEXT;
             ALTER TABLE logs ADD COLUMN parse_error  TEXT;

             CREATE INDEX IF NOT EXISTS idx_logs_http_status_time
                 ON logs(http_status, timestamp) WHERE http_status IS NOT NULL;
             CREATE INDEX IF NOT EXISTS idx_logs_auth_outcome_time
                 ON logs(auth_outcome, timestamp) WHERE auth_outcome IS NOT NULL;
             CREATE INDEX IF NOT EXISTS idx_logs_dns_blocked_time
                 ON logs(dns_blocked, timestamp) WHERE dns_blocked IS NOT NULL;
             CREATE INDEX IF NOT EXISTS idx_logs_event_action_time
                 ON logs(event_action, timestamp) WHERE event_action IS NOT NULL;

             INSERT INTO schema_migrations (version) VALUES (10);",
        )?;
    }
```

- [ ] **Step 4: Run the test to confirm pass.**

```bash
cargo test --lib db::pool_tests::migration_10_adds_enrichment_columns
```

Expected: PASS.

- [ ] **Step 5: Run the full pool test module to check no regressions.**

```bash
cargo test --lib db::pool_tests
```

Expected: all existing tests still pass.

- [ ] **Step 6: Commit.**

```bash
git add src/db/pool.rs src/db/pool_tests.rs
git commit -m "feat(db): migration 10 adds enrichment columns + partial indexes"
```

---

### Task 3: Extend `LogBatchEntry` with new fields

**Files:**
- Modify: `src/db/models.rs:13-33` (the `LogBatchEntry` struct)

- [ ] **Step 1: Write a failing test asserting the new fields exist with defaults.**

Open `src/db/models_tests.rs`. Append:

```rust
#[test]
fn log_batch_entry_has_enrichment_fields() {
    let entry = super::LogBatchEntry {
        timestamp: String::new(),
        hostname: String::new(),
        facility: None,
        severity: String::new(),
        app_name: None,
        process_id: None,
        message: String::new(),
        raw: String::new(),
        source_ip: String::new(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    };
    assert!(entry.http_status.is_none());
    assert!(entry.auth_outcome.is_none());
    assert!(entry.dns_blocked.is_none());
    assert!(entry.event_action.is_none());
    assert!(entry.parse_error.is_none());
}
```

- [ ] **Step 2: Run to confirm failure.**

```bash
cargo test --lib db::models_tests::log_batch_entry_has_enrichment_fields
```

Expected: FAIL with "no field http_status".

- [ ] **Step 3: Add fields to `LogBatchEntry`.**

In `src/db/models.rs`, locate the `LogBatchEntry` struct (around line 13-33). Add at the bottom of the field list (right before the closing brace):

```rust
    /// HTTP status code (3 digits). Indexed column. Set by `swag` parser.
    pub http_status: Option<i32>,

    /// Authentication outcome ("success" | "failure" | "denied" | "challenge").
    /// Indexed column. Set by `authelia` parser.
    pub auth_outcome: Option<&'static str>,

    /// DNS block decision. `Some(true)` = filtered/blocked, `Some(false)` = explicit
    /// allow, `None` = N/A (rewrites and non-DNS rows). Indexed column.
    pub dns_blocked: Option<bool>,

    /// Normalised event verb (closed enum per parser). Indexed column.
    pub event_action: Option<String>,

    /// Per-row parser diagnostic: "{parser_name}: {ParserError::Display}",
    /// truncated to 512 bytes. No index — diagnostic only.
    pub parse_error: Option<String>,
```

- [ ] **Step 4: Run test to confirm pass.**

```bash
cargo test --lib db::models_tests::log_batch_entry_has_enrichment_fields
```

Expected: PASS.

- [ ] **Step 5: Fix every other LogBatchEntry construction site (compile error sweep).**

```bash
cargo build 2>&1 | grep -E "(error\[E0063\]|missing field)" | head -30
```

For each compile error, add the 5 new fields initialized to `None`. Likely sites:
- `src/syslog/parser.rs` (when constructing entries from parsed RFC packets)
- `src/docker_ingest/parser.rs` (Docker stream + event paths)
- `src/otlp.rs` (OTLP records)
- `src/scanner/*.rs` (AI transcript ingest)
- Test files that construct entries

Use `Default::default()` style only when the entire struct already has a `Default` impl — `LogBatchEntry` does not, so explicitly initialise each.

- [ ] **Step 6: Build clean.**

```bash
cargo build 2>&1 | tail -10
```

Expected: no errors. Warnings about unused fields are fine until Task 5.

- [ ] **Step 7: Commit.**

```bash
git add -u
git commit -m "feat(db): extend LogBatchEntry with enrichment fields"
```

---

### Task 4: Wire new columns into `insert_logs_batch`

**Files:**
- Modify: `src/db/ingest.rs`

- [ ] **Step 1: Read the current INSERT statement.**

```bash
grep -n "INSERT INTO logs" src/db/ingest.rs
```

Note the column list and the parameter binding loop.

- [ ] **Step 2: Write a failing test that asserts the columns persist after insert.**

Open `src/db/ingest_tests.rs`. Append:

```rust
#[test]
fn insert_logs_batch_persists_enrichment_fields() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    let pool = crate::db::pool::init_pool(&config).unwrap();

    let entry = crate::db::LogBatchEntry {
        timestamp: "2026-05-16T10:00:00Z".to_string(),
        hostname: "test-host".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("swag".to_string()),
        process_id: None,
        message: "GET / 200".to_string(),
        raw: "raw line".to_string(),
        source_ip: "docker://localhost/swag/stdout".to_string(),
        docker_checkpoint: None,
        ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
        metadata_json: Some(r#"{"swag":{"method":"GET"}}"#.to_string()),
        http_status: Some(200),
        auth_outcome: None,
        dns_blocked: None,
        event_action: Some("http_request".to_string()),
        parse_error: None,
    };

    super::insert_logs_batch(&pool, &[entry]).expect("insert ok");

    let conn = pool.get().unwrap();
    let row: (Option<i32>, Option<String>, Option<i64>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT http_status, auth_outcome, dns_blocked, event_action, parse_error FROM logs LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row.0, Some(200));
    assert_eq!(row.1, None);
    assert_eq!(row.2, None);
    assert_eq!(row.3, Some("http_request".to_string()));
    assert_eq!(row.4, None);
}
```

- [ ] **Step 3: Run the test to confirm failure.**

```bash
cargo test --lib db::ingest_tests::insert_logs_batch_persists_enrichment_fields
```

Expected: FAIL (new columns not bound; default NULL).

- [ ] **Step 4: Update the INSERT in `insert_logs_batch`.**

Find the `INSERT INTO logs (...)` statement. Add 5 new columns to the column list and 5 placeholders. The new columns line up after `metadata_json`:

Column list addition: `, http_status, auth_outcome, dns_blocked, event_action, parse_error`

Placeholders: add `?, ?, ?, ?, ?` matching positions.

In the binding loop, bind:

```rust
stmt.execute(rusqlite::params![
    // ... existing bindings ...
    entry.metadata_json,
    entry.http_status,
    entry.auth_outcome,
    entry.dns_blocked.map(|b| b as i64),  // SQLite stores bools as int
    entry.event_action,
    entry.parse_error,
])?;
```

Adjust to match the actual statement structure (positional or named).

- [ ] **Step 5: Run the test to confirm pass.**

```bash
cargo test --lib db::ingest_tests::insert_logs_batch_persists_enrichment_fields
```

Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add src/db/ingest.rs src/db/ingest_tests.rs
git commit -m "feat(db): persist enrichment columns in insert_logs_batch"
```

---

## Phase 2 — Parser scaffolding

### Task 5: Create `src/enrich` module with Parser trait

**Files:**
- Create: `src/enrich/mod.rs`
- Create: `src/enrich/parser.rs`
- Create: `src/enrich/parser_tests.rs`
- Modify: `src/lib.rs` (add `pub mod enrich;`)
- Source: copy the type definitions from `docs/contracts/parser-trait.rs`

- [ ] **Step 1: Create `src/enrich/mod.rs`.**

```rust
//! Enrichment framework — parser dispatch on the writer hot path.
//!
//! Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md
//! Contract: docs/contracts/parser-trait.rs
//!
//! Architecture:
//!   LogBatchEntry → AI scrub (existing) → dispatcher → parser → merge into entry
//!
//! Parser failure does NOT drop the row — `parse_error` records the diagnostic
//! and the row is written with whatever fields the parser populated before
//! failing.

pub mod parser;
pub mod parsers;
pub mod dispatch;
pub mod output;

pub use parser::{AuthOutcome, Parser, ParserError, ParserId, ParserInput, ParserOutput, SourceKind};
pub use dispatch::EnrichmentPipeline;
```

- [ ] **Step 2: Create `src/enrich/parser.rs` by copying the contract.**

Open `docs/contracts/parser-trait.rs`. Copy the contents into `src/enrich/parser.rs`, but make the following adjustments:

1. Drop the leading module doc comment (the contract-only section).
2. Drop the `//! Compilation` and `//! Where this file lives` sections — those are contract-only.
3. Keep the `use serde::{Deserialize, Serialize};` and `use thiserror::Error;` lines.
4. Append at the very bottom:

```rust
#[cfg(test)]
#[path = "parser_tests.rs"]
mod parser_tests;
```

- [ ] **Step 3: Create `src/enrich/parser_tests.rs` with sanity tests.**

```rust
use super::{AuthOutcome, ParserError, ParserId, SourceKind};

#[test]
fn source_kind_as_str_matches_serde() {
    assert_eq!(SourceKind::SyslogUdp.as_str(), "syslog-udp");
    assert_eq!(SourceKind::DockerStream.as_str(), "docker-stream");
    assert_eq!(SourceKind::DockerEvent.as_str(), "docker-event");
    assert_eq!(SourceKind::AdguardApi.as_str(), "adguard-api");
    assert_eq!(SourceKind::UnifiApi.as_str(), "unifi-api");
}

#[test]
fn source_kind_is_syslog_covers_both() {
    assert!(SourceKind::SyslogUdp.is_syslog());
    assert!(SourceKind::SyslogTcp.is_syslog());
    assert!(!SourceKind::DockerStream.is_syslog());
}

#[test]
fn auth_outcome_as_str_round_trip() {
    for variant in [AuthOutcome::Success, AuthOutcome::Failure, AuthOutcome::Denied, AuthOutcome::Challenge] {
        let s = variant.as_str();
        let parsed: AuthOutcome = serde_json::from_str(&format!("\"{s}\"")).unwrap();
        assert_eq!(parsed, variant);
    }
}

#[test]
fn parser_id_as_str_matches_serde() {
    assert_eq!(ParserId::Kernel.as_str(), "kernel");
    assert_eq!(ParserId::DockerEvent.as_str(), "docker_event");
    assert_eq!(ParserId::Authelia.as_str(), "authelia");
    assert_eq!(ParserId::Swag.as_str(), "swag");
    assert_eq!(ParserId::Adguard.as_str(), "adguard");
    assert_eq!(ParserId::Fail2ban.as_str(), "fail2ban");
}

#[test]
fn parser_error_display_for_storage() {
    let err = ParserError::MissingField("http_status");
    assert_eq!(format!("{err}"), "missing required field: http_status");
}
```

- [ ] **Step 4: Add `pub mod enrich;` to `src/lib.rs`.**

Open `src/lib.rs`. After existing `pub mod` declarations, add:

```rust
pub mod enrich;
```

- [ ] **Step 5: Build and run the parser tests.**

```bash
cargo build
cargo test --lib enrich::parser_tests
```

Expected: build clean, 5/5 tests pass.

If you hit an error about `pub mod parsers;` or `pub mod dispatch;` not resolving — that's expected; those land in the next tasks. Create stub files now:

```bash
mkdir -p src/enrich/parsers
echo "//! Stub — parsers land in Phase 4." > src/enrich/parsers/mod.rs
echo "//! Stub — dispatcher lands in Task 6." > src/enrich/dispatch.rs
echo "//! Stub — output merge lands in Task 7." > src/enrich/output.rs
```

Then re-run the build. Adjust `mod.rs` re-exports if symbols don't exist yet (comment them out until the modules ship).

- [ ] **Step 6: Commit.**

```bash
git add src/enrich/ src/lib.rs
git commit -m "feat(enrich): parser trait + types ported from contract"
```

---

### Task 6: Empty dispatcher skeleton

**Files:**
- Modify: `src/enrich/dispatch.rs`
- Create: `src/enrich/dispatch_tests.rs`

- [ ] **Step 1: Write a failing test asserting the empty dispatcher leaves entries unchanged.**

Create `src/enrich/dispatch_tests.rs`:

```rust
use crate::db::LogBatchEntry;
use crate::enrich::EnrichmentPipeline;

fn fixture_entry() -> LogBatchEntry {
    LogBatchEntry {
        timestamp: "2026-05-16T10:00:00Z".into(),
        hostname: "h".into(),
        facility: None,
        severity: "info".into(),
        app_name: Some("kernel".into()),
        process_id: None,
        message: "hello".into(),
        raw: "hello".into(),
        source_ip: "udp://127.0.0.1:5678".into(),
        docker_checkpoint: None,
        ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn empty_pipeline_leaves_entry_unchanged() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = fixture_entry();
    pipeline.dispatch(&mut entry);
    assert!(entry.http_status.is_none());
    assert!(entry.event_action.is_none());
    assert!(entry.parse_error.is_none());
    assert!(entry.metadata_json.is_none());
}
```

- [ ] **Step 2: Replace `src/enrich/dispatch.rs` stub with the skeleton.**

```rust
//! Dispatcher — picks a parser per `(source_kind, app_name, container_name)`
//! and merges its output onto the entry.
//!
//! Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md §4

use crate::db::LogBatchEntry;
use crate::enrich::Parser;

/// Singleton dispatcher. Built once at startup, then handed to the batch writer.
pub struct EnrichmentPipeline {
    // Populated in Task 18 (parser registration); empty for now.
    _parsers: Vec<&'static dyn Parser>,
}

impl EnrichmentPipeline {
    /// Build the dispatcher with the V1 parser set. For now empty.
    pub fn new() -> Self {
        Self { _parsers: Vec::new() }
    }

    /// Dispatch and merge. No-op while the parser table is empty (Phase 4 fills it).
    pub fn dispatch(&self, _entry: &mut LogBatchEntry) {
        // No parsers registered yet — leave entry as-is.
    }
}

impl Default for EnrichmentPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
```

- [ ] **Step 3: Run the test to confirm pass.**

```bash
cargo test --lib enrich::dispatch_tests::empty_pipeline_leaves_entry_unchanged
```

Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add src/enrich/dispatch.rs src/enrich/dispatch_tests.rs
git commit -m "feat(enrich): empty EnrichmentPipeline dispatcher"
```

---

### Task 7: `output.rs` — merge `ParserOutput` into `LogBatchEntry`

**Files:**
- Modify: `src/enrich/output.rs`
- Create: `src/enrich/output_tests.rs`

- [ ] **Step 1: Write failing tests.**

Create `src/enrich/output_tests.rs`:

```rust
use crate::db::LogBatchEntry;
use crate::enrich::{AuthOutcome, ParserOutput};
use serde_json::json;

fn blank_entry() -> LogBatchEntry {
    LogBatchEntry {
        timestamp: String::new(), hostname: String::new(), facility: None,
        severity: "info".into(), app_name: None, process_id: None,
        message: String::new(), raw: String::new(), source_ip: String::new(),
        docker_checkpoint: None, ai_tool: None, ai_project: None,
        ai_session_id: None, ai_transcript_path: None, metadata_json: None,
        http_status: None, auth_outcome: None, dns_blocked: None,
        event_action: None, parse_error: None,
    }
}

#[test]
fn merges_indexed_columns() {
    let mut entry = blank_entry();
    let out = ParserOutput {
        http_status: Some(404),
        auth_outcome: Some(AuthOutcome::Failure),
        dns_blocked: Some(true),
        event_action: Some("http_request".into()),
        severity: Some("err"),
        metadata: Default::default(),
    };
    super::merge_output(&mut entry, "swag", out);
    assert_eq!(entry.http_status, Some(404));
    assert_eq!(entry.auth_outcome, Some("failure"));
    assert_eq!(entry.dns_blocked, Some(true));
    assert_eq!(entry.event_action.as_deref(), Some("http_request"));
    assert_eq!(entry.severity, "err");
}

#[test]
fn merges_metadata_under_namespace() {
    let mut entry = blank_entry();
    let mut meta = serde_json::Map::new();
    meta.insert("method".into(), json!("GET"));
    meta.insert("path".into(), json!("/api"));
    let out = ParserOutput { metadata: meta, ..Default::default() };
    super::merge_output(&mut entry, "swag", out);

    let parsed: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["swag"]["method"], json!("GET"));
    assert_eq!(parsed["swag"]["path"], json!("/api"));
    assert_eq!(parsed["parser"]["name"], json!("swag"));
}

#[test]
fn preserves_existing_metadata_namespaces() {
    let mut entry = blank_entry();
    entry.metadata_json = Some(r#"{"docker":{"container_name":"swag"}}"#.into());
    let mut meta = serde_json::Map::new();
    meta.insert("method".into(), json!("GET"));
    let out = ParserOutput { metadata: meta, ..Default::default() };
    super::merge_output(&mut entry, "swag", out);

    let parsed: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["docker"]["container_name"], json!("swag"));
    assert_eq!(parsed["swag"]["method"], json!("GET"));
}

#[test]
fn record_error_writes_parse_error_truncated() {
    let mut entry = blank_entry();
    let long = "x".repeat(1000);
    super::record_error(&mut entry, "swag", &format!("structural: {long}"));
    let pe = entry.parse_error.unwrap();
    assert!(pe.starts_with("swag: structural: "));
    assert!(pe.len() <= 512);
}
```

- [ ] **Step 2: Run to confirm failure.**

```bash
cargo test --lib enrich::output_tests
```

Expected: FAIL with "function `merge_output` not found".

- [ ] **Step 3: Implement `src/enrich/output.rs`.**

```rust
//! Merge `ParserOutput` onto a `LogBatchEntry`.

use crate::db::LogBatchEntry;
use crate::enrich::ParserOutput;
use serde_json::{json, Value};

const PARSE_ERROR_MAX_BYTES: usize = 512;

/// Apply a parser's output to the entry. Caller passes the parser's namespace
/// key so the metadata fields land under the canonical owner.
pub fn merge_output(entry: &mut LogBatchEntry, namespace: &'static str, out: ParserOutput) {
    if let Some(v) = out.http_status {
        entry.http_status = Some(v);
    }
    if let Some(o) = out.auth_outcome {
        entry.auth_outcome = Some(o.as_str());
    }
    if let Some(v) = out.dns_blocked {
        entry.dns_blocked = Some(v);
    }
    if let Some(v) = out.event_action {
        entry.event_action = Some(v);
    }
    if let Some(s) = out.severity {
        entry.severity = s.to_string();
    }

    merge_metadata(entry, namespace, out.metadata);
}

fn merge_metadata(
    entry: &mut LogBatchEntry,
    namespace: &'static str,
    parser_fields: serde_json::Map<String, Value>,
) {
    let mut root: serde_json::Map<String, Value> = match &entry.metadata_json {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::Map::new()),
        None => serde_json::Map::new(),
    };

    if !parser_fields.is_empty() {
        root.insert(namespace.to_string(), Value::Object(parser_fields));
    }

    // Parser provenance.
    root.insert(
        "parser".to_string(),
        json!({"name": namespace, "version": 1}),
    );

    entry.metadata_json = Some(Value::Object(root).to_string());
}

/// Record a parser failure on the entry. Format: "{parser_name}: {error}",
/// truncated to PARSE_ERROR_MAX_BYTES.
pub fn record_error(entry: &mut LogBatchEntry, parser_name: &str, error: &str) {
    let mut s = format!("{parser_name}: {error}");
    if s.len() > PARSE_ERROR_MAX_BYTES {
        s.truncate(PARSE_ERROR_MAX_BYTES);
    }
    entry.parse_error = Some(s);
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod output_tests;
```

- [ ] **Step 4: Run tests to confirm pass.**

```bash
cargo test --lib enrich::output_tests
```

Expected: 4/4 PASS.

- [ ] **Step 5: Re-export `merge_output` and `record_error`.**

In `src/enrich/mod.rs`, change the existing `pub use` line to also re-export from `output`:

```rust
pub use output::{merge_output, record_error};
```

- [ ] **Step 6: Commit.**

```bash
git add src/enrich/output.rs src/enrich/output_tests.rs src/enrich/mod.rs
git commit -m "feat(enrich): merge_output + record_error helpers"
```

---

## Phase 3 — Wire the dispatcher into the writer

### Task 8: Populate `source_kind` on the listener and Docker paths

Existing entries don't carry a `source_kind` value the dispatcher can read. We store it in `metadata_json.source_kind` (per `docs/contracts/metadata-json-shape.md`) — a stamp added at ingest. No new column.

**Files:**
- Modify: `src/syslog/parser.rs` (UDP and TCP packet parsing — stamp `syslog-udp` or `syslog-tcp`)
- Modify: `src/docker_ingest/parser.rs` (stamp `docker-stream` for `log_output_to_entry`, `docker-event` for `docker_event_to_entry`)
- Modify: `src/otlp.rs` (stamp `otlp` on `/v1/logs` records)

- [ ] **Step 1: Add a helper for stamping source_kind into metadata_json.**

Add to `src/enrich/output.rs` (above the test module):

```rust
/// Stamp `source_kind` into the entry's metadata_json. Called once per ingest
/// path BEFORE the entry reaches the batch writer. Idempotent — if a value is
/// already present, leaves it (caller wins).
pub fn stamp_source_kind(entry: &mut LogBatchEntry, kind: crate::enrich::SourceKind) {
    let mut root: serde_json::Map<String, Value> = match &entry.metadata_json {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::Map::new()),
        None => serde_json::Map::new(),
    };
    if !root.contains_key("source_kind") {
        root.insert("source_kind".to_string(), Value::String(kind.as_str().to_string()));
        entry.metadata_json = Some(Value::Object(root).to_string());
    }
}
```

- [ ] **Step 2: Stamp at every ingest path. For each, locate the function that produces a `LogBatchEntry` and call `stamp_source_kind` just before returning.**

For `src/syslog/parser.rs` — find the UDP and TCP packet handlers (search for `LogBatchEntry {` constructions). After construction:

```rust
crate::enrich::stamp_source_kind(&mut entry, crate::enrich::SourceKind::SyslogUdp);
```

(or `SyslogTcp` for the TCP path).

For `src/docker_ingest/parser.rs` — find `log_output_to_entry` and `docker_event_to_entry`. Stamp `DockerStream` and `DockerEvent` respectively.

For `src/otlp.rs` — find where OTLP records are converted to `LogBatchEntry`. Stamp `Otlp`.

- [ ] **Step 3: Re-export `stamp_source_kind` from the module.**

Add to `src/enrich/mod.rs`:

```rust
pub use output::{merge_output, record_error, stamp_source_kind};
```

- [ ] **Step 4: Write a test that verifies stamping.**

Append to `src/enrich/output_tests.rs`:

```rust
#[test]
fn stamps_source_kind_in_metadata() {
    let mut entry = blank_entry();
    super::stamp_source_kind(&mut entry, crate::enrich::SourceKind::DockerStream);
    let parsed: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["source_kind"], serde_json::json!("docker-stream"));
}

#[test]
fn stamps_source_kind_idempotent() {
    let mut entry = blank_entry();
    entry.metadata_json = Some(r#"{"source_kind":"docker-event"}"#.into());
    super::stamp_source_kind(&mut entry, crate::enrich::SourceKind::SyslogUdp);
    let parsed: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["source_kind"], serde_json::json!("docker-event"));
}
```

- [ ] **Step 5: Run tests + full build.**

```bash
cargo build
cargo test --lib enrich::output_tests
```

Expected: 6/6 PASS, no build errors.

- [ ] **Step 6: Commit.**

```bash
git add -u src/syslog/parser.rs src/docker_ingest/parser.rs src/otlp.rs src/enrich/
git commit -m "feat(enrich): stamp source_kind at every ingest path"
```

---

### Task 9: Hook `EnrichmentPipeline::dispatch` into `flush_batch`

**Files:**
- Modify: `src/syslog/writer.rs` (around line 124-129 where `enrich_entry` runs)
- Modify: `WriterContext` (find in `src/syslog/writer.rs` or wherever it's defined) — add `pipeline: Arc<EnrichmentPipeline>` field
- Modify: wherever `WriterContext` is constructed (runtime.rs likely) — pass an `EnrichmentPipeline::new()`

- [ ] **Step 1: Locate `WriterContext`.**

```bash
grep -rn "struct WriterContext" src/
grep -rn "WriterContext {" src/
```

Note the file paths.

- [ ] **Step 2: Add `pipeline` field to `WriterContext`.**

In the struct definition:

```rust
pub enrichment: EnrichmentConfig,
+ pub pipeline: std::sync::Arc<crate::enrich::EnrichmentPipeline>,
pub storage: StorageConfig,
// ... rest as-is
```

- [ ] **Step 3: Wire pipeline through every `WriterContext { ... }` construction.**

For each site (likely `runtime.rs`), construct:

```rust
let pipeline = std::sync::Arc::new(crate::enrich::EnrichmentPipeline::new());
let context = WriterContext {
    // ... existing fields ...
    pipeline,
};
```

- [ ] **Step 4: Call `dispatch` in `flush_batch`.**

In `src/syslog/writer.rs::flush_batch`, the existing block (lines 124-129) is:

```rust
let batch_to_write: Vec<db::LogBatchEntry> = std::mem::take(batch)
    .into_iter()
    .map(|e| enrich_entry(e, &context.enrichment))
    .collect();
```

Replace with:

```rust
let batch_to_write: Vec<db::LogBatchEntry> = std::mem::take(batch)
    .into_iter()
    .map(|e| {
        let mut e = enrich_entry(e, &context.enrichment);
        context.pipeline.dispatch(&mut e);
        e
    })
    .collect();
```

- [ ] **Step 5: Build clean and run all writer tests.**

```bash
cargo build
cargo test --lib syslog::writer_tests
```

Expected: PASS. The empty pipeline is a no-op, so existing tests should not regress.

- [ ] **Step 6: Add an integration test.**

Append to `src/syslog/writer_tests.rs` (or create a new test if the file lacks setup):

```rust
#[tokio::test]
async fn dispatch_runs_on_batch_flush() {
    // This is a smoke test that the pipeline is invoked. With no parsers
    // registered, dispatch is a no-op — we just verify the call site
    // compiles and the batch flushes cleanly.
    // Real per-parser behaviour is covered in Phase 4 + tests/enrich_pipeline.rs.
}
```

(Trivial test — the value is in Phase 5's integration test.)

- [ ] **Step 7: Commit.**

```bash
git add -u
git commit -m "feat(syslog): wire EnrichmentPipeline into flush_batch"
```

---

## Phase 4 — Implement parsers (one per task)

Each parser task follows the same pattern: fixtures → failing test → implementation → pass → commit.

### Task 10: `kernel` parser

**Files:**
- Create: `src/enrich/parsers/kernel.rs`
- Create: `src/enrich/parsers/kernel_tests.rs`
- Create: `tests/fixtures/parsers/kernel/{oom_killed.txt,link_up.txt,link_down.txt,mac_collision.txt,unknown_kern.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures.**

```bash
mkdir -p tests/fixtures/parsers/kernel
```

Write each file:

`tests/fixtures/parsers/kernel/oom_killed.txt`:
```
Out of memory: Killed process 2475067 (postgres) total-vm:2484556kB, anon-rss:143224kB, file-rss:0kB, shmem-rss:452kB, UID:1011 pgtables:588kB oom_score_adj:900
```

`tests/fixtures/parsers/kernel/link_up.txt`:
```
eth0: link up, 1000Mbps, full-duplex, lpa 0x45E1
```

`tests/fixtures/parsers/kernel/link_down.txt`:
```
eth0: link down
```

`tests/fixtures/parsers/kernel/mac_collision.txt`:
```
br0: received packet on eth1 with own address as source address (addr:aa:bb:cc:dd:ee:ff, vlan:0)
```

`tests/fixtures/parsers/kernel/unknown_kern.txt`:
```
audit: type=1300 audit(1700000000.000:42): foo bar baz
```

- [ ] **Step 2: Write failing tests.**

Create `src/enrich/parsers/kernel_tests.rs`:

```rust
use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    let path = format!("tests/fixtures/parsers/kernel/{fixture}");
    std::fs::read_to_string(&path).expect(&path).trim().to_string()
}

fn parse(message: &str) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    let parser = super::KernelParser;
    let input = ParserInput {
        app_name: Some("kernel"),
        container_name: None,
        message,
        raw: message,
        source_kind: SourceKind::SyslogTcp,
        severity: "info",
    };
    parser.parse(&input)
}

#[test]
fn oom_kill_extracts_fields() {
    let msg = input_from("oom_killed.txt");
    let out = parse(&msg).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("oom_kill"));
    assert_eq!(out.metadata["pid"], serde_json::json!(2475067));
    assert_eq!(out.metadata["comm"], serde_json::json!("postgres"));
    assert_eq!(out.metadata["total_vm_kb"], serde_json::json!(2484556));
    assert_eq!(out.metadata["anon_rss_kb"], serde_json::json!(143224));
    assert_eq!(out.metadata["uid"], serde_json::json!(1011));
    assert_eq!(out.metadata["oom_score_adj"], serde_json::json!(900));
}

#[test]
fn link_up_extracts_speed() {
    let msg = input_from("link_up.txt");
    let out = parse(&msg).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("link_up"));
    assert_eq!(out.metadata["interface"], serde_json::json!("eth0"));
    assert_eq!(out.metadata["state"], serde_json::json!("up"));
    assert_eq!(out.metadata["speed_mbps"], serde_json::json!(1000));
}

#[test]
fn link_down_no_speed() {
    let msg = input_from("link_down.txt");
    let out = parse(&msg).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("link_down"));
    assert_eq!(out.metadata["interface"], serde_json::json!("eth0"));
    assert_eq!(out.metadata["state"], serde_json::json!("down"));
    assert!(out.metadata.get("speed_mbps").is_none());
}

#[test]
fn mac_collision_extracts_mac() {
    let msg = input_from("mac_collision.txt");
    let out = parse(&msg).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("mac_collision"));
    assert_eq!(out.metadata["interface"], serde_json::json!("br0"));
    assert_eq!(out.metadata["colliding_mac"], serde_json::json!("aa:bb:cc:dd:ee:ff"));
    assert_eq!(out.metadata["vlan"], serde_json::json!(0));
}

#[test]
fn unknown_kernel_message_returns_no_match() {
    let msg = input_from("unknown_kern.txt");
    let err = parse(&msg).unwrap_err();
    assert!(matches!(err, crate::enrich::ParserError::NoMatch(_)));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::kernel_tests
```

Expected: FAIL with "no struct `KernelParser`".

- [ ] **Step 4: Implement the parser.**

Create `src/enrich/parsers/kernel.rs`:

```rust
//! Linux kernel parser — OOM kills, link state, MAC collisions.
//! Spec §7.1.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct KernelParser;

static OOM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^Out of memory: Killed process (?P<pid>\d+) \((?P<comm>[^)]+)\) total-vm:(?P<vm>\d+)kB, anon-rss:(?P<rss>\d+)kB.* UID:(?P<uid>\d+).*oom_score_adj:(?P<adj>-?\d+)",
    )
    .expect("static regex")
});

static LINK_UP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<if>\w+): link up,\s*(?P<speed>\d+)Mbps").expect("static regex")
});

static LINK_DOWN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<if>\w+): link down").expect("static regex"));

static MAC_COLLISION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<if>\w+): received packet on \S+ with own address as source address \(addr:(?P<mac>[0-9a-f:]+)(?:, vlan:(?P<vlan>\d+))?\)",
    )
    .expect("static regex")
});

impl Parser for KernelParser {
    fn name(&self) -> &'static str { "kernel" }
    fn namespace(&self) -> &'static str { "kernel" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let msg = input.message;

        // Cheap prefix discrimination first.
        if msg.starts_with("Out of memory:") {
            return parse_oom(msg);
        }
        if msg.contains(": link up") || msg.contains(": link down") {
            return parse_link(msg);
        }
        if msg.contains("with own address as source address") {
            return parse_mac_collision(msg);
        }
        Err(ParserError::NoMatch("not a recognised kernel pattern"))
    }
}

fn parse_oom(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = OOM_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("oom_killer line malformed"))?;
    let mut metadata = Map::new();
    metadata.insert("pid".into(), json!(caps["pid"].parse::<i64>().unwrap_or(0)));
    metadata.insert("comm".into(), json!(&caps["comm"]));
    metadata.insert("total_vm_kb".into(), json!(caps["vm"].parse::<i64>().unwrap_or(0)));
    metadata.insert("anon_rss_kb".into(), json!(caps["rss"].parse::<i64>().unwrap_or(0)));
    metadata.insert("uid".into(), json!(caps["uid"].parse::<i32>().unwrap_or(0)));
    metadata.insert("oom_score_adj".into(), json!(caps["adj"].parse::<i32>().unwrap_or(0)));
    Ok(ParserOutput {
        event_action: Some("oom_kill".into()),
        severity: Some("crit"),
        metadata,
        ..Default::default()
    })
}

fn parse_link(msg: &str) -> Result<ParserOutput, ParserError> {
    let mut metadata = Map::new();
    if let Some(caps) = LINK_UP_RE.captures(msg) {
        metadata.insert("interface".into(), json!(&caps["if"]));
        metadata.insert("state".into(), json!("up"));
        if let Ok(speed) = caps["speed"].parse::<i32>() {
            metadata.insert("speed_mbps".into(), json!(speed));
        }
        return Ok(ParserOutput {
            event_action: Some("link_up".into()),
            metadata,
            ..Default::default()
        });
    }
    if let Some(caps) = LINK_DOWN_RE.captures(msg) {
        metadata.insert("interface".into(), json!(&caps["if"]));
        metadata.insert("state".into(), json!("down"));
        return Ok(ParserOutput {
            event_action: Some("link_down".into()),
            metadata,
            ..Default::default()
        });
    }
    Err(ParserError::NoMatch("link line malformed"))
}

fn parse_mac_collision(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = MAC_COLLISION_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("mac collision malformed"))?;
    let mut metadata = Map::new();
    metadata.insert("interface".into(), json!(&caps["if"]));
    metadata.insert("colliding_mac".into(), json!(&caps["mac"]));
    if let Some(vlan) = caps.name("vlan") {
        if let Ok(v) = vlan.as_str().parse::<i32>() {
            metadata.insert("vlan".into(), json!(v));
        }
    }
    Ok(ParserOutput {
        event_action: Some("mac_collision".into()),
        metadata,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "kernel_tests.rs"]
mod kernel_tests;
```

- [ ] **Step 5: Register the parser module.**

Edit `src/enrich/parsers/mod.rs`:

```rust
//! V1 parsers. Each parser is a zero-state singleton.

pub mod kernel;

pub use kernel::KernelParser;
```

- [ ] **Step 6: Run tests.**

```bash
cargo test --lib enrich::parsers::kernel
```

Expected: 5/5 PASS.

- [ ] **Step 7: Commit.**

```bash
git add src/enrich/parsers/kernel.rs src/enrich/parsers/kernel_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/kernel/
git commit -m "feat(enrich): kernel parser — OOM, link state, MAC collision"
```

---

### Task 11: `docker_event` parser

**Files:**
- Create: `src/enrich/parsers/docker_event.rs` + `docker_event_tests.rs`
- Create: `tests/fixtures/parsers/docker_event/{die.txt,oom.txt,start.txt,health_unhealthy.txt,rename.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures (the `message` strings produced by `src/docker_ingest/parser.rs::docker_event_to_entry`).**

```bash
mkdir -p tests/fixtures/parsers/docker_event
```

Write:

`die.txt`:
```
docker container event: die container=postgres image=postgres:16 compose_project=stack compose_service=db exit_code=137
```

`oom.txt`:
```
docker container event: oom container=plex image=plexinc/pms-docker:latest compose_project=media compose_service=plex
```

`start.txt`:
```
docker container event: start container=traefik image=traefik:v3.0 compose_project=edge compose_service=router
```

`health_unhealthy.txt`:
```
docker container event: health_status_unhealthy container=adguard image=adguard/adguardhome:latest compose_project=dns compose_service=adguard
```

`rename.txt`:
```
docker container event: rename container=swag image=lscr.io/linuxserver/swag:latest compose_project=edge compose_service=proxy old_name=nginx-proxy
```

- [ ] **Step 2: Write tests in `docker_event_tests.rs`.**

```rust
use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/docker_event/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(message: &str) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::DockerEventParser.parse(&ParserInput {
        app_name: Some("dockerd"),
        container_name: None,
        message,
        raw: message,
        source_kind: SourceKind::DockerEvent,
        severity: "info",
    })
}

#[test]
fn die_extracts_exit_code_and_severity() {
    let out = parse(&input_from("die.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("die"));
    assert_eq!(out.metadata["container_name"], serde_json::json!("postgres"));
    assert_eq!(out.metadata["image"], serde_json::json!("postgres:16"));
    assert_eq!(out.metadata["exit_code"], serde_json::json!(137));
    assert_eq!(out.severity, Some("warning"));
}

#[test]
fn oom_promotes_severity_to_crit() {
    let out = parse(&input_from("oom.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("oom"));
    assert_eq!(out.severity, Some("crit"));
}

#[test]
fn start_is_info_severity() {
    let out = parse(&input_from("start.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("start"));
    assert_eq!(out.severity, None);  // leave existing
}

#[test]
fn health_unhealthy_normalised() {
    let out = parse(&input_from("health_unhealthy.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("health_status_unhealthy"));
    assert_eq!(out.severity, Some("warning"));
}

#[test]
fn rename_captures_old_name() {
    let out = parse(&input_from("rename.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("rename"));
    assert_eq!(out.metadata["old_name"], serde_json::json!("nginx-proxy"));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::docker_event_tests
```

Expected: FAIL.

- [ ] **Step 4: Implement `src/enrich/parsers/docker_event.rs`.**

```rust
//! Docker lifecycle event parser. Spec §7.2.
//! Input format produced by src/docker_ingest/parser.rs::docker_event_to_entry:
//!   "docker container event: <action> container=X image=Y compose_project=Z compose_service=W [...attributes]"

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct DockerEventParser;

static EVENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^docker container event:\s+(?P<action>\S+)\s+(?P<attrs>.*)").expect("static regex")
});

static ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w+)=([^\s]+)").expect("static regex"));

impl Parser for DockerEventParser {
    fn name(&self) -> &'static str { "docker_event" }
    fn namespace(&self) -> &'static str { "docker" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let caps = EVENT_RE
            .captures(input.message)
            .ok_or(ParserError::NoMatch("not a docker event line"))?;
        let action = caps["action"].to_string();

        let mut metadata = Map::new();
        for m in ATTR_RE.captures_iter(&caps["attrs"]) {
            let key = m[1].to_string();
            let val = m[2].to_string();
            // Coerce known-int keys.
            if matches!(key.as_str(), "exit_code") {
                if let Ok(n) = val.parse::<i32>() {
                    metadata.insert(key, json!(n));
                    continue;
                }
            }
            metadata.insert(key, Value::String(val));
        }

        // Hoist container_name into the canonical key (some attr names came in as 'container').
        if let Some(Value::String(s)) = metadata.remove("container") {
            metadata.insert("container_name".to_string(), Value::String(s));
        }

        let severity = match action.as_str() {
            "oom" => Some("crit"),
            "die" | "kill" | "health_status_unhealthy" => Some("warning"),
            _ => None,
        };

        Ok(ParserOutput {
            event_action: Some(action),
            severity,
            metadata,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "docker_event_tests.rs"]
mod docker_event_tests;
```

- [ ] **Step 5: Register in `mod.rs`.**

```rust
pub mod docker_event;
pub use docker_event::DockerEventParser;
```

- [ ] **Step 6: Run tests.**

```bash
cargo test --lib enrich::parsers::docker_event
```

Expected: 5/5 PASS.

- [ ] **Step 7: Commit.**

```bash
git add src/enrich/parsers/docker_event.rs src/enrich/parsers/docker_event_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/docker_event/
git commit -m "feat(enrich): docker_event parser — lifecycle verbs"
```

---

### Task 12: `authelia` parser

**Files:**
- Create: `src/enrich/parsers/authelia.rs` + `authelia_tests.rs`
- Create: `tests/fixtures/parsers/authelia/{1fa_success.json,1fa_failure.json,totp_success.json,totp_failure.json,health_probe.json,text_mode_legacy.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures.**

```bash
mkdir -p tests/fixtures/parsers/authelia
```

`1fa_success.json`:
```
{"level":"info","msg":"Authentication attempt successful","method":"POST","path":"/api/firstfactor","remote_ip":"100.0.0.1","time":"2026-05-15T03:46:03Z","username":"alice"}
```

`1fa_failure.json`:
```
{"level":"error","msg":"Unsuccessful 1FA authentication attempt by user 'bob'","method":"POST","path":"/api/firstfactor","remote_ip":"203.0.113.7","time":"2026-05-15T03:46:11Z"}
```

`totp_success.json`:
```
{"level":"info","msg":"Authentication attempt successful","path":"/api/secondfactor/totp","remote_ip":"100.0.0.1","time":"2026-05-15T03:46:30Z","username":"alice"}
```

`totp_failure.json`:
```
{"level":"warning","msg":"Unsuccessful TOTP authentication attempt by user 'alice'","path":"/api/secondfactor/totp","remote_ip":"100.0.0.1","time":"2026-05-15T03:46:33Z"}
```

`health_probe.json`:
```
{"level":"info","msg":"GET /api/health","method":"GET","path":"/api/health","remote_ip":"127.0.0.1","time":"2026-05-15T03:46:00Z"}
```

`text_mode_legacy.txt`:
```
time="2024-01-15T10:00:00Z" level=info msg="Authentication attempt successful" user=alice
```

- [ ] **Step 2: Tests in `authelia_tests.rs`.**

```rust
use crate::enrich::{AuthOutcome, Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/authelia/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(message: &str) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::AutheliaParser.parse(&ParserInput {
        app_name: Some("authelia"),
        container_name: Some("authelia"),
        message,
        raw: message,
        source_kind: SourceKind::DockerStream,
        severity: "info",
    })
}

#[test]
fn fafa_success() {
    let out = parse(&input_from("1fa_success.json")).unwrap();
    assert_eq!(out.auth_outcome, Some(AuthOutcome::Success));
    assert_eq!(out.metadata["username"], serde_json::json!("alice"));
    assert_eq!(out.metadata["mfa_method"], serde_json::json!("1fa"));
    assert_eq!(out.metadata["src_ip"], serde_json::json!("100.0.0.1"));
}

#[test]
fn fafa_failure() {
    let out = parse(&input_from("1fa_failure.json")).unwrap();
    assert_eq!(out.auth_outcome, Some(AuthOutcome::Failure));
    assert_eq!(out.metadata["username"], serde_json::json!("bob"));
    assert_eq!(out.severity, Some("err"));
}

#[test]
fn totp_success() {
    let out = parse(&input_from("totp_success.json")).unwrap();
    assert_eq!(out.auth_outcome, Some(AuthOutcome::Success));
    assert_eq!(out.metadata["mfa_method"], serde_json::json!("totp"));
}

#[test]
fn totp_failure_warning_severity() {
    let out = parse(&input_from("totp_failure.json")).unwrap();
    assert_eq!(out.auth_outcome, Some(AuthOutcome::Failure));
    assert_eq!(out.severity, Some("warning"));
}

#[test]
fn health_probe_no_auth_outcome() {
    let out = parse(&input_from("health_probe.json")).unwrap();
    assert_eq!(out.auth_outcome, None);
    // metadata still populated with path/remote_ip/method
    assert_eq!(out.metadata["path"], serde_json::json!("/api/health"));
}

#[test]
fn text_mode_legacy_returns_structural_error() {
    let out = parse(&input_from("text_mode_legacy.txt"));
    assert!(matches!(out, Err(crate::enrich::ParserError::Structural(_))));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::authelia
```

Expected: FAIL.

- [ ] **Step 4: Implement `src/enrich/parsers/authelia.rs`.**

```rust
//! Authelia auth parser. Spec §7.3.
//! Authelia emits JSON in modern deployments.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::enrich::{AuthOutcome, Parser, ParserError, ParserInput, ParserOutput};

pub struct AutheliaParser;

static USERNAME_QUOTED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"user '([^']+)'").expect("static regex"));

impl Parser for AutheliaParser {
    fn name(&self) -> &'static str { "authelia" }
    fn namespace(&self) -> &'static str { "authelia" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let trimmed = input.message.trim_start();
        if !trimmed.starts_with('{') {
            return Err(ParserError::Structural("not json (text-mode authelia)"));
        }
        let value: Value = serde_json::from_str(trimmed)?;
        let obj = value.as_object().ok_or(ParserError::Structural("json not object"))?;

        let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let msg = obj.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        let level = obj.get("level").and_then(|v| v.as_str()).unwrap_or("info");
        let remote_ip = obj.get("remote_ip").and_then(|v| v.as_str());
        let method = obj.get("method").and_then(|v| v.as_str());
        let username = obj
            .get("username")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| USERNAME_QUOTED_RE.captures(msg).map(|c| c[1].to_string()));

        // Severity mapping (Authelia → syslog).
        let severity = match level {
            "debug" => Some("debug"),
            "info" => Some("info"),
            "warning" | "warn" => Some("warning"),
            "error" => Some("err"),
            "critical" | "fatal" => Some("crit"),
            _ => None,
        };

        // Auth outcome — only assigned when this is actually an auth event.
        let is_auth_event = path.starts_with("/api/firstfactor")
            || path.starts_with("/api/secondfactor")
            || path.starts_with("/api/u2f")
            || path.starts_with("/api/duo");
        let auth_outcome = if is_auth_event {
            if msg.contains("Unsuccessful") {
                Some(AuthOutcome::Failure)
            } else if msg.contains("successful") || msg.contains("Successful") {
                Some(AuthOutcome::Success)
            } else if msg.contains("denied") || msg.contains("Denied") {
                Some(AuthOutcome::Denied)
            } else {
                None
            }
        } else {
            None
        };

        // mfa_method from path.
        let mfa_method = if path.starts_with("/api/firstfactor") {
            Some("1fa")
        } else if path.contains("/secondfactor/totp") {
            Some("totp")
        } else if path.contains("/secondfactor/duo") || path.starts_with("/api/duo") {
            Some("duo")
        } else if path.contains("/secondfactor/webauthn") || path.starts_with("/api/u2f") {
            Some("webauthn")
        } else {
            None
        };

        let mut metadata = Map::new();
        if let Some(u) = username {
            metadata.insert("username".into(), Value::String(u));
        }
        if let Some(m) = mfa_method {
            metadata.insert("mfa_method".into(), json!(m));
        }
        if let Some(ip) = remote_ip {
            metadata.insert("src_ip".into(), json!(ip));
        }
        if let Some(m) = method {
            metadata.insert("method".into(), json!(m));
        }
        if !path.is_empty() {
            metadata.insert("path".into(), json!(path));
        }

        Ok(ParserOutput {
            auth_outcome,
            severity,
            metadata,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "authelia_tests.rs"]
mod authelia_tests;
```

- [ ] **Step 5: Register in `mod.rs`.**

```rust
pub mod authelia;
pub use authelia::AutheliaParser;
```

- [ ] **Step 6: Run tests.**

```bash
cargo test --lib enrich::parsers::authelia
```

Expected: 6/6 PASS.

- [ ] **Step 7: Commit.**

```bash
git add src/enrich/parsers/authelia.rs src/enrich/parsers/authelia_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/authelia/
git commit -m "feat(enrich): authelia parser — JSON auth events"
```

---

### Task 13: `swag` / `nginx` parser

**Files:**
- Create: `src/enrich/parsers/swag.rs` + `swag_tests.rs`
- Create: `tests/fixtures/parsers/swag/{access_combined.txt,access_combined_upstream.txt,access_ipv6.txt,access_escaped_quote.txt,error_upstream_timeout.txt,error_no_upstream.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures.**

```bash
mkdir -p tests/fixtures/parsers/swag
```

`access_combined.txt`:
```
192.0.2.55 - alice [15/May/2026:14:22:11 +0000] "POST /login HTTP/1.1" 401 87 "-" "curl/8.0"
```

`access_combined_upstream.txt`:
```
192.0.2.55 - - [15/May/2026:14:22:11 +0000] "GET /api/movies HTTP/2.0" 200 1432 "https://example.com/" "Mozilla/5.0" "203.0.113.7" 0.041
```

`access_ipv6.txt`:
```
[2001:db8::1] - - [15/May/2026:14:22:11 +0000] "GET / HTTP/2.0" 200 100 "-" "ua"
```

`access_escaped_quote.txt`:
```
192.0.2.55 - - [15/May/2026:14:22:11 +0000] "GET /q?x=\x22hi\x22 HTTP/1.1" 200 50 "-" "ua"
```

`error_upstream_timeout.txt`:
```
2026/05/15 14:22:11 [error] 17#17: *4321 upstream timed out (110: Connection timed out) while reading response header from upstream, client: 192.0.2.55, server: example.com, request: "GET / HTTP/2.0", upstream: "http://10.0.0.5:3000/"
```

`error_no_upstream.txt`:
```
2026/05/15 14:22:11 [error] 17#17: *4321 open() "/config/nginx/html/foo" failed (2: No such file or directory)
```

- [ ] **Step 2: Tests in `swag_tests.rs`.**

```rust
use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/swag/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(message: &str) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::SwagParser.parse(&ParserInput {
        app_name: Some("swag"),
        container_name: Some("swag"),
        message,
        raw: message,
        source_kind: SourceKind::DockerStream,
        severity: "info",
    })
}

#[test]
fn access_combined_400_class() {
    let out = parse(&input_from("access_combined.txt")).unwrap();
    assert_eq!(out.http_status, Some(401));
    assert_eq!(out.event_action.as_deref(), Some("http_request"));
    assert_eq!(out.metadata["method"], serde_json::json!("POST"));
    assert_eq!(out.metadata["path"], serde_json::json!("/login"));
    assert_eq!(out.metadata["client_ip"], serde_json::json!("192.0.2.55"));
    assert_eq!(out.metadata["bytes_sent"], serde_json::json!(87));
}

#[test]
fn access_combined_upstream_extracts_latency_and_forwarded_for() {
    let out = parse(&input_from("access_combined_upstream.txt")).unwrap();
    assert_eq!(out.http_status, Some(200));
    assert_eq!(out.metadata["forwarded_for"], serde_json::json!("203.0.113.7"));
    assert_eq!(out.metadata["latency_ms"], serde_json::json!(41));
}

#[test]
fn access_ipv6_client() {
    let out = parse(&input_from("access_ipv6.txt")).unwrap();
    assert_eq!(out.http_status, Some(200));
    assert_eq!(out.metadata["client_ip"], serde_json::json!("2001:db8::1"));
}

#[test]
fn access_escaped_quote_in_path() {
    let out = parse(&input_from("access_escaped_quote.txt")).unwrap();
    assert_eq!(out.http_status, Some(200));
    // Path retains escaped sequences as-is; we don't unescape.
    assert!(out.metadata["path"].as_str().unwrap().contains("x=\\x22hi\\x22"));
}

#[test]
fn error_upstream_timeout() {
    let out = parse(&input_from("error_upstream_timeout.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("upstream_error"));
    assert_eq!(out.metadata["upstream"], serde_json::json!("http://10.0.0.5:3000/"));
    assert_eq!(out.metadata["error_class"], serde_json::json!("timeout"));
    assert_eq!(out.severity, Some("err"));
}

#[test]
fn error_no_upstream_returns_no_match() {
    // Error lines that don't mention upstream are skipped (they're nginx-internal).
    let err = parse(&input_from("error_no_upstream.txt")).unwrap_err();
    assert!(matches!(err, crate::enrich::ParserError::NoMatch(_)));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::swag
```

Expected: FAIL.

- [ ] **Step 4: Implement `src/enrich/parsers/swag.rs`.**

```rust
//! SWAG / nginx access + error log parser. Spec §7.4.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct SwagParser;

const PATH_MAX: usize = 2048;
const UA_MAX: usize = 512;

/// Combined access log with optional upstream extras.
/// Captures: client, user, time, method, path, http_ver, status, bytes,
///   referrer, user_agent, optional forwarded_for, optional request_time.
static ACCESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?P<client>\S+|\[[0-9a-fA-F:]+\]) - (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<path>[^"]*) HTTP/[\d.]+" (?P<status>\d{3}) (?P<bytes>\d+) "(?P<ref>[^"]*)" "(?P<ua>[^"]*)"(?:\s+"(?P<xff>[^"]*)"\s+(?P<rt>[\d.]+))?"#,
    )
    .expect("static regex")
});

/// Error log mentioning upstream.
/// Captures error_class via known substrings (timeout, connrefused, etc.) and upstream URL.
static ERROR_UPSTREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"upstream(?P<rest>.*?)upstream:\s+"(?P<upstream>[^"]+)""#,
    )
    .expect("static regex")
});

impl Parser for SwagParser {
    fn name(&self) -> &'static str { "swag" }
    fn namespace(&self) -> &'static str { "swag" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let msg = input.message;

        // Try access log first (most common).
        if let Some(caps) = ACCESS_RE.captures(msg) {
            return parse_access(caps);
        }

        // Try upstream error.
        if msg.contains(" [error] ") && msg.contains("upstream") {
            return parse_upstream_error(msg);
        }

        Err(ParserError::NoMatch("not an access or upstream error line"))
    }
}

fn parse_access(caps: regex::Captures) -> Result<ParserOutput, ParserError> {
    let status: i32 = caps["status"]
        .parse()
        .map_err(|_| ParserError::MissingField("http_status"))?;
    let bytes: i64 = caps["bytes"].parse().unwrap_or(0);

    // Strip [..] from IPv6 client.
    let client_raw = &caps["client"];
    let client = client_raw.trim_start_matches('[').trim_end_matches(']');

    let mut path = caps["path"].to_string();
    if path.len() > PATH_MAX {
        path.truncate(PATH_MAX);
    }

    let mut ua = caps["ua"].to_string();
    if ua.len() > UA_MAX {
        ua.truncate(UA_MAX);
    }

    let mut metadata = Map::new();
    metadata.insert("method".into(), json!(&caps["method"]));
    metadata.insert("path".into(), json!(path));
    metadata.insert("client_ip".into(), json!(client));
    metadata.insert("bytes_sent".into(), json!(bytes));
    metadata.insert("referrer".into(), json!(&caps["ref"]));
    metadata.insert("user_agent".into(), json!(ua));

    if let Some(xff) = caps.name("xff") {
        metadata.insert("forwarded_for".into(), json!(xff.as_str()));
    }
    if let Some(rt) = caps.name("rt") {
        if let Ok(secs) = rt.as_str().parse::<f64>() {
            metadata.insert("latency_ms".into(), json!((secs * 1000.0) as i32));
        }
    }

    Ok(ParserOutput {
        http_status: Some(status),
        event_action: Some("http_request".into()),
        metadata,
        ..Default::default()
    })
}

fn parse_upstream_error(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = ERROR_UPSTREAM_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("upstream error format unrecognised"))?;
    let upstream = caps["upstream"].to_string();

    let error_class = if msg.contains("timed out") {
        "timeout"
    } else if msg.contains("Connection refused") || msg.contains("connrefused") {
        "connrefused"
    } else if msg.contains("Connection reset") {
        "reset"
    } else {
        "other"
    };

    let mut metadata = Map::new();
    metadata.insert("upstream".into(), Value::String(upstream));
    metadata.insert("error_class".into(), json!(error_class));

    Ok(ParserOutput {
        event_action: Some("upstream_error".into()),
        severity: Some("err"),
        metadata,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "swag_tests.rs"]
mod swag_tests;
```

- [ ] **Step 5: Register.**

```rust
pub mod swag;
pub use swag::SwagParser;
```

- [ ] **Step 6: Run tests.**

```bash
cargo test --lib enrich::parsers::swag
```

Expected: 6/6 PASS.

- [ ] **Step 7: Commit.**

```bash
git add src/enrich/parsers/swag.rs src/enrich/parsers/swag_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/swag/
git commit -m "feat(enrich): swag/nginx parser — access + upstream errors"
```

---

### Task 14: `adguard` parser

**Files:**
- Create: `src/enrich/parsers/adguard.rs` + `adguard_tests.rs`
- Create: `tests/fixtures/parsers/adguard/{block.json,allow.json,rewrite.json,dnssec_failure.json,cached_hit.json,legacy_camelcase.json,api_poller_normalised.json,truncated_invalid.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures.**

```bash
mkdir -p tests/fixtures/parsers/adguard
```

`block.json`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"doubleclick.net","QT":"A","QC":"IN","Client":"192.168.10.55","Upstream":"https://dns.cloudflare.com/dns-query","Elapsed":"0.000234s","Result":{"IsFiltered":true,"Reason":"FilteredBlackList","Rule":"||doubleclick.net^","FilterID":1}}
```

`allow.json`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"github.com","QT":"A","Client":"192.168.10.55","Upstream":"https://dns.cloudflare.com/dns-query","Elapsed":"0.000125s","Result":{"IsFiltered":false,"Reason":"NotFilteredNotFound"}}
```

`rewrite.json`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"plex.local","QT":"A","Client":"192.168.10.55","Result":{"IsFiltered":false,"Reason":"Rewrite","Rules":[{"FilterListID":-2}]}}
```

`dnssec_failure.json`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"badsig.example","QT":"A","Client":"192.168.10.55","Result":{"IsFiltered":false,"Reason":"NotFilteredError","DNSSECResult":3}}
```

`cached_hit.json`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"github.com","QT":"A","Client":"192.168.10.55","Cached":true,"Result":{"IsFiltered":false,"Reason":"NotFilteredNotFound"}}
```

`legacy_camelcase.json`:
```
{"time":"2025-01-01T00:00:00Z","question":{"host":"example.com","type":"A"},"client":"192.168.10.55","result":{"filtered":false,"reason":"NotFilteredNotFound"}}
```

`api_poller_normalised.json`: byte-identical to `block.json`.

`truncated_invalid.txt`:
```
{"T":"2026-05-15T14:22:11.123Z","QH":"trunc
```

- [ ] **Step 2: Tests.**

```rust
use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/adguard/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(message: &str, source_kind: SourceKind) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::AdguardParser.parse(&ParserInput {
        app_name: Some("adguard-query"),
        container_name: None,
        message,
        raw: message,
        source_kind,
        severity: "info",
    })
}

#[test]
fn block_marks_dns_blocked_true() {
    let out = parse(&input_from("block.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, Some(true));
    assert_eq!(out.event_action.as_deref(), Some("dns_query"));
    assert_eq!(out.metadata["query"], serde_json::json!("doubleclick.net"));
    assert_eq!(out.metadata["qtype"], serde_json::json!("A"));
    assert_eq!(out.metadata["client"], serde_json::json!("192.168.10.55"));
    assert_eq!(out.metadata["reason"], serde_json::json!("FilteredBlackList"));
    assert_eq!(out.metadata["rule"], serde_json::json!("||doubleclick.net^"));
}

#[test]
fn allow_marks_dns_blocked_false() {
    let out = parse(&input_from("allow.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, Some(false));
}

#[test]
fn rewrite_marks_dns_blocked_null() {
    // Rewrite is neither blocked nor a plain allow — see spec §13 OQ#3.
    let out = parse(&input_from("rewrite.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, None);
    assert_eq!(out.metadata["reason"], serde_json::json!("Rewrite"));
}

#[test]
fn cached_hit() {
    let out = parse(&input_from("cached_hit.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.metadata["cached"], serde_json::json!(true));
}

#[test]
fn legacy_camelcase_falls_back() {
    let out = parse(&input_from("legacy_camelcase.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.metadata["query"], serde_json::json!("example.com"));
    assert_eq!(out.dns_blocked, Some(false));
}

#[test]
fn api_poller_path_yields_identical_output() {
    let from_docker = parse(&input_from("block.json"), SourceKind::DockerStream).unwrap();
    let from_api = parse(&input_from("api_poller_normalised.json"), SourceKind::AdguardApi).unwrap();
    assert_eq!(from_docker.dns_blocked, from_api.dns_blocked);
    assert_eq!(from_docker.metadata, from_api.metadata);
}

#[test]
fn truncated_invalid_returns_json_error() {
    let err = parse(&input_from("truncated_invalid.txt"), SourceKind::DockerStream).unwrap_err();
    assert!(matches!(err, crate::enrich::ParserError::Json(_)));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::adguard
```

Expected: FAIL.

- [ ] **Step 4: Implement `src/enrich/parsers/adguard.rs`.**

```rust
//! AdGuard Home query log parser. Spec §7.5 + §8 (dual path).
//! Same code handles container-log and API-poller rows.

use serde_json::{json, Map, Value};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct AdguardParser;

impl Parser for AdguardParser {
    fn name(&self) -> &'static str { "adguard" }
    fn namespace(&self) -> &'static str { "adguard" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let value: Value = serde_json::from_str(input.message.trim())?;
        let obj = value.as_object().ok_or(ParserError::Structural("not a json object"))?;

        // PascalCase keys first (modern). Fall back to lowercase legacy.
        let query = pick_str(obj, &["QH"]).or_else(|| {
            obj.get("question")
                .and_then(|q| q.as_object())
                .and_then(|q| q.get("host"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        let qtype = pick_str(obj, &["QT"]).or_else(|| {
            obj.get("question")
                .and_then(|q| q.as_object())
                .and_then(|q| q.get("type"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        let client = pick_str(obj, &["Client", "client"]);
        let upstream = pick_str(obj, &["Upstream"]);
        let elapsed = pick_str(obj, &["Elapsed"]);
        let cached = obj.get("Cached").and_then(|v| v.as_bool());

        let result = obj
            .get("Result")
            .or_else(|| obj.get("result"))
            .and_then(|v| v.as_object())
            .ok_or(ParserError::Structural("missing Result"))?;
        let reason = pick_str(result, &["Reason", "reason"]);
        let rule = pick_str(result, &["Rule"]);
        let is_filtered = result
            .get("IsFiltered")
            .or_else(|| result.get("filtered"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let dns_blocked = match reason.as_deref() {
            Some(r) if r.starts_with("Rewrite") => None,
            _ if is_filtered => Some(true),
            _ => Some(false),
        };

        let mut metadata = Map::new();
        if let Some(q) = query {
            metadata.insert("query".into(), Value::String(q));
        }
        if let Some(t) = qtype {
            metadata.insert("qtype".into(), Value::String(t));
        }
        if let Some(c) = client {
            metadata.insert("client".into(), Value::String(c));
        }
        if let Some(u) = upstream {
            metadata.insert("upstream".into(), Value::String(u));
        }
        if let Some(r) = reason {
            metadata.insert("reason".into(), Value::String(r));
        }
        if let Some(r) = rule {
            metadata.insert("rule".into(), Value::String(r));
        }
        if let Some(e) = elapsed {
            // "0.000234s" → 0.234 ms
            if let Some(stripped) = e.strip_suffix('s') {
                if let Ok(secs) = stripped.parse::<f64>() {
                    metadata.insert("elapsed_ms".into(), json!(secs * 1000.0));
                }
            }
        }
        if let Some(c) = cached {
            metadata.insert("cached".into(), json!(c));
        }

        Ok(ParserOutput {
            dns_blocked,
            event_action: Some("dns_query".into()),
            metadata,
            ..Default::default()
        })
    }
}

fn pick_str(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
#[path = "adguard_tests.rs"]
mod adguard_tests;
```

- [ ] **Step 5: Register + test.**

```rust
pub mod adguard;
pub use adguard::AdguardParser;
```

```bash
cargo test --lib enrich::parsers::adguard
```

Expected: 7/7 PASS.

- [ ] **Step 6: Commit.**

```bash
git add src/enrich/parsers/adguard.rs src/enrich/parsers/adguard_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/adguard/
git commit -m "feat(enrich): adguard parser — DNS query log (container + API)"
```

---

### Task 15: `fail2ban` parser

**Files:**
- Create: `src/enrich/parsers/fail2ban.rs` + `fail2ban_tests.rs`
- Create: `tests/fixtures/parsers/fail2ban/{ban.txt,unban.txt,found.txt,restore_ban.txt,multi_ip_ban.txt,error_line.txt}`
- Modify: `src/enrich/parsers/mod.rs`

- [ ] **Step 1: Create fixtures.**

```bash
mkdir -p tests/fixtures/parsers/fail2ban
```

`ban.txt`:
```
2026-05-15 14:22:11,037 fail2ban.actions [992]: NOTICE [sshd] Ban 203.0.113.7
```

`unban.txt`:
```
2026-05-15 14:22:26,259 fail2ban.actions [992]: NOTICE [sshd] Unban 203.0.113.7
```

`found.txt`:
```
2026-05-15 14:31:14,420 fail2ban.filter [9599]: INFO [sshd] Found 203.0.113.7 - 2026-05-15 14:31:14
```

`restore_ban.txt`:
```
2026-05-15 14:35:01,001 fail2ban.actions [992]: NOTICE [authelia] Restore Ban 198.51.100.4
```

`multi_ip_ban.txt`:
```
2026-05-15 14:36:00,500 fail2ban.actions [992]: NOTICE [sshd] Ban 1.2.3.4 5.6.7.8
```

`error_line.txt`:
```
2026-05-15 14:39:00,000 fail2ban.server [992]: ERROR Failed to fetch jail config
```

- [ ] **Step 2: Tests.**

```rust
use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/fail2ban/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(message: &str) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::Fail2banParser.parse(&ParserInput {
        app_name: Some("fail2ban"),
        container_name: None,
        message,
        raw: message,
        source_kind: SourceKind::SyslogTcp,
        severity: "notice",
    })
}

#[test]
fn ban() {
    let out = parse(&input_from("ban.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("ban"));
    assert_eq!(out.metadata["jail"], serde_json::json!("sshd"));
    assert_eq!(out.metadata["banned_ip"], serde_json::json!("203.0.113.7"));
    assert_eq!(out.severity, Some("warning"));
}

#[test]
fn unban() {
    let out = parse(&input_from("unban.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("unban"));
    assert_eq!(out.metadata["jail"], serde_json::json!("sshd"));
}

#[test]
fn found() {
    let out = parse(&input_from("found.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("found"));
    assert_eq!(out.metadata["banned_ip"], serde_json::json!("203.0.113.7"));
}

#[test]
fn restore_ban_different_jail() {
    let out = parse(&input_from("restore_ban.txt")).unwrap();
    assert_eq!(out.event_action.as_deref(), Some("restore_ban"));
    assert_eq!(out.metadata["jail"], serde_json::json!("authelia"));
    assert_eq!(out.metadata["banned_ip"], serde_json::json!("198.51.100.4"));
}

#[test]
fn multi_ip_ban_first_in_banned_ip() {
    let out = parse(&input_from("multi_ip_ban.txt")).unwrap();
    assert_eq!(out.metadata["banned_ip"], serde_json::json!("1.2.3.4"));
    let all = out.metadata["all_ips"].as_array().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn error_line_no_match() {
    let err = parse(&input_from("error_line.txt")).unwrap_err();
    assert!(matches!(err, crate::enrich::ParserError::NoMatch(_)));
}
```

- [ ] **Step 3: Run to confirm failure.**

```bash
cargo test --lib enrich::parsers::fail2ban
```

Expected: FAIL.

- [ ] **Step 4: Implement `src/enrich/parsers/fail2ban.rs`.**

```rust
//! fail2ban parser. Spec §7.6.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct Fail2banParser;

static LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"fail2ban\.\w+\s+\[\d+\]:\s+\w+\s+\[(?P<jail>[^\]]+)\]\s+(?P<verb>Ban|Unban|Found|Restore Ban)\s+(?P<ips>[\d\.:a-fA-F\s]+?)(?:\s+-\s+\d|$)",
    )
    .expect("static regex")
});

impl Parser for Fail2banParser {
    fn name(&self) -> &'static str { "fail2ban" }
    fn namespace(&self) -> &'static str { "fail2ban" }

    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let caps = LINE_RE
            .captures(input.message)
            .ok_or(ParserError::NoMatch("not a fail2ban action line"))?;
        let jail = caps["jail"].to_string();
        let verb = &caps["verb"];
        let ips_raw = caps["ips"].trim();
        let ips: Vec<String> = ips_raw
            .split_whitespace()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if ips.is_empty() {
            return Err(ParserError::MissingField("banned_ip"));
        }

        let event_action = match verb {
            "Ban" => "ban",
            "Unban" => "unban",
            "Found" => "found",
            "Restore Ban" => "restore_ban",
            _ => unreachable!(),
        };

        let severity = match event_action {
            "ban" | "restore_ban" => Some("warning"),
            "unban" => Some("info"),
            "found" => Some("notice"),
            _ => None,
        };

        let mut metadata = Map::new();
        metadata.insert("jail".into(), Value::String(jail));
        metadata.insert("banned_ip".into(), Value::String(ips[0].clone()));
        if ips.len() > 1 {
            metadata.insert(
                "all_ips".into(),
                Value::Array(ips.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }

        Ok(ParserOutput {
            event_action: Some(event_action.into()),
            severity,
            metadata,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "fail2ban_tests.rs"]
mod fail2ban_tests;
```

- [ ] **Step 5: Register + test.**

```rust
pub mod fail2ban;
pub use fail2ban::Fail2banParser;
```

```bash
cargo test --lib enrich::parsers::fail2ban
```

Expected: 6/6 PASS.

- [ ] **Step 6: Commit.**

```bash
git add src/enrich/parsers/fail2ban.rs src/enrich/parsers/fail2ban_tests.rs src/enrich/parsers/mod.rs tests/fixtures/parsers/fail2ban/
git commit -m "feat(enrich): fail2ban parser — bans/unbans/founds"
```

---

## Phase 5 — Dispatcher registration and routing

### Task 16: `container_to_canonical` map + parser registration

**Files:**
- Modify: `src/enrich/dispatch.rs`
- Modify: `src/enrich/dispatch_tests.rs`

- [ ] **Step 1: Write failing tests for dispatch behaviour.**

Append to `src/enrich/dispatch_tests.rs`:

```rust
use crate::enrich::SourceKind;

fn entry_for_dispatch(
    app_name: Option<&str>,
    container_name: Option<&str>,
    message: &str,
    source_kind: SourceKind,
) -> LogBatchEntry {
    let mut e = fixture_entry();
    e.app_name = app_name.map(str::to_string);
    e.message = message.to_string();
    e.raw = message.to_string();
    if let Some(c) = container_name {
        let json = format!(
            r#"{{"source_kind":"{}","docker":{{"container_name":"{}"}}}}"#,
            source_kind.as_str(), c
        );
        e.metadata_json = Some(json);
    } else {
        e.metadata_json = Some(format!(r#"{{"source_kind":"{}"}}"#, source_kind.as_str()));
    }
    e
}

#[test]
fn dispatch_routes_swag_container_to_swag_parser() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("nginx"),
        Some("swag"),
        r#"192.0.2.55 - - [15/May/2026:14:22:11 +0000] "GET / HTTP/1.1" 200 100 "-" "ua""#,
        SourceKind::DockerStream,
    );
    pipeline.dispatch(&mut entry);
    assert_eq!(entry.http_status, Some(200));
    assert_eq!(entry.event_action.as_deref(), Some("http_request"));
}

#[test]
fn dispatch_routes_docker_event_by_source_kind() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("dockerd"),
        None,
        "docker container event: die container=postgres image=postgres:16 compose_project=stack compose_service=db exit_code=137",
        SourceKind::DockerEvent,
    );
    pipeline.dispatch(&mut entry);
    assert_eq!(entry.event_action.as_deref(), Some("die"));
}

#[test]
fn dispatch_routes_authelia_main_to_authelia_parser() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("authelia"),
        Some("authelia-main"),  // operator-renamed
        r#"{"level":"info","msg":"Authentication attempt successful","path":"/api/firstfactor","remote_ip":"100.0.0.1","time":"2026-05-15T03:46:03Z","username":"alice"}"#,
        SourceKind::DockerStream,
    );
    pipeline.dispatch(&mut entry);
    assert_eq!(entry.auth_outcome, Some("success"));
}

#[test]
fn dispatch_routes_adguard_api_to_adguard_parser() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("adguard-query"),
        None,
        r#"{"T":"2026-05-15T14:22:11.123Z","QH":"doubleclick.net","QT":"A","Client":"192.168.10.55","Result":{"IsFiltered":true,"Reason":"FilteredBlackList"}}"#,
        SourceKind::AdguardApi,
    );
    pipeline.dispatch(&mut entry);
    assert_eq!(entry.dns_blocked, Some(true));
}

#[test]
fn dispatch_unknown_source_no_op() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(Some("randomapp"), None, "hello world", SourceKind::SyslogTcp);
    pipeline.dispatch(&mut entry);
    assert!(entry.event_action.is_none());
    assert!(entry.parse_error.is_none());
}

#[test]
fn dispatch_records_parse_error_on_parser_failure() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("adguard-query"),
        None,
        "{ bad json",
        SourceKind::AdguardApi,
    );
    pipeline.dispatch(&mut entry);
    assert!(entry.parse_error.is_some());
    assert!(entry.parse_error.as_ref().unwrap().starts_with("adguard:"));
}

#[test]
fn dispatch_kernel_facility_via_app_name() {
    let pipeline = EnrichmentPipeline::new();
    let mut entry = entry_for_dispatch(
        Some("kernel"),
        None,
        "Out of memory: Killed process 100 (foo) total-vm:1024kB, anon-rss:512kB, file-rss:0kB, shmem-rss:0kB, UID:0 pgtables:8kB oom_score_adj:0",
        SourceKind::SyslogTcp,
    );
    pipeline.dispatch(&mut entry);
    assert_eq!(entry.event_action.as_deref(), Some("oom_kill"));
}
```

- [ ] **Step 2: Run to confirm failure.**

```bash
cargo test --lib enrich::dispatch_tests
```

Expected: 7 of the new tests FAIL (the existing empty-pipeline test still passes).

- [ ] **Step 3: Replace dispatcher impl with the populated version.**

Replace `src/enrich/dispatch.rs` (preserving the test re-include line at the bottom):

```rust
//! Dispatcher — picks a parser per (source_kind, app_name, container_name)
//! and merges its output onto the entry.
//!
//! Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md §4

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

use crate::db::LogBatchEntry;
use crate::enrich::output::{merge_output, record_error};
use crate::enrich::parsers::{
    AdguardParser, AutheliaParser, DockerEventParser, Fail2banParser, KernelParser, SwagParser,
};
use crate::enrich::Parser;

const LRU_CAP: usize = 256;

/// Operator-friendly container names that fold onto the canonical parser key.
fn container_to_canonical(container: &str) -> &'static str {
    match container {
        "authelia" | "authelia-main" | "authelia-prod" | "authelia-master" => "authelia",
        "swag" | "swag-main" | "nginx" | "nginx-proxy" => "swag",
        "adguardhome" | "adguard" | "adguardhome-main" => "adguard",
        "fail2ban" | "fail2ban-main" => "fail2ban",
        _ => "",
    }
}

pub struct EnrichmentPipeline {
    by_name: HashMap<&'static str, &'static dyn Parser>,
    docker_event: &'static DockerEventParser,
}

static KERNEL: KernelParser = KernelParser;
static DOCKER_EVENT: DockerEventParser = DockerEventParser;
static AUTHELIA: AutheliaParser = AutheliaParser;
static SWAG: SwagParser = SwagParser;
static ADGUARD: AdguardParser = AdguardParser;
static FAIL2BAN: Fail2banParser = Fail2banParser;

static UNKNOWN_APPS: LazyLock<Mutex<lru::LruCache<String, ()>>> = LazyLock::new(|| {
    Mutex::new(lru::LruCache::new(std::num::NonZeroUsize::new(LRU_CAP).unwrap()))
});

impl EnrichmentPipeline {
    pub fn new() -> Self {
        let mut by_name: HashMap<&'static str, &'static dyn Parser> = HashMap::new();
        by_name.insert("kernel", &KERNEL);
        by_name.insert("authelia", &AUTHELIA);
        by_name.insert("swag", &SWAG);
        by_name.insert("adguard", &ADGUARD);
        by_name.insert("adguard-query", &ADGUARD);  // poller-tagged app_name
        by_name.insert("fail2ban", &FAIL2BAN);

        Self { by_name, docker_event: &DOCKER_EVENT }
    }

    pub fn dispatch(&self, entry: &mut LogBatchEntry) {
        let source_kind = read_source_kind(entry);

        // docker-event short-circuit (spec §4).
        if source_kind.as_deref() == Some("docker-event") {
            self.apply(entry, self.docker_event);
            return;
        }

        // Read container_name from metadata_json.docker.container_name.
        let container = read_container_name(entry);

        // Container-first lookup.
        if let Some(c) = container.as_deref() {
            let canon = container_to_canonical(c);
            if !canon.is_empty() {
                if let Some(parser) = self.by_name.get(canon) {
                    self.apply(entry, *parser);
                    return;
                }
            }
        }

        // app_name fallback (case-insensitive).
        let app_lower = entry.app_name.as_deref().map(|s| s.to_ascii_lowercase());
        if let Some(app) = app_lower.as_deref() {
            if let Some(parser) = self.by_name.get(app) {
                self.apply(entry, *parser);
                return;
            }
            // Unknown app — debug-log once per name.
            let mut lru = UNKNOWN_APPS.lock().expect("LRU poisoned");
            if lru.put(app.to_string(), ()).is_none() {
                tracing::debug!(target = "enrich", app_name = app, "no parser registered for app");
            }
        }
    }

    fn apply(&self, entry: &mut LogBatchEntry, parser: &'static dyn Parser) {
        use crate::enrich::{ParserInput, SourceKind};
        // Read source_kind back into the enum (best-effort).
        let kind_str = read_source_kind(entry).unwrap_or_else(|| "syslog-tcp".to_string());
        let source_kind = match kind_str.as_str() {
            "syslog-udp" => SourceKind::SyslogUdp,
            "syslog-tcp" => SourceKind::SyslogTcp,
            "docker-stream" => SourceKind::DockerStream,
            "docker-event" => SourceKind::DockerEvent,
            "otlp" => SourceKind::Otlp,
            "adguard-api" => SourceKind::AdguardApi,
            "unifi-api" => SourceKind::UnifiApi,
            "agent" => SourceKind::Agent,
            _ => SourceKind::SyslogTcp,
        };
        let container = read_container_name(entry);
        let input = ParserInput {
            app_name: entry.app_name.as_deref(),
            container_name: container.as_deref(),
            message: &entry.message,
            raw: &entry.raw,
            source_kind,
            severity: &entry.severity,
        };
        match parser.parse(&input) {
            Ok(out) => merge_output(entry, parser.namespace(), out),
            Err(e) => record_error(entry, parser.name(), &e.to_string()),
        }
    }
}

impl Default for EnrichmentPipeline {
    fn default() -> Self { Self::new() }
}

fn read_source_kind(entry: &LogBatchEntry) -> Option<String> {
    let raw = entry.metadata_json.as_deref()?;
    let v: Value = serde_json::from_str(raw).ok()?;
    v.get("source_kind")?.as_str().map(str::to_string)
}

fn read_container_name(entry: &LogBatchEntry) -> Option<String> {
    let raw = entry.metadata_json.as_deref()?;
    let v: Value = serde_json::from_str(raw).ok()?;
    v.get("docker")?.get("container_name")?.as_str().map(str::to_string)
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
```

- [ ] **Step 4: Add `lru` to `Cargo.toml` `[dependencies]` (alphabetical position).**

```toml
lru = "0.12"
```

- [ ] **Step 5: Build, run tests.**

```bash
cargo build
cargo test --lib enrich::dispatch_tests
```

Expected: 8/8 PASS (1 empty-pipeline + 7 new).

- [ ] **Step 6: Run the full enrich module tests.**

```bash
cargo test --lib enrich
```

Expected: all PASS.

- [ ] **Step 7: Commit.**

```bash
git add -u src/enrich/dispatch.rs src/enrich/dispatch_tests.rs Cargo.toml Cargo.lock
git commit -m "feat(enrich): wire parser dispatch with container_to_canonical + LRU debug"
```

---

## Phase 6 — End-to-end verification

### Task 17: Integration test through the real ingest pipeline

**Files:**
- Create: `tests/enrich_pipeline.rs`

- [ ] **Step 1: Read existing integration test for the pattern.**

```bash
ls tests/
```

If there are no existing `tests/*.rs`, this is the first integration test — that's fine. It'll live as a top-level Cargo integration target.

- [ ] **Step 2: Write the integration test.**

Create `tests/enrich_pipeline.rs`:

```rust
//! End-to-end: spin up an in-memory pool, push fixture rows through
//! enrich_entry + parser dispatch, insert via insert_logs_batch, assert.

use std::sync::Arc;

use syslog_mcp::db::LogBatchEntry;
use syslog_mcp::enrich::{stamp_source_kind, EnrichmentPipeline, SourceKind};

fn pool() -> syslog_mcp::db::pool::DbPool {
    let dir = tempfile::tempdir().unwrap();
    let config = syslog_mcp::config::StorageConfig {
        db_path: dir.into_path().join("test.db"),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    syslog_mcp::db::pool::init_pool(&config).unwrap()
}

fn make_entry(app: &str, message: &str, source_kind: SourceKind) -> LogBatchEntry {
    let mut e = LogBatchEntry {
        timestamp: "2026-05-16T10:00:00Z".into(),
        hostname: "h".into(),
        facility: None,
        severity: "info".into(),
        app_name: Some(app.into()),
        process_id: None,
        message: message.into(),
        raw: message.into(),
        source_ip: "udp://127.0.0.1:514".into(),
        docker_checkpoint: None,
        ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    };
    stamp_source_kind(&mut e, source_kind);
    e
}

#[test]
fn swag_row_lands_with_http_status() {
    let pool = pool();
    let pipeline = Arc::new(EnrichmentPipeline::new());

    let mut entry = make_entry(
        "swag",
        r#"192.0.2.55 - - [15/May/2026:14:22:11 +0000] "GET /api HTTP/2.0" 404 87 "-" "ua""#,
        SourceKind::DockerStream,
    );
    // Stamp the container too (dispatch needs it).
    entry.metadata_json = Some(r#"{"source_kind":"docker-stream","docker":{"container_name":"swag"}}"#.into());

    pipeline.dispatch(&mut entry);
    syslog_mcp::db::insert_logs_batch(&pool, &[entry]).unwrap();

    let conn = pool.get().unwrap();
    let (status, event_action): (Option<i32>, Option<String>) = conn
        .query_row(
            "SELECT http_status, event_action FROM logs LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, Some(404));
    assert_eq!(event_action.as_deref(), Some("http_request"));
}

#[test]
fn adguard_row_lands_with_dns_blocked() {
    let pool = pool();
    let pipeline = Arc::new(EnrichmentPipeline::new());

    let mut entry = make_entry(
        "adguard-query",
        r#"{"T":"2026-05-16T14:00:00Z","QH":"ads.example","QT":"A","Client":"192.168.0.10","Result":{"IsFiltered":true,"Reason":"FilteredBlackList"}}"#,
        SourceKind::AdguardApi,
    );

    pipeline.dispatch(&mut entry);
    syslog_mcp::db::insert_logs_batch(&pool, &[entry]).unwrap();

    let conn = pool.get().unwrap();
    let blocked: Option<i64> = conn
        .query_row("SELECT dns_blocked FROM logs LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(blocked, Some(1));
}

#[test]
fn unknown_source_writes_row_unchanged() {
    let pool = pool();
    let pipeline = Arc::new(EnrichmentPipeline::new());

    let mut entry = make_entry("unknown-app", "random log line", SourceKind::SyslogTcp);
    pipeline.dispatch(&mut entry);
    syslog_mcp::db::insert_logs_batch(&pool, &[entry]).unwrap();

    let conn = pool.get().unwrap();
    let (status, parse_error): (Option<i32>, Option<String>) = conn
        .query_row(
            "SELECT http_status, parse_error FROM logs LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, None);
    assert_eq!(parse_error, None);
}

#[test]
fn parser_failure_records_parse_error_but_persists_row() {
    let pool = pool();
    let pipeline = Arc::new(EnrichmentPipeline::new());

    let mut entry = make_entry("adguard-query", "{ bad json", SourceKind::AdguardApi);
    pipeline.dispatch(&mut entry);
    syslog_mcp::db::insert_logs_batch(&pool, &[entry]).unwrap();

    let conn = pool.get().unwrap();
    let (count, parse_error): (i64, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*), MAX(parse_error) FROM logs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(parse_error.unwrap().starts_with("adguard:"));
}
```

- [ ] **Step 3: Verify what's pub in lib.rs.**

```bash
grep -n "^pub" src/lib.rs | head -20
```

If `db`, `enrich`, or `config` aren't `pub`, the integration test won't compile. They should be (the existing CLI binary uses them). If anything's missing, add `pub` to the module declaration in `src/lib.rs`.

If `db::insert_logs_batch` isn't re-exported at `db::`, add to `src/db.rs` (or wherever the module root lives):

```rust
pub use crate::db::ingest::insert_logs_batch;
```

- [ ] **Step 4: Run the integration test.**

```bash
cargo test --test enrich_pipeline
```

Expected: 4/4 PASS.

- [ ] **Step 5: Commit.**

```bash
git add tests/enrich_pipeline.rs
[ -z "$(git diff --cached --stat src/db.rs src/lib.rs 2>/dev/null)" ] || git add -u src/db.rs src/lib.rs
git commit -m "test(enrich): end-to-end pipeline integration test"
```

---

### Task 18: Smoke test addition

**Files:**
- Modify: `scripts/smoke-test.sh`

- [ ] **Step 1: Locate the existing smoke test.**

```bash
cat scripts/smoke-test.sh | head -40
```

Note the section that checks ingest/MCP responses.

- [ ] **Step 2: Add a synthetic SWAG line forwarder + assertion.**

In `scripts/smoke-test.sh`, find a section that's already past server startup (e.g. after the `syslog status` check). Append a block:

```bash
# --- Enrichment framework smoke (epic syslog-mcp-1wjr) ---
# Forward a synthetic SWAG access line, then assert http_status materialised.

SWAG_LINE='<134>1 2026-05-16T10:00:00Z localhost swag - - - 192.0.2.55 - - [16/May/2026:10:00:00 +0000] "GET /smoke HTTP/1.1" 418 13 "-" "smoketest/1.0"'
echo "$SWAG_LINE" | nc -w1 -u 127.0.0.1 "${SYSLOG_PORT:-1514}" || true
sleep 1

# Query the DB directly (smoke-test runs alongside the server).
COUNT=$(sqlite3 "${SYSLOG_MCP_DB_PATH}" \
    "SELECT COUNT(*) FROM logs WHERE http_status = 418")
if [ "$COUNT" = "0" ]; then
    echo "FAIL: enrichment smoke — synthetic SWAG line did not yield http_status=418"
    exit 1
fi
echo "OK: enrichment framework wired (http_status=418 found)"
```

- [ ] **Step 3: Manually verify the script syntax.**

```bash
bash -n scripts/smoke-test.sh
```

Expected: no syntax errors.

- [ ] **Step 4: Commit.**

```bash
git add scripts/smoke-test.sh
git commit -m "test(smoke): assert enrichment framework emits http_status"
```

---

### Task 19: Full test suite sweep + clippy

**Files:** none (verification only)

- [ ] **Step 1: Run the full library test suite.**

```bash
cargo test --lib
```

Expected: all PASS (existing + new).

- [ ] **Step 2: Run integration tests.**

```bash
cargo test --tests
```

Expected: all PASS.

- [ ] **Step 3: Run clippy at the project's strict level.**

```bash
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -40
```

Expected: no warnings. Fix any that surface in the new modules:
- Common: `clippy::needless_pass_by_value`, `clippy::redundant_clone`, `clippy::unnecessary_wraps`.
- Acceptable: `#[allow(...)]` only with a comment explaining why.

- [ ] **Step 4: Format.**

```bash
cargo fmt
git diff --stat
```

If anything changed: commit it.

- [ ] **Step 5: Final commit if anything moved.**

```bash
git add -u
git commit -m "style: cargo fmt + clippy for enrich module" || true
```

---

## Post-implementation

### Final verification checklist

Before marking epic `syslog-mcp-1wjr` closed, verify:

- [ ] Every parser ships with a sidecar `*_tests.rs` and fixtures under `tests/fixtures/parsers/<name>/`.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- [ ] `scripts/smoke-test.sh` exits 0 against a fresh deployment.
- [ ] No regressions in existing MCP actions (run `cargo test --lib mcp` to confirm).
- [ ] The DB migration applies cleanly to the prod-shaped DB (test against a copy of `data/syslog.db` if available).

Then:

```bash
bd close syslog-mcp-1wjr --reason="Implemented per docs/superpowers/plans/2026-05-16-enrichment-framework-implementation.md. All 6 V1 parsers ship with fixtures + tests; migration 10 adds 5 columns + 4 partial indexes; dispatcher integrated on writer hot path; smoke test verifies end-to-end."
git push -u origin worktree-epic-b-enrichment-prereqs
```

Then open a PR.

---

## Notes for the executing agent

- **DRY:** every parser shares the same fixture-loading pattern. If it gets ugly, factor a `parsers_test_util.rs` module.
- **YAGNI:** do not pre-populate the `unifi` slot. Epic C ships that.
- **Frequent commits:** one commit per task. Don't bundle.
- **Spec deviation:** if you find the spec contradicts the contracts (in `docs/contracts/`), the **contract wins** (it's the audited canonical form). Note the deviation in the commit message.
- **Performance:** if a parser exceeds the 30 µs/row budget under criterion (the spec calls for one in §10 but this plan deferred the bench), file a follow-up bead — don't reshape the framework.
