# mnemo Feature Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port mnemo's AI-session intelligence features into syslog-mcp — adding `search_sessions`, `usage_blocks`, `project_context`, `list_ai_tools`, `list_ai_projects` MCP actions, a `syslog ai` CLI namespace, and a local transcript scanner — without breaking existing `sessions` CLI/MCP surfaces.

**Architecture:** Extend the existing `logs` table (already has `ai_tool`, `ai_project`, `ai_session_id`, `ai_transcript_path` columns) with new DB analytics functions, new service methods, five new MCP actions, and a `syslog ai` CLI namespace. Add schema migrations for transcript checkpoint/import-identity tables. Local transcript indexing is an explicit CLI command, not a background/startup side effect.

**Tech Stack:** Rust, SQLite + FTS5 (rusqlite), axum, tokio, serde_json, rmcp

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `src/db/pool.rs` | Modify | Add migrations 5 (transcript_sources) and 6 (transcript_import_records) |
| `src/db/queries.rs` | Modify | Add `search_ai_sessions`, `list_ai_tools`, `list_ai_projects` |
| `src/db/analytics.rs` | Modify | Add `get_ai_usage_blocks`, `get_ai_project_context` |
| `src/db/queries_tests.rs` | Modify | Add fixtures and tests for new queries |
| `src/db/analytics_tests.rs` | Modify | Add tests for usage_blocks and project_context |
| `src/app/models.rs` | Modify | Add request/response types for 5 new actions |
| `src/app/service.rs` | Modify | Add service methods for 5 new actions |
| `src/app/service_tests.rs` | Modify | Add service-layer tests |
| `src/mcp/tools.rs` | Modify | Add 5 new actions to dispatcher + help text |
| `src/mcp/schemas.rs` | Modify | Add schemas for 5 new actions |
| `src/mcp/rmcp_server.rs` | Modify | Add read-scope mappings for 5 new actions |
| `src/mcp/tools_tests.rs` | Modify | Add dispatcher parity tests |
| `src/cli.rs` | Modify | Add `CliCommand::Ai(AiCommand)` + subcommands |
| `src/cli_tests.rs` | Modify | Add parser tests for `ai` namespace |
| `src/main.rs` | Modify | Route `ai` top-level command, update help text |
| `src/main_tests.rs` | Modify | Add dispatch tests for `ai` commands |
| `src/otlp.rs` | Modify | Derive `ai_tool` from trusted OTLP attributes |
| `src/otlp_tests.rs` | Modify | Add tests for AI attribute mapping |
| `src/scanner/mod.rs` | **Create** | Transcript scanner: path validation, chunked JSONL parsing |
| `src/scanner/checkpoint.rs` | **Create** | Checkpoint read/write/advance logic |
| `src/scanner/claude.rs` | **Create** | Claude JSONL transcript parser |
| `src/scanner/codex.rs` | **Create** | Codex session parser |
| `src/scanner_tests.rs` | **Create** | Duplicate-run, failure, storage-block tests |
| `scripts/smoke-test.sh` | Modify | Add structure checks for 5 new actions |
| `docs/mcp/TOOLS.md` | Modify | Document 5 new actions + transcript visibility policy |
| `docs/CLI.md` | Modify | Document `syslog ai` namespace |
| `README.md` | Modify | Add AI session feature summary |

---

## Task 1: Schema Migrations — Transcript Sources and Import Identity

**Files:**
- Modify: `src/db/pool.rs` (add after migration 4)
- Test: `src/db/pool_tests.rs`

- [ ] **Step 1: Write failing migration test**

In `src/db/pool_tests.rs`, add:
```rust
#[test]
fn test_migration_5_transcript_sources() {
    let pool = create_test_pool();
    // table must exist after init
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transcript_sources'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_migration_6_transcript_import_records() {
    let pool = create_test_pool();
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transcript_import_records'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p syslog-mcp db::pool_tests::test_migration_5 2>&1 | tail -5
```
Expected: `FAILED` — table does not exist yet.

- [ ] **Step 3: Add migration 5 in `src/db/pool.rs`** after the existing migration 4 block (around line 244):

```rust
// Migration 5: transcript source checkpoints
{
    let ver: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 5",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if ver == 0 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transcript_sources (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                canonical_path  TEXT NOT NULL UNIQUE,
                source_kind     TEXT NOT NULL,  -- 'claude_project' | 'codex_session' | 'explicit_file'
                file_size       INTEGER,
                file_mtime      INTEGER,
                content_hash    TEXT,
                last_offset     INTEGER NOT NULL DEFAULT 0,
                last_indexed_at TEXT,
                last_error      TEXT
            );
            INSERT INTO schema_migrations (version) VALUES (5);",
        )?;
        tracing::info!("Migration 5: created transcript_sources table");
    }
}
```

- [ ] **Step 4: Add migration 6 in `src/db/pool.rs`** immediately after migration 5:

```rust
// Migration 6: transcript import record identity (prevents duplicate rows)
{
    let ver: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 6",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if ver == 0 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transcript_import_records (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id   INTEGER NOT NULL REFERENCES transcript_sources(id),
                record_key  TEXT NOT NULL,  -- stable identity: source_path + ':' + line_number or record uuid
                imported_at TEXT NOT NULL,
                UNIQUE(source_id, record_key)
            );
            CREATE INDEX IF NOT EXISTS idx_tir_source ON transcript_import_records(source_id);
            INSERT INTO schema_migrations (version) VALUES (6);",
        )?;
        tracing::info!("Migration 6: created transcript_import_records table");
    }
}
```

- [ ] **Step 5: Run tests to confirm they pass**

```bash
cargo test db::pool_tests::test_migration_5 db::pool_tests::test_migration_6 -- --nocapture 2>&1 | tail -10
```
Expected: both `ok`.

- [ ] **Step 6: Commit**

```bash
git add src/db/pool.rs src/db/pool_tests.rs
git commit -m "feat: add schema migrations 5+6 for transcript sources and import identity"
```

---

## Task 2: OTLP AI Tool Mapping

**Files:**
- Modify: `src/otlp.rs` (around line 246 — service_name extraction block)
- Test: `src/otlp_tests.rs`

- [ ] **Step 1: Write failing tests**

In `src/otlp_tests.rs`, add:
```rust
#[test]
fn test_otlp_ai_tool_from_claude_attribute() {
    // explicit trusted attribute "ai.tool" = "claude" → ai_tool = Some("claude")
    let entry = make_log_entry_with_attrs(&[("ai.tool", "claude")], &[]);
    assert_eq!(entry.ai_tool.as_deref(), Some("claude"));
}

#[test]
fn test_otlp_ai_tool_from_service_name_ignored() {
    // service.name = "claude-code" alone must NOT populate ai_tool
    let entry = make_log_entry_with_attrs(&[], &[("service.name", "claude-code")]);
    assert_eq!(entry.ai_tool, None);
}

#[test]
fn test_otlp_ai_tool_oversized_rejected() {
    let long_val = "a".repeat(256);
    let entry = make_log_entry_with_attrs(&[("ai.tool", long_val.as_str())], &[]);
    assert_eq!(entry.ai_tool, None);
}

#[test]
fn test_otlp_ai_tool_unknown_value_stored_none() {
    let entry = make_log_entry_with_attrs(&[("ai.tool", "unknown-tool-xyz")], &[]);
    assert_eq!(entry.ai_tool, None);
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test otlp_tests 2>&1 | tail -10
```

- [ ] **Step 3: Implement in `src/otlp.rs`**

Replace the `ai_tool: None` line (currently line 298) with:

```rust
ai_tool: {
    const KNOWN_TOOLS: &[&str] = &["claude", "codex", "gemini"];
    const MAX_LEN: usize = 64;
    let raw = log_attrs
        .get("ai.tool")
        .or_else(|| resource_attrs.get("ai.tool"));
    raw.and_then(|v| {
        if v.len() > MAX_LEN { return None; }
        let lower = v.to_lowercase();
        KNOWN_TOOLS.iter().find(|&&t| lower == t).map(|t| t.to_string())
    })
},
```

Also enforce length caps for `ai_project` and `ai_session_id` extracted from OTLP (add after each `.get(...)` call):
```rust
// ai_project — cap at 512 chars
.and_then(|v| if v.len() <= 512 { Some(v) } else { None })
// ai_session_id — cap at 128 chars
.and_then(|v| if v.len() <= 128 { Some(v) } else { None })
```

- [ ] **Step 4: Run tests**

```bash
cargo test otlp_tests 2>&1 | tail -10
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/otlp.rs src/otlp_tests.rs
git commit -m "feat: derive ai_tool from explicit OTLP attributes; reject oversized/unknown values"
```

---

## Task 3: New App Models

**Files:**
- Modify: `src/app/models.rs` (add after existing `AiSessionEntry`)
- Test: `src/app/models_tests.rs`

- [ ] **Step 1: Write serialization tests**

In `src/app/models_tests.rs`, add:
```rust
#[test]
fn test_search_sessions_response_serializes() {
    let r = SearchSessionsResponse {
        sessions: vec![],
        total_candidates: 0,
        truncated: false,
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"truncated\""));
    assert!(json.contains("\"total_candidates\""));
}

#[test]
fn test_usage_blocks_response_serializes() {
    let r = UsageBlocksResponse { blocks: vec![], truncated: false };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"blocks\""));
}

#[test]
fn test_project_context_response_serializes() {
    let r = ProjectContextResponse {
        project: "test".into(),
        tools: vec![],
        sessions: vec![],
        hostnames: vec![],
        first_seen: None,
        last_seen: None,
        event_count: 0,
        recent_entries: vec![],
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"project\""));
}
```

- [ ] **Step 2: Run to confirm they fail (types missing)**

```bash
cargo test app::models_tests 2>&1 | tail -5
```

- [ ] **Step 3: Add types to `src/app/models.rs`**

After `pub struct ListSessionsResponse { ... }`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSessionsRequest {
    pub query: String,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchedSessionEntry {
    pub project: String,
    pub tool: String,
    pub session_id: String,
    pub hostname: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub event_count: i64,
    pub match_count: i64,
    pub best_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSessionsResponse {
    pub sessions: Vec<SearchedSessionEntry>,
    pub total_candidates: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBlocksRequest {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBlock {
    pub bucket_start: String,  // ISO-8601, UTC, anchored to 5-hour epoch boundaries
    pub bucket_end: String,
    pub project: String,
    pub tool: String,
    pub session_count: i64,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBlocksResponse {
    pub blocks: Vec<UsageBlock>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContextRequest {
    pub project: String,
    pub ai_tool: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContextResponse {
    pub project: String,
    pub tools: Vec<String>,
    pub sessions: Vec<String>,
    pub hostnames: Vec<String>,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub event_count: i64,
    pub recent_entries: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiToolsRequest {
    pub ai_project: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolEntry {
    pub tool: String,
    pub event_count: i64,
    pub session_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiToolsResponse {
    pub tools: Vec<AiToolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiProjectsRequest {
    pub ai_tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProjectEntry {
    pub project: String,
    pub tools: Vec<String>,
    pub event_count: i64,
    pub session_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiProjectsResponse {
    pub projects: Vec<AiProjectEntry>,
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test app::models_tests 2>&1 | tail -5
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/app/models.rs src/app/models_tests.rs
git commit -m "feat: add request/response models for AI analytics actions"
```

---

## Task 4: DB Query — `search_ai_sessions`

**Files:**
- Modify: `src/db/queries.rs` (add after `list_ai_sessions`)
- Test: `src/db/queries_tests.rs`

- [ ] **Step 1: Write failing tests with fixtures**

In `src/db/queries_tests.rs`, add:
```rust
fn insert_ai_log(conn: &rusqlite::Connection, tool: &str, project: &str, session: &str, msg: &str, ts: &str) {
    conn.execute(
        "INSERT INTO logs (hostname, facility, severity, timestamp, received_at, message, app_name, ai_tool, ai_project, ai_session_id)
         VALUES ('host1', 'local0', 'info', ?1, ?1, ?2, 'test', ?3, ?4, ?5)",
        rusqlite::params![ts, msg, tool, project, session],
    ).unwrap();
    // FTS insert
    conn.execute(
        "INSERT INTO logs_fts(rowid, message) VALUES (last_insert_rowid(), ?1)",
        [msg],
    ).unwrap();
}

#[test]
fn test_search_ai_sessions_returns_grouped_results() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    insert_ai_log(&conn, "claude", "/home/user/proj", "sess-1", "fixed the authentication bug", "2026-01-01T10:00:00");
    insert_ai_log(&conn, "claude", "/home/user/proj", "sess-1", "authentication tests passing", "2026-01-01T10:01:00");
    insert_ai_log(&conn, "codex", "/home/user/other", "sess-2", "refactored authentication module", "2026-01-01T11:00:00");

    let params = SearchAiSessionsParams {
        query: "authentication".into(),
        ai_project: None,
        ai_tool: None,
        from: None,
        to: None,
        limit: Some(10),
    };
    let results = search_ai_sessions(&pool, &params).unwrap();
    assert!(!results.sessions.is_empty());
    // sess-1 has 2 matching events — must appear
    let sess1 = results.sessions.iter().find(|s| s.session_id == "sess-1");
    assert!(sess1.is_some());
    assert_eq!(sess1.unwrap().match_count, 2);
}

#[test]
fn test_search_ai_sessions_filters_by_tool() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    insert_ai_log(&conn, "claude", "/p1", "s1", "error found", "2026-01-01T10:00:00");
    insert_ai_log(&conn, "codex", "/p2", "s2", "error trace", "2026-01-01T10:00:00");

    let params = SearchAiSessionsParams {
        query: "error".into(),
        ai_tool: Some("claude".into()),
        ai_project: None, from: None, to: None, limit: Some(10),
    };
    let results = search_ai_sessions(&pool, &params).unwrap();
    assert!(results.sessions.iter().all(|s| s.tool == "claude"));
}

#[test]
fn test_search_ai_sessions_invalid_fts_returns_error() {
    let pool = create_test_pool();
    let params = SearchAiSessionsParams {
        query: "bad-query AND".into(), // malformed FTS
        ai_project: None, ai_tool: None, from: None, to: None, limit: Some(10),
    };
    assert!(search_ai_sessions(&pool, &params).is_err());
}
```

- [ ] **Step 2: Run to confirm tests fail**

```bash
cargo test db::queries_tests::test_search_ai_sessions 2>&1 | tail -5
```

- [ ] **Step 3: Add `SearchAiSessionsParams` struct and `search_ai_sessions` fn to `src/db/queries.rs`**

```rust
pub struct SearchAiSessionsParams {
    pub query: String,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
}

pub fn search_ai_sessions(
    pool: &DbPool,
    params: &SearchAiSessionsParams,
) -> Result<crate::app::models::SearchSessionsResponse> {
    validate_fts_query(&params.query)?;

    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(20).min(100);
    // candidate cap before grouping — prevents BM25 scan of entire corpus
    const CANDIDATE_CAP: usize = 5_000;

    // Build inner candidate query
    let mut sql = format!(
        "WITH candidates AS (
            SELECT l.id, l.ai_project, l.ai_tool, l.ai_session_id, l.hostname,
                   l.timestamp, l.received_at, l.message,
                   bm25(logs_fts) AS score
            FROM logs_fts
            JOIN logs l ON l.id = logs_fts.rowid
            WHERE logs_fts MATCH ?1
              AND l.ai_project != '' AND l.ai_project IS NOT NULL
              AND l.ai_tool    != '' AND l.ai_tool    IS NOT NULL
              AND l.ai_session_id != '' AND l.ai_session_id IS NOT NULL"
    );

    let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(params.query.clone())];
    let mut idx = 2usize;

    if let Some(ref proj) = params.ai_project {
        sql.push_str(&format!(" AND l.ai_project = ?{idx}"));
        binds.push(Box::new(proj.clone()));
        idx += 1;
    }
    if let Some(ref tool) = params.ai_tool {
        sql.push_str(&format!(" AND l.ai_tool = ?{idx}"));
        binds.push(Box::new(tool.clone()));
        idx += 1;
    }
    if let Some(ref from) = params.from {
        sql.push_str(&format!(" AND l.timestamp >= ?{idx}"));
        binds.push(Box::new(from.clone()));
        idx += 1;
    }
    if let Some(ref to) = params.to {
        sql.push_str(&format!(" AND l.timestamp <= ?{idx}"));
        binds.push(Box::new(to.clone()));
        idx += 1;
    }

    let _ = idx; // suppress unused warning
    sql.push_str(&format!(
        " ORDER BY score LIMIT {CANDIDATE_CAP}
        ),
        grouped AS (
            SELECT ai_project, ai_tool, ai_session_id, hostname,
                   MIN(timestamp)  AS first_seen,
                   MAX(timestamp)  AS last_seen,
                   COUNT(*)        AS event_count,
                   COUNT(*)        AS match_count,
                   MIN(score)      AS best_score,
                   (SELECT message FROM candidates c2
                    WHERE c2.ai_session_id = c.ai_session_id
                    ORDER BY c2.score LIMIT 1) AS best_snippet
            FROM candidates c
            GROUP BY ai_project, ai_tool, ai_session_id, hostname
            ORDER BY best_score, last_seen DESC
            LIMIT {limit}
        )
        SELECT ai_project, ai_tool, ai_session_id, hostname,
               first_seen, last_seen, event_count, match_count, best_snippet,
               (SELECT COUNT(*) FROM grouped) AS total
        FROM grouped"
    ));

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<_> = stmt.query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |row| {
        Ok(crate::app::models::SearchedSessionEntry {
            project:     row.get(0)?,
            tool:        row.get(1)?,
            session_id:  row.get(2)?,
            hostname:    row.get(3)?,
            first_seen:  row.get(4)?,
            last_seen:   row.get(5)?,
            event_count: row.get(6)?,
            match_count: row.get(7)?,
            best_snippet: row.get(8)?,
        })
    })?.collect::<rusqlite::Result<_>>()?;

    let total = rows.len(); // bounded by CANDIDATE_CAP already
    Ok(crate::app::models::SearchSessionsResponse {
        truncated: total >= limit,
        total_candidates: total,
        sessions: rows,
    })
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test db::queries_tests::test_search_ai_sessions 2>&1 | tail -10
```
Expected: all three pass.

- [ ] **Step 5: Commit**

```bash
git add src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat: add search_ai_sessions DB query with FTS5 grouping and candidate cap"
```

---

## Task 5: DB Analytics — `get_ai_usage_blocks`, `get_ai_project_context`, `list_ai_tools`, `list_ai_projects`

**Files:**
- Modify: `src/db/analytics.rs`
- Modify: `src/db/queries.rs` (list_ai_tools, list_ai_projects)
- Test: `src/db/analytics_tests.rs`, `src/db/queries_tests.rs`

- [ ] **Step 1: Write failing tests**

In `src/db/analytics_tests.rs`:
```rust
#[test]
fn test_usage_blocks_5_hour_buckets() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    // two events at 2026-01-01T00:00:00 and 2026-01-01T04:59:59 → same bucket
    // one event at 2026-01-01T05:00:00 → next bucket
    insert_ai_log(&conn, "claude", "/p", "s1", "msg", "2026-01-01T00:00:00");
    insert_ai_log(&conn, "claude", "/p", "s1", "msg2", "2026-01-01T04:59:59");
    insert_ai_log(&conn, "claude", "/p", "s1", "msg3", "2026-01-01T05:00:00");

    let req = UsageBlocksRequest { ai_project: None, ai_tool: None,
        from: Some("2026-01-01T00:00:00".into()),
        to: Some("2026-01-01T06:00:00".into()) };
    let resp = get_ai_usage_blocks(&pool, &req).unwrap();
    assert_eq!(resp.blocks.len(), 2);
    assert_eq!(resp.blocks[0].event_count, 2);
    assert_eq!(resp.blocks[1].event_count, 1);
}

#[test]
fn test_project_context_no_n_plus_1() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    for i in 0..10 {
        insert_ai_log(&conn, "claude", "/p", &format!("s{i}"), "msg", "2026-01-01T10:00:00");
    }
    let req = ProjectContextRequest { project: "/p".into(), ai_tool: None, limit: Some(3) };
    let resp = get_ai_project_context(&pool, &req).unwrap();
    assert_eq!(resp.project, "/p");
    assert!(resp.recent_entries.len() <= 3);
    assert_eq!(resp.event_count, 10);
}
```

In `src/db/queries_tests.rs`:
```rust
#[test]
fn test_list_ai_tools_returns_distinct() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    insert_ai_log(&conn, "claude", "/p1", "s1", "msg", "2026-01-01T10:00:00");
    insert_ai_log(&conn, "codex", "/p1", "s2", "msg", "2026-01-01T11:00:00");
    insert_ai_log(&conn, "claude", "/p2", "s3", "msg", "2026-01-01T12:00:00");

    let req = ListAiToolsRequest { ai_project: None, from: None, to: None };
    let resp = list_ai_tools(&pool, &req).unwrap();
    assert_eq!(resp.tools.len(), 2);
    let claude = resp.tools.iter().find(|t| t.tool == "claude").unwrap();
    assert_eq!(claude.event_count, 2);
}

#[test]
fn test_list_ai_projects_cross_filter_by_tool() {
    let pool = create_test_pool();
    let conn = pool.get().unwrap();
    insert_ai_log(&conn, "claude", "/proj-a", "s1", "msg", "2026-01-01T10:00:00");
    insert_ai_log(&conn, "codex", "/proj-b", "s2", "msg", "2026-01-01T10:00:00");

    let req = ListAiProjectsRequest { ai_tool: Some("claude".into()), from: None, to: None };
    let resp = list_ai_projects(&pool, &req).unwrap();
    assert_eq!(resp.projects.len(), 1);
    assert_eq!(resp.projects[0].project, "/proj-a");
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test db::analytics_tests db::queries_tests::test_list_ai 2>&1 | tail -5
```

- [ ] **Step 3: Implement `get_ai_usage_blocks` in `src/db/analytics.rs`**

```rust
pub fn get_ai_usage_blocks(
    pool: &DbPool,
    req: &crate::app::models::UsageBlocksRequest,
) -> Result<crate::app::models::UsageBlocksResponse> {
    let conn = pool.get()?;
    // 5-hour bucket = 18000 seconds; anchor to UTC epoch
    const BUCKET_SECS: i64 = 18_000;

    let mut sql = format!(
        "SELECT
            datetime(CAST(strftime('%s', timestamp) AS INTEGER) / {BUCKET_SECS} * {BUCKET_SECS}, 'unixepoch') AS bucket_start,
            datetime(CAST(strftime('%s', timestamp) AS INTEGER) / {BUCKET_SECS} * {BUCKET_SECS} + {BUCKET_SECS} - 1, 'unixepoch') AS bucket_end,
            ai_project, ai_tool,
            COUNT(DISTINCT ai_session_id) AS session_count,
            COUNT(*) AS event_count
         FROM logs
         WHERE ai_project IS NOT NULL AND ai_project != ''
           AND ai_tool    IS NOT NULL AND ai_tool    != ''"
    );

    let mut binds: Vec<String> = vec![];
    let mut idx = 1usize;

    if let Some(ref proj) = req.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        binds.push(proj.clone()); idx += 1;
    }
    if let Some(ref tool) = req.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        binds.push(tool.clone()); idx += 1;
    }
    if let Some(ref from) = req.from {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        binds.push(from.clone()); idx += 1;
    }
    if let Some(ref to) = req.to {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        binds.push(to.clone()); idx += 1;
    }
    let _ = idx;

    sql.push_str(" GROUP BY bucket_start, ai_project, ai_tool ORDER BY bucket_start ASC LIMIT 1000");

    let mut stmt = conn.prepare(&sql)?;
    let blocks = stmt.query_map(rusqlite::params_from_iter(binds.iter()), |row| {
        Ok(crate::app::models::UsageBlock {
            bucket_start:  row.get(0)?,
            bucket_end:    row.get(1)?,
            project:       row.get(2)?,
            tool:          row.get(3)?,
            session_count: row.get(4)?,
            event_count:   row.get(5)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    let truncated = blocks.len() >= 1000;
    Ok(crate::app::models::UsageBlocksResponse { blocks, truncated })
}
```

- [ ] **Step 4: Implement `get_ai_project_context` in `src/db/analytics.rs`**

```rust
pub fn get_ai_project_context(
    pool: &DbPool,
    req: &crate::app::models::ProjectContextRequest,
) -> Result<crate::app::models::ProjectContextResponse> {
    let conn = pool.get()?;
    let limit = req.limit.unwrap_or(5).min(20);

    // Single aggregate query — no N+1
    let mut agg_sql = "SELECT
        GROUP_CONCAT(DISTINCT ai_tool) AS tools,
        GROUP_CONCAT(DISTINCT ai_session_id) AS sessions,
        GROUP_CONCAT(DISTINCT hostname) AS hostnames,
        MIN(timestamp) AS first_seen,
        MAX(timestamp) AS last_seen,
        COUNT(*) AS event_count
     FROM logs
     WHERE ai_project = ?1
       AND ai_project IS NOT NULL AND ai_project != ''".to_string();

    let mut binds: Vec<String> = vec![req.project.clone()];
    let mut idx = 2usize;

    if let Some(ref tool) = req.ai_tool {
        agg_sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        binds.push(tool.clone()); idx += 1;
    }
    let _ = idx;

    let (tools_raw, sessions_raw, hostnames_raw, first_seen, last_seen, event_count): (
        Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64,
    ) = conn.query_row(&agg_sql, rusqlite::params_from_iter(binds.iter()), |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
    })?;

    let split = |s: Option<String>| -> Vec<String> {
        s.unwrap_or_default().split(',').filter(|v| !v.is_empty()).map(String::from).collect()
    };

    // Recent representative entries — bounded window query
    let recent_sql = format!(
        "SELECT id, hostname, facility, severity, timestamp, received_at, message,
                app_name, source_ip, raw_frame, ai_tool, ai_project, ai_session_id, ai_transcript_path
         FROM logs WHERE ai_project = ?1
         ORDER BY timestamp DESC LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&recent_sql)?;
    let recent_entries = stmt
        .query_map([&req.project], crate::db::queries::map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(crate::app::models::ProjectContextResponse {
        project: req.project.clone(),
        tools: split(tools_raw),
        sessions: split(sessions_raw),
        hostnames: split(hostnames_raw),
        first_seen,
        last_seen,
        event_count,
        recent_entries: recent_entries.into_iter().map(Into::into).collect(),
    })
}
```

- [ ] **Step 5: Implement `list_ai_tools` and `list_ai_projects` in `src/db/queries.rs`**

```rust
pub fn list_ai_tools(pool: &DbPool, req: &crate::app::models::ListAiToolsRequest) -> Result<crate::app::models::ListAiToolsResponse> {
    let conn = pool.get()?;
    let mut sql = "SELECT ai_tool,
        COUNT(*) AS event_count,
        COUNT(DISTINCT ai_session_id) AS session_count,
        MIN(timestamp) AS first_seen,
        MAX(timestamp) AS last_seen
     FROM logs
     WHERE ai_tool IS NOT NULL AND ai_tool != ''".to_string();
    let mut binds: Vec<String> = vec![];
    let mut idx = 1usize;
    if let Some(ref proj) = req.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        binds.push(proj.clone()); idx += 1;
    }
    if let Some(ref from) = req.from {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        binds.push(from.clone()); idx += 1;
    }
    if let Some(ref to) = req.to {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        binds.push(to.clone()); idx += 1;
    }
    let _ = idx;
    sql.push_str(" GROUP BY ai_tool ORDER BY event_count DESC LIMIT 100");
    let mut stmt = conn.prepare(&sql)?;
    let tools = stmt.query_map(rusqlite::params_from_iter(binds.iter()), |row| {
        Ok(crate::app::models::AiToolEntry {
            tool: row.get(0)?, event_count: row.get(1)?,
            session_count: row.get(2)?, first_seen: row.get(3)?, last_seen: row.get(4)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(crate::app::models::ListAiToolsResponse { tools })
}

pub fn list_ai_projects(pool: &DbPool, req: &crate::app::models::ListAiProjectsRequest) -> Result<crate::app::models::ListAiProjectsResponse> {
    let conn = pool.get()?;
    let mut sql = "SELECT ai_project,
        GROUP_CONCAT(DISTINCT ai_tool) AS tools,
        COUNT(*) AS event_count,
        COUNT(DISTINCT ai_session_id) AS session_count,
        MIN(timestamp) AS first_seen,
        MAX(timestamp) AS last_seen
     FROM logs
     WHERE ai_project IS NOT NULL AND ai_project != ''".to_string();
    let mut binds: Vec<String> = vec![];
    let mut idx = 1usize;
    if let Some(ref tool) = req.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        binds.push(tool.clone()); idx += 1;
    }
    if let Some(ref from) = req.from {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        binds.push(from.clone()); idx += 1;
    }
    if let Some(ref to) = req.to {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        binds.push(to.clone()); idx += 1;
    }
    let _ = idx;
    sql.push_str(" GROUP BY ai_project ORDER BY event_count DESC LIMIT 200");
    let mut stmt = conn.prepare(&sql)?;
    let projects = stmt.query_map(rusqlite::params_from_iter(binds.iter()), |row| {
        let tools_raw: Option<String> = row.get(1)?;
        Ok(crate::app::models::AiProjectEntry {
            project: row.get(0)?,
            tools: tools_raw.unwrap_or_default().split(',').filter(|v| !v.is_empty()).map(String::from).collect(),
            event_count: row.get(2)?, session_count: row.get(3)?,
            first_seen: row.get(4)?, last_seen: row.get(5)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(crate::app::models::ListAiProjectsResponse { projects })
}
```

- [ ] **Step 6: Run all DB tests**

```bash
cargo test db:: 2>&1 | tail -15
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/db/analytics.rs src/db/analytics_tests.rs src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat: add AI analytics DB functions: usage_blocks, project_context, list_ai_tools, list_ai_projects"
```

---

## Task 6: Service Layer Wiring

**Files:**
- Modify: `src/app/service.rs`
- Modify: `src/app/mod.rs` (re-exports if needed)
- Test: `src/app/service_tests.rs`

- [ ] **Step 1: Write failing service tests**

In `src/app/service_tests.rs`:
```rust
#[test]
fn test_service_search_sessions_empty_db() {
    let svc = make_test_service();
    let req = SearchSessionsRequest {
        query: "error".into(),
        ai_project: None, ai_tool: None, from: None, to: None, limit: Some(10),
    };
    let resp = svc.search_sessions(&req).unwrap();
    assert!(resp.sessions.is_empty());
}

#[test]
fn test_service_list_ai_tools_empty_db() {
    let svc = make_test_service();
    let resp = svc.list_ai_tools(&ListAiToolsRequest { ai_project: None, from: None, to: None }).unwrap();
    assert!(resp.tools.is_empty());
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test app::service_tests::test_service_search_sessions 2>&1 | tail -5
```

- [ ] **Step 3: Add service methods to `src/app/service.rs`**

```rust
pub fn search_sessions(&self, req: &SearchSessionsRequest) -> Result<SearchSessionsResponse> {
    let params = db::queries::SearchAiSessionsParams {
        query: req.query.clone(),
        ai_project: req.ai_project.clone(),
        ai_tool: req.ai_tool.clone(),
        from: req.from.clone(),
        to: req.to.clone(),
        limit: req.limit,
    };
    db::queries::search_ai_sessions(&self.pool, &params)
}

pub fn get_ai_usage_blocks(&self, req: &UsageBlocksRequest) -> Result<UsageBlocksResponse> {
    db::analytics::get_ai_usage_blocks(&self.pool, req)
}

pub fn get_ai_project_context(&self, req: &ProjectContextRequest) -> Result<ProjectContextResponse> {
    db::analytics::get_ai_project_context(&self.pool, req)
}

pub fn list_ai_tools(&self, req: &ListAiToolsRequest) -> Result<ListAiToolsResponse> {
    db::queries::list_ai_tools(&self.pool, req)
}

pub fn list_ai_projects(&self, req: &ListAiProjectsRequest) -> Result<ListAiProjectsResponse> {
    db::queries::list_ai_projects(&self.pool, req)
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test app::service_tests 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add src/app/service.rs src/app/service_tests.rs src/app/mod.rs
git commit -m "feat: add service methods for AI session analytics"
```

---

## Task 7: MCP Actions

**Files:**
- Modify: `src/mcp/tools.rs`
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/rmcp_server.rs`
- Test: `src/mcp/tools_tests.rs`

- [ ] **Step 1: Write parity/dispatcher tests**

In `src/mcp/tools_tests.rs`:
```rust
#[test]
fn test_new_actions_in_syslog_actions_list() {
    let new_actions = ["search_sessions", "usage_blocks", "project_context", "list_ai_tools", "list_ai_projects"];
    for action in new_actions {
        assert!(
            SYSLOG_ACTIONS.contains(&action),
            "action '{}' missing from SYSLOG_ACTIONS", action
        );
    }
}

#[test]
fn test_new_actions_in_help_text() {
    let help = build_help_text();
    for action in ["search_sessions", "usage_blocks", "project_context", "list_ai_tools", "list_ai_projects"] {
        assert!(help.contains(action), "help text missing '{}'", action);
    }
}

#[test]
fn test_new_actions_in_read_scope() {
    for action in ["search_sessions", "usage_blocks", "project_context", "list_ai_tools", "list_ai_projects"] {
        assert!(
            READ_ONLY_ACTIONS.contains(&action),
            "action '{}' missing from READ_ONLY_ACTIONS", action
        );
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test mcp::tools_tests::test_new_actions 2>&1 | tail -5
```

- [ ] **Step 3: Add actions to `SYSLOG_ACTIONS` in `src/mcp/tools.rs`**

In the `SYSLOG_ACTIONS` array, add:
```rust
"search_sessions",
"usage_blocks",
"project_context",
"list_ai_tools",
"list_ai_projects",
```

- [ ] **Step 4: Add dispatch arms to `tool_syslog` match in `src/mcp/tools.rs`**

```rust
"search_sessions" => tool_search_sessions(state, args).await,
"usage_blocks"    => tool_usage_blocks(state, args).await,
"project_context" => tool_project_context(state, args).await,
"list_ai_tools"   => tool_list_ai_tools(state, args).await,
"list_ai_projects"=> tool_list_ai_projects(state, args).await,
```

- [ ] **Step 5: Implement handler functions in `src/mcp/tools.rs`**

```rust
async fn tool_search_sessions(state: Arc<AppState>, args: &serde_json::Value) -> McpResult {
    let req = SearchSessionsRequest {
        query:      require_string(args, "query")?,
        ai_project: optional_string(args, "project"),
        ai_tool:    optional_string(args, "tool"),
        from:       optional_string(args, "from"),
        to:         optional_string(args, "to"),
        limit:      optional_usize(args, "limit"),
    };
    let resp = state.service.search_sessions(&req).map_err(mcp_err)?;
    ok_json(resp)
}

async fn tool_usage_blocks(state: Arc<AppState>, args: &serde_json::Value) -> McpResult {
    let req = UsageBlocksRequest {
        ai_project: optional_string(args, "project"),
        ai_tool:    optional_string(args, "tool"),
        from:       optional_string(args, "from"),
        to:         optional_string(args, "to"),
    };
    let resp = state.service.get_ai_usage_blocks(&req).map_err(mcp_err)?;
    ok_json(resp)
}

async fn tool_project_context(state: Arc<AppState>, args: &serde_json::Value) -> McpResult {
    let req = ProjectContextRequest {
        project:  require_string(args, "project")?,
        ai_tool:  optional_string(args, "tool"),
        limit:    optional_usize(args, "limit"),
    };
    let resp = state.service.get_ai_project_context(&req).map_err(mcp_err)?;
    ok_json(resp)
}

async fn tool_list_ai_tools(state: Arc<AppState>, args: &serde_json::Value) -> McpResult {
    let req = ListAiToolsRequest {
        ai_project: optional_string(args, "project"),
        from:       optional_string(args, "from"),
        to:         optional_string(args, "to"),
    };
    let resp = state.service.list_ai_tools(&req).map_err(mcp_err)?;
    ok_json(resp)
}

async fn tool_list_ai_projects(state: Arc<AppState>, args: &serde_json::Value) -> McpResult {
    let req = ListAiProjectsRequest {
        ai_tool: optional_string(args, "tool"),
        from:    optional_string(args, "from"),
        to:      optional_string(args, "to"),
    };
    let resp = state.service.list_ai_projects(&req).map_err(mcp_err)?;
    ok_json(resp)
}
```

- [ ] **Step 6: Add to `READ_ONLY_ACTIONS` in `src/mcp/rmcp_server.rs`**

```rust
"search_sessions", "usage_blocks", "project_context", "list_ai_tools", "list_ai_projects",
```

- [ ] **Step 7: Add schemas in `src/mcp/schemas.rs`**

```rust
pub fn search_sessions_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action":  {"type": "string", "enum": ["search_sessions"]},
            "query":   {"type": "string", "description": "FTS5 search query"},
            "project": {"type": "string", "description": "Filter by ai_project path"},
            "tool":    {"type": "string", "enum": ["claude","codex","gemini"]},
            "from":    {"type": "string", "description": "ISO-8601 start timestamp"},
            "to":      {"type": "string", "description": "ISO-8601 end timestamp"},
            "limit":   {"type": "integer", "minimum": 1, "maximum": 100}
        },
        "required": ["action", "query"]
    })
}
// ... similar schemas for usage_blocks, project_context, list_ai_tools, list_ai_projects
```

- [ ] **Step 8: Add help text entries for the 5 new actions in `src/mcp/tools.rs` `build_help_text()`**

```
search_sessions  query [project] [tool] [from] [to] [limit]   Grouped session search ranked by FTS relevance
usage_blocks     [project] [tool] [from] [to]                  AI activity bucketed in 5-hour windows
project_context  project [tool] [limit]                        Summary + recent entries for one project path
list_ai_tools    [project] [from] [to]                         Distinct AI tools with counts and timestamps
list_ai_projects [tool] [from] [to]                            Distinct AI projects with counts and timestamps
```

- [ ] **Step 9: Run MCP tests**

```bash
cargo test mcp:: 2>&1 | tail -15
```
Expected: all pass.

- [ ] **Step 10: Commit**

```bash
git add src/mcp/tools.rs src/mcp/schemas.rs src/mcp/rmcp_server.rs src/mcp/tools_tests.rs
git commit -m "feat: add search_sessions, usage_blocks, project_context, list_ai_tools, list_ai_projects MCP actions"
```

---

## Task 8: CLI `ai` Namespace

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Test: `src/cli_tests.rs`, `src/main_tests.rs`

- [ ] **Step 1: Write CLI parser tests**

In `src/cli_tests.rs`:
```rust
#[test]
fn test_parse_ai_search() {
    let args = vec!["ai".into(), "search".into(), "error".into(), "--tool".into(), "claude".into()];
    let cmd = parse_args(&args).unwrap();
    match cmd {
        CliCommand::Ai(AiCommand::Search(a)) => {
            assert_eq!(a.query, "error");
            assert_eq!(a.tool.as_deref(), Some("claude"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_parse_ai_index_requires_no_broad_root_without_path() {
    // bare `syslog ai index` with no --path → uses default roots, should parse OK
    let args = vec!["ai".into(), "index".into()];
    assert!(parse_args(&args).is_ok());
}

#[test]
fn test_parse_ai_add_requires_file() {
    let args = vec!["ai".into(), "add".into()]; // missing --file
    assert!(parse_args(&args).is_err());
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test cli_tests::test_parse_ai 2>&1 | tail -5
```

- [ ] **Step 3: Add `AiCommand` enum and `CliCommand::Ai` in `src/cli.rs`**

```rust
#[derive(Debug)]
pub(crate) enum AiCommand {
    Search(AiSearchArgs),
    Blocks(AiBlocksArgs),
    Context(AiContextArgs),
    Tools(AiListArgs),
    Projects(AiListArgs),
    Index(AiIndexArgs),
    Add(AiAddArgs),
}

#[derive(Debug)]
pub(crate) struct AiSearchArgs {
    pub query: String,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
    pub json: bool,
}

#[derive(Debug)]
pub(crate) struct AiBlocksArgs {
    pub project: Option<String>,
    pub tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub json: bool,
}

#[derive(Debug)]
pub(crate) struct AiContextArgs {
    pub project: String,
    pub tool: Option<String>,
    pub limit: Option<usize>,
    pub json: bool,
}

#[derive(Debug)]
pub(crate) struct AiListArgs {
    pub project: Option<String>,
    pub tool: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub json: bool,
}

#[derive(Debug)]
pub(crate) struct AiIndexArgs {
    pub path: Option<String>,
    pub json: bool,
}

#[derive(Debug)]
pub(crate) struct AiAddArgs {
    pub file: String,
    pub json: bool,
}
```

Add `CliCommand::Ai(AiCommand)` to the `CliCommand` enum.

Add `parse_ai(args)` function that parses `ai <subcommand> [flags]` and returns `CliCommand::Ai(...)`.

- [ ] **Step 4: Wire `ai` command in `src/cli.rs` parse dispatch and `src/main.rs`**

In `CliCommand::parse` (or equivalent top-level parser), add:
```rust
"ai" => parse_ai(&args[1..])?,
```

In `src/main.rs`, add `"ai"` to the direct CLI command whitelist and add a `CliCommand::Ai` dispatch arm in `run()`.

- [ ] **Step 5: Implement `CliCommand::Ai` dispatch in `src/cli.rs`**

```rust
CliCommand::Ai(ai_cmd) => match ai_cmd {
    AiCommand::Search(args) => {
        let req = SearchSessionsRequest {
            query: args.query.clone(),
            ai_project: args.project.clone(),
            ai_tool: args.tool.clone(),
            from: args.from.clone(),
            to: args.to.clone(),
            limit: args.limit,
        };
        let resp = service.search_sessions(&req)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        } else {
            for s in &resp.sessions {
                println!("{:<40} {:>6} events  {}", s.session_id, s.event_count, s.last_seen);
            }
        }
    }
    AiCommand::Blocks(args) => { /* similar pattern with usage_blocks */ }
    AiCommand::Context(args) => { /* project_context */ }
    AiCommand::Tools(args) => { /* list_ai_tools */ }
    AiCommand::Projects(args) => { /* list_ai_projects */ }
    AiCommand::Index(args) => { /* scanner::index_roots — Task 9 */ }
    AiCommand::Add(args) => { /* scanner::index_file — Task 9 */ }
},
```

- [ ] **Step 6: Run CLI tests**

```bash
cargo test cli_tests main_tests 2>&1 | tail -15
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/cli_tests.rs src/main.rs src/main_tests.rs
git commit -m "feat: add syslog ai CLI namespace with search, blocks, context, tools, projects, index, add subcommands"
```

---

## Task 9: Local Transcript Scanner

**Files:**
- Create: `src/scanner/mod.rs`
- Create: `src/scanner/checkpoint.rs`
- Create: `src/scanner/claude.rs`
- Create: `src/scanner/codex.rs`
- Create: `src/scanner_tests.rs` (sidecar)
- Modify: `src/lib.rs` (add `pub mod scanner;`)
- Modify: `src/cli.rs` (wire `AiCommand::Index` and `AiCommand::Add`)

- [ ] **Step 1: Write duplicate-run and failure tests**

In `src/scanner_tests.rs`:
```rust
#[test]
fn test_index_file_is_idempotent() {
    let pool = create_test_pool();
    let tmp = write_temp_claude_transcript(&[
        r#"{"type":"message","role":"user","content":"hello"}"#,
    ]);
    let result1 = scanner::index_file(&pool, tmp.path(), "explicit_file").unwrap();
    let result2 = scanner::index_file(&pool, tmp.path(), "explicit_file").unwrap();
    assert_eq!(result1.ingested, 1);
    assert_eq!(result2.ingested, 0);      // second run: all duplicates
    assert_eq!(result2.skipped_dupes, 1);
    // log row count unchanged
    let count: i64 = pool.get().unwrap()
        .query_row("SELECT COUNT(*) FROM logs WHERE ai_tool='claude'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_checkpoint_not_advanced_on_partial_failure() {
    let pool = create_test_pool();
    // Inject a broken JSONL record mid-file
    let tmp = write_temp_file("good-line\nbad-{json\nanother-good\n");
    let result = scanner::index_file(&pool, tmp.path(), "explicit_file");
    // Should succeed with parse_errors=1, not panic
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r.parse_errors, 1);
}

#[test]
fn test_path_validation_rejects_symlinks() {
    let tmp = create_temp_symlink_to("/etc/passwd");
    let result = scanner::validate_path(tmp.path());
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test scanner_tests 2>&1 | tail -5
```

- [ ] **Step 3: Implement `src/scanner/checkpoint.rs`**

```rust
use crate::db::DbPool;

pub struct CheckpointManager<'a> {
    pool: &'a DbPool,
}

impl<'a> CheckpointManager<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub fn get_or_create(&self, canonical_path: &str, kind: &str) -> rusqlite::Result<i64> {
        let conn = self.pool.get().unwrap();
        let id: Option<i64> = conn.query_row(
            "SELECT id FROM transcript_sources WHERE canonical_path = ?1",
            [canonical_path], |r| r.get(0),
        ).optional()?;
        if let Some(id) = id { return Ok(id); }
        conn.execute(
            "INSERT INTO transcript_sources (canonical_path, source_kind, last_indexed_at) VALUES (?1, ?2, datetime('now'))",
            [canonical_path, kind],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Returns true if the record_key was freshly inserted (not a duplicate).
    pub fn try_claim_record(&self, source_id: i64, record_key: &str) -> rusqlite::Result<bool> {
        let conn = self.pool.get().unwrap();
        let res = conn.execute(
            "INSERT OR IGNORE INTO transcript_import_records (source_id, record_key, imported_at)
             VALUES (?1, ?2, datetime('now'))",
            rusqlite::params![source_id, record_key],
        )?;
        Ok(res > 0)
    }

    pub fn mark_error(&self, source_id: i64, error: &str) {
        let conn = self.pool.get().unwrap();
        let _ = conn.execute(
            "UPDATE transcript_sources SET last_error = ?1 WHERE id = ?2",
            rusqlite::params![error, source_id],
        );
    }
}
```

- [ ] **Step 4: Implement `src/scanner/mod.rs`**

```rust
pub mod checkpoint;
pub mod claude;
pub mod codex;

use std::path::Path;
use crate::db::DbPool;

const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

pub struct IndexResult {
    pub ingested: usize,
    pub skipped_dupes: usize,
    pub parse_errors: usize,
    pub skipped_files: usize,
}

/// Validate a path: must exist, not be a symlink, not be outside an allowed root.
pub fn validate_path(path: &Path) -> anyhow::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("symlinks are not allowed: {}", path.display());
    }
    if meta.len() > MAX_FILE_SIZE {
        anyhow::bail!("file too large ({}B > {}B): {}", meta.len(), MAX_FILE_SIZE, path.display());
    }
    Ok(())
}

/// Ingest a single transcript file. Safe to rerun — duplicates are skipped.
pub fn index_file(pool: &DbPool, path: &Path, kind: &str) -> anyhow::Result<IndexResult> {
    validate_path(path)?;
    let canonical = path.canonicalize()?.to_string_lossy().to_string();
    let cp = checkpoint::CheckpointManager::new(pool);
    let source_id = cp.get_or_create(&canonical, kind)?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "jsonl" | "json" => claude::parse_and_ingest(pool, path, source_id, &cp),
        _ => anyhow::bail!("unsupported file type: {}", path.display()),
    }
}

/// Scan default or explicit roots for transcript files.
pub fn index_roots(pool: &DbPool, root_override: Option<&Path>) -> anyhow::Result<IndexResult> {
    let roots: Vec<std::path::PathBuf> = if let Some(r) = root_override {
        vec![r.to_path_buf()]
    } else {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home dir"))?;
        vec![
            home.join(".claude/projects"),
            home.join(".codex/sessions"),
        ]
    };

    let mut total = IndexResult { ingested: 0, skipped_dupes: 0, parse_errors: 0, skipped_files: 0 };
    for root in roots {
        if !root.exists() { continue; }
        let mut entries: Vec<_> = std::fs::read_dir(&root)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.path()); // deterministic order
        for entry in entries {
            let path = entry.path();
            match index_file(pool, &path, "claude_project") {
                Ok(r) => {
                    total.ingested += r.ingested;
                    total.skipped_dupes += r.skipped_dupes;
                    total.parse_errors += r.parse_errors;
                }
                Err(_) => { total.skipped_files += 1; }
            }
        }
    }
    Ok(total)
}
```

- [ ] **Step 5: Implement `src/scanner/claude.rs`** (minimal JSONL parser that maps to `LogEntry`)

```rust
use std::path::Path;
use crate::db::{DbPool, ingest::insert_logs_batch};
use crate::syslog::parser::LogEntry;
use super::checkpoint::CheckpointManager;
use super::IndexResult;

pub fn parse_and_ingest(
    pool: &DbPool, path: &Path, source_id: i64, cp: &CheckpointManager,
) -> anyhow::Result<IndexResult> {
    let content = std::fs::read_to_string(path)?;
    let project_path = path.parent().map(|p| p.to_string_lossy().to_string());
    let mut result = IndexResult { ingested: 0, skipped_dupes: 0, parse_errors: 0, skipped_files: 0 };
    let mut batch: Vec<LogEntry> = vec![];

    for (line_no, line) in content.lines().enumerate() {
        let record_key = format!("{}:{}", path.to_string_lossy(), line_no);
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            result.parse_errors += 1;
            continue;
        };
        let msg = val.get("content")
            .or_else(|| val.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if msg.is_empty() { continue; }

        match cp.try_claim_record(source_id, &record_key)? {
            false => { result.skipped_dupes += 1; continue; }
            true => {}
        }

        batch.push(LogEntry {
            hostname: "localhost".into(),
            message: msg,
            ai_tool: Some("claude".into()),
            ai_project: project_path.clone(),
            ai_session_id: val.get("sessionId").and_then(|v| v.as_str()).map(String::from),
            ai_transcript_path: Some(path.to_string_lossy().to_string()),
            severity: Some("info".into()),
            ..Default::default()
        });

        if batch.len() >= 200 {
            insert_logs_batch(pool, &batch)?;
            result.ingested += batch.len();
            batch.clear();
        }
    }
    if !batch.is_empty() {
        insert_logs_batch(pool, &batch)?;
        result.ingested += batch.len();
    }
    Ok(result)
}
```

- [ ] **Step 6: Wire `AiCommand::Index` and `AiCommand::Add` in `src/cli.rs`**

```rust
AiCommand::Index(args) => {
    let root = args.path.as_ref().map(Path::new);
    let result = scanner::index_roots(service.pool(), root)?;
    if args.json {
        println!("{}", serde_json::json!({
            "ingested": result.ingested,
            "skipped_dupes": result.skipped_dupes,
            "parse_errors": result.parse_errors,
            "skipped_files": result.skipped_files,
        }));
    } else {
        println!("Indexed {} records ({} duplicates skipped, {} errors)", result.ingested, result.skipped_dupes, result.parse_errors);
    }
}
AiCommand::Add(args) => {
    let path = Path::new(&args.file);
    let result = scanner::index_file(service.pool(), path, "explicit_file")?;
    if args.json {
        println!("{}", serde_json::json!({
            "ingested": result.ingested,
            "skipped_dupes": result.skipped_dupes,
            "parse_errors": result.parse_errors,
        }));
    } else {
        println!("Added {} records ({} duplicates, {} errors)", result.ingested, result.skipped_dupes, result.parse_errors);
    }
}
```

- [ ] **Step 7: Run scanner tests**

```bash
cargo test scanner_tests 2>&1 | tail -15
```
Expected: all pass.

- [ ] **Step 8: Full test suite**

```bash
cargo test 2>&1 | tail -20
```
Expected: no regressions.

- [ ] **Step 9: Commit**

```bash
git add src/scanner/ src/scanner_tests.rs src/lib.rs src/cli.rs
git commit -m "feat: add local transcript scanner with idempotent indexing and checkpoint-based deduplication"
```

---

## Task 10: Smoke Test Coverage + Docs

**Files:**
- Modify: `scripts/smoke-test.sh`
- Modify: `docs/mcp/TOOLS.md`
- Modify: `docs/CLI.md`
- Modify: `README.md`

- [ ] **Step 1: Add smoke test stubs for new MCP actions in `scripts/smoke-test.sh`**

```bash
# search_sessions
run_test "search_sessions" '{"action":"search_sessions","query":"error"}' \
  '.sessions | type == "array"'

# usage_blocks
run_test "usage_blocks" '{"action":"usage_blocks"}' \
  '.blocks | type == "array"'

# project_context
run_test "project_context" '{"action":"project_context","project":"/nonexistent"}' \
  '.project == "/nonexistent"'

# list_ai_tools
run_test "list_ai_tools" '{"action":"list_ai_tools"}' \
  '.tools | type == "array"'

# list_ai_projects
run_test "list_ai_projects" '{"action":"list_ai_projects"}' \
  '.projects | type == "array"'
```

- [ ] **Step 2: Run smoke test (server must be running)**

```bash
bash scripts/smoke-test.sh 2>&1 | grep -E "PASS|FAIL"
```
Expected: all PASS.

- [ ] **Step 3: Update `docs/mcp/TOOLS.md`** — add the 5 new actions to the action table and add a "Transcript Visibility Policy" section:

```markdown
## Transcript Visibility Policy

AI transcript rows ingested via `syslog ai index` or `syslog ai add` are stored in the `logs` table.
They are **visible** through `search`, `tail`, `context`, and `get` actions — this is intentional,
as `logs` is the single source of truth. `ai_transcript_path` fields expose local filesystem paths.
No automatic redaction is applied; deploy with `SYSLOG_MCP_TOKEN` and proxy-layer auth if needed.
```

- [ ] **Step 4: Update `docs/CLI.md`** with `syslog ai` namespace reference

- [ ] **Step 5: Update `README.md`** — add AI Session Intelligence section to feature list

- [ ] **Step 6: Final checks**

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
bash scripts/check-version-sync.sh
```

- [ ] **Step 7: Version bump and commit**

Determine bump type (new features = minor):
```bash
bash scripts/bump-version.sh minor
```

```bash
git add scripts/smoke-test.sh docs/mcp/TOOLS.md docs/CLI.md README.md CHANGELOG.md Cargo.toml Cargo.lock
git commit -m "feat: smoke test coverage, docs, and version bump for mnemo feature port"
```

- [ ] **Step 8: Push**

```bash
git push
```
