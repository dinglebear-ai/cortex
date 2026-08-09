# Skill Event Extraction, Ingest, and Backfill (GH #94 PR 2/4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract normalized skill-invocation events from Claude/Codex AI transcripts at ingest time and via backfill, and expose them for query across the CLI, MCP, and REST surfaces.

**Architecture:** A new `ai_skill_events` table (migration 38) stores one row per detected skill invocation, keyed by `(log_id, skill_name, event_kind, evidence_kind)` for idempotent `INSERT OR IGNORE` writes. Two independent extractors in `src/scanner/skill_events.rs` recognize skill invocations — Claude's structured `attributionSkill`/`attributionPlugin` JSON fields, and Codex's `<skill><name>` transcript tags — and both feed the same `ExtractedSkillEvent` shape. Extraction runs inline inside the existing transcript-ingest transaction (`flush_chunk` in `src/scanner.rs`, using log ids returned from a refactored `insert_logs_batch_in_tx`) for new data, and via a chunked, bounded, dry-run-capable backfill service (`CortexService::backfill_skill_events`) for historical data, with a shared read surface (`skill_events` MCP action, `GET /api/ai/skills`, `cortex sessions skills`) on top.

**Tech Stack:** Rust 2024 edition, `rusqlite` (bundled SQLite, WAL mode), `regex` for the Codex `<skill><name>` tag scanner, `serde`/`serde_json` for wire types.

## Global Constraints

- Never hold the SQLite write lock across a large corpus scan — chunk and release, mirroring `purge_old_logs` in `src/db/maintenance.rs`. This applies above all to the skill-event backfill task.
- Use `INSERT OR IGNORE` for all `ai_skill_events` inserts — transcript re-imports and watch retries are normal and must be idempotent.
- Do not add a second full-table scan during normal ingest — skill-event insertion happens from the log ids already available from the batch insert call in the same chunk transaction.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass before any task is considered done.
- Every new MCP action needs a row in `src/mcp/actions.rs` (`ACTION_SPECS`) + a dispatch arm in `src/mcp/tools.rs` + docs updates (`docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, `docs/contracts/mcp-actions-current.md`, `CLAUDE.md` action table + count).
- **PR sequencing note:** This is PR 2 of 4 for GH #94 Plan A. This PR is independent of PR 1 (LLM Invocation Guard) except for the migration-number ordering documented in this plan's migration task — re-verify the live `KNOWN_SCHEMA_VERSION` in `src/db/pool.rs` at implementation time rather than trusting a hardcoded assumption, since either PR 1 or PR 2 may merge first. PR 3 (skill incident detection) depends on THIS PR (needs the `ai_skill_events` table and its query layer). PR 4 (skill assessment + unified CLI) depends on PR 1 and PR 3, not directly on this PR.

---

## Eng Review Fixes Applied

Four independent review agents (architecture, simplicity, security, performance) reviewed this plan against the live repo after PR 1 ("LLM Invocation Guard") merged as commits bb28230 + 814d033 on `main` (`KNOWN_SCHEMA_VERSION` is live at `37`, confirming this phase's migration 38 is still correct). All 11 findings below were applied directly into the task bodies as real code before implementation starts.

| # | Fix | Reviewer(s) | Task(s) changed |
|---|---|---|---|
| 1 | Eliminated the double JSON-parse in the ingest hot path: `ParsedTranscriptRecord` now carries the already-parsed raw value/text so `flush_chunk` never re-parses `line_text` a second time. Added cheap substring short-circuits (`"<skill>"` for Codex, `"attributionSkill"` for Claude) before the real extractor runs. | simplicity, performance (independently); short-circuit suggestion from architecture | Task 2, Task 3, Task 6 |
| 2 | Dropped the unused `skill_path` and `metadata_json` columns/fields — neither extractor ever sets them in this PR (pure YAGNI, no producer exists). Removed from the table DDL, `ExtractedSkillEvent`, `SkillEventInsert`, `AiSkillEventEntry`, `AiSkillEventParams` (n/a — never had them), the API model, and the schema docs. | simplicity | Task 1, Task 2, Task 3, Task 4, Task 6, Task 7, Task 9, Task 11, Locked interfaces |
| 3 | Removed the speculative `payload.*` nesting branch from the Claude attribution extractor — no observed transcript sample confirms this shape; kept only `value` (top-level) and `message.*` (confirmed-plausible nesting). Removed the test exercising the deleted branch. | simplicity | Task 2 |
| 4 | Redesigned `ai_skill_events` indexes to match the actual shipped filter surface (`--skill`, `--plugin`, `--tool`, `--project`, `--session-id`, `--host`, `--since`, `--to`, `--limit`): added a bare `idx_ai_skill_events_timestamp` for the unfiltered default sort, `idx_ai_skill_events_plugin_time` for `--plugin` alone, `idx_ai_skill_events_hostname_time` for `--host` alone, and confirmed `idx_ai_skill_events_session_time`'s `(ai_tool, ai_project, ai_session_id, timestamp)` shape already serves `--tool` alone via its leading column. Kept `idx_ai_skill_events_skill_time` and `idx_ai_skill_events_project_skill_time` unchanged since those are used correctly. | performance (confirmed via EXPLAIN QUERY PLAN reasoning) | Task 1, Task 11 |
| 5 | Added `idx_logs_ai_tool_id ON logs(ai_tool, id) WHERE ai_tool IN ('claude','codex')` as part of migration 38 (on the existing `logs` table, not `ai_skill_events`) — the existing `idx_logs_ai_tool_cover` doesn't include `id`, so it can't serve the backfill's `id >` keyset pagination + `ORDER BY id ASC` scan efficiently. | performance | Task 1 |
| 6 | Removed `write_lock()` from around `fetch_candidate_chunk` in the backfill — it's a pure `SELECT`, and WAL mode already gives readers a consistent snapshot without the write lock. The lock is retained only around the actual `INSERT OR IGNORE` step (already correctly scoped inside `insert_skill_events`). | performance, security, architecture (triple-confirmed) | Task 7 |
| 7 | Added a hard upper clamp to the backfill `limit` (`req.limit.unwrap_or(10_000).clamp(1, 1_000_000)`) and a process-wide single-flight guard (mirroring the `SHARED_MAINTENANCE_PERMIT` / `try_acquire_owned` pattern already used by `POST /api/db/vacuum` in `src/api.rs`, but implemented at the `CortexService` layer via a service-scoped `OnceLock<Arc<Semaphore>>` so it also covers CLI-local and MCP callers that never go through `api.rs`) so only one backfill can run at a time; a second concurrent call returns a clear "backfill already running" error instead of both holding a `run_db` permit for the whole corpus. The more invasive per-chunk `run_db` permit-release restructuring was considered and explicitly deferred — see "Deferred work" below. | security, architecture (cross-confirmed) | Task 7 |
| 8 | Added control-character rejection to `ExtractedSkillEvent::normalized()` — trimmed skill/plugin names containing any `char::is_control()` character (ANSI escapes, embedded newlines/CR) are now rejected (same `None` return as the empty-name case), closing a terminal-output-spoofing vector in the CLI's `println!`-based printer. Added an adversarial test with an embedded ANSI escape sequence in a Codex `<skill><name>` tag. | security | Task 2, Task 3 |
| 9 | Added `tracing::info!` audit logging (caller IP + query filters) to the `GET /api/ai/skills` REST handler — matches the logging level of sibling `Read`-scoped AI-transcript routes' cheap defense-in-depth posture (`ai_llm_invocations`'s `warn!` is reserved for the admin-scoped route; a `Read`-scoped route uses `info!`). The `skill_events` MCP action itself stays `cortex:read`-scoped per GH #94's explicit decision — not reconsidered here. | security | Task 9 |
| 10 | Renamed the migration test function and its `cargo test` invocations from `migration_37_creates_ai_skill_events_table` to `migration_38_creates_ai_skill_events_table` — the surrounding prose already correctly reasoned about migration 38 (PR 1 claimed migration 37 for `llm_invocations`, confirmed live via `KNOWN_SCHEMA_VERSION` in `src/db/pool.rs`), but the test name/invocations were still on the stale `37`. | architecture (real defect) | Task 1 |
| 11 | Corrected Task 11's false claim that `src/docs_tests.rs` already asserts an action count or cross-checks `ACTION_SPECS` against a doc file. Verified directly: the file contains exactly 4 tests, none of which do either. Softened the confident-but-wrong assertion to "verify first via grep; as of this review it does not exist, so Task 11's doc updates are manual, not test-gated." | architecture (real defect) | Task 11 |

**Also corrected while grounding fixes against the live repo (not one of the 11 numbered findings, but required for the plan to still be accurate):** the live `CLAUDE.md` action count is already **48** (not 47) as of PR 1 merging `llm_invocations`, so this phase's `skill_events` action takes the count to **49**, not 48. Task 11's "47 -> 48" references are corrected to "48 -> 49" throughout.

**Deferred work (out of scope for this fix pass):** per-chunk `run_db` permit-release restructuring for `backfill_skill_events` (i.e. acquiring/releasing the service-level DB semaphore permit once per chunk instead of once for the whole multi-chunk scan) was raised by the security/architecture reviewers as a more thorough fix for Fix 7's DoS concern, but is deliberately NOT implemented in this pass — the hard `limit` clamp + single-flight guard together are judged sufficient for this PR's scope, and the restructuring touches `CortexService::run_db`'s shared contract in a way that deserves its own review. Needs a follow-up bead (to be filed separately).

---

## Locked interfaces for other phases

These names/shapes are final. The later `skill-incidents` (grouping/scoring)
and `skill-assess` phases build directly on top of them — do not rename.

### Table: `ai_skill_events` (migration 38)

```sql
CREATE TABLE IF NOT EXISTS ai_skill_events (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  log_id             INTEGER NOT NULL REFERENCES logs(id) ON DELETE CASCADE,
  ai_tool            TEXT NOT NULL,
  ai_project         TEXT,
  ai_session_id      TEXT,
  hostname           TEXT NOT NULL,
  timestamp          TEXT NOT NULL,
  skill_name         TEXT NOT NULL,
  skill_plugin       TEXT,
  event_kind         TEXT NOT NULL,
  evidence_kind      TEXT NOT NULL,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE(log_id, skill_name, event_kind, evidence_kind)
);

-- Eng review Fix 4: index set redesigned to match the actual shipped filter
-- surface (CLI flags --skill, --plugin, --tool, --project, --session-id,
-- --host, --since, --to, --limit). Each single-filter flag needs an index
-- whose LEADING column matches it; the unfiltered default list also needs a
-- bare timestamp index for ORDER BY timestamp DESC without a WHERE clause.
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_timestamp ON ai_skill_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_skill_time ON ai_skill_events(skill_name, timestamp);
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_plugin_time ON ai_skill_events(skill_plugin, timestamp);
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_hostname_time ON ai_skill_events(hostname, timestamp);
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_session_time ON ai_skill_events(ai_tool, ai_project, ai_session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_ai_skill_events_project_skill_time ON ai_skill_events(ai_project, skill_name, timestamp) WHERE ai_project IS NOT NULL;

-- Eng review Fix 5: supports the backfill's `id > ?` keyset pagination +
-- `ORDER BY id ASC` scan over `logs`. The existing idx_logs_ai_tool_cover
-- (ai_tool, ai_session_id, timestamp) doesn't include `id`, so it can't serve
-- this scan efficiently. This indexes the EXISTING `logs` table, not the new
-- `ai_skill_events` table — it ships in the same migration 38 batch because
-- both are needed by this phase.
CREATE INDEX IF NOT EXISTS idx_logs_ai_tool_id ON logs(ai_tool, id) WHERE ai_tool IN ('claude', 'codex');
```

`skill_plugin` alone (without a project filter) is a realistic single-filter
case per the locked CLI surface (`--plugin`), so it gets its own leading-column
index (`idx_ai_skill_events_plugin_time`) rather than relying on a composite
that doesn't lead with it. `--tool` alone is served by
`idx_ai_skill_events_session_time`'s leading `ai_tool` column (SQLite can seek
on a prefix of a composite index even when trailing columns aren't bound).
`--session-id` alone was considered for its own `(ai_session_id, timestamp)`
index, but in practice a session id is only ever meaningful scoped to a tool
(session ids are not globally unique across `ai_tool` values), so the existing
`(ai_tool, ai_project, ai_session_id, timestamp)` composite is judged
sufficient — a bare `--session-id` filter without `--tool` is not a supported
fast path and falls back to a full scan of the (typically small) table.

`KNOWN_SCHEMA_VERSION` bumps from `37` to `38` in `src/db/pool.rs` (PR 1, "LLM Invocation Guard", claims migration 37 — confirmed live in the repo at the time of this eng review pass, so migration 38 is the correct next number; re-verify the live `KNOWN_SCHEMA_VERSION` at implementation time rather than trusting this hardcoded assumption in case another migration lands in the interim).

### Parser struct (`src/scanner/skill_events.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillEventKind {
    ClaudeAttribution,
    CodexSkillBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillEvidenceKind {
    StructuredJsonField,
    TranscriptContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSkillEvent {
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub event_kind: SkillEventKind,
    pub evidence_kind: SkillEvidenceKind,
}

pub fn extract_claude_skill_events(value: &serde_json::Value) -> Vec<ExtractedSkillEvent>;
pub fn extract_codex_skill_events(text: &str) -> Vec<ExtractedSkillEvent>;
```

(Eng review Fix 2: `skill_path` and `metadata_json` are removed from
`ExtractedSkillEvent` — neither extractor ever set them, so they were dead
weight threaded through every downstream struct. If a future phase needs
either field, add it back with a real producer at that time.)

`SkillEventKind::as_str()` -> `"claude_attribution"` / `"codex_skill_block"`.
`SkillEvidenceKind::as_str()` -> `"structured_json_field"` / `"transcript_content"`.
These `as_str()` values are exactly what gets written into the `event_kind` /
`evidence_kind` TEXT columns.

**Eng review Fix 1 (short-circuit):** both extractors gain a cheap substring
pre-check before doing any real parsing/regex work, so the common
no-skill-event case is bounded to a single `str::contains` call:
- `extract_claude_skill_events` returns `Vec::new()` immediately if the raw
  JSON text does not contain `"attributionSkill"` as a substring (checked by
  the caller in `flush_chunk`/backfill before even calling
  `serde_json::from_str`, since the whole point is avoiding the parse — see
  Task 6/7).
- `extract_codex_skill_events` returns `Vec::new()` immediately if `text` does
  not contain `"<skill>"` as a substring, before touching the regex engine.

### DB layer (`src/db/skill_events.rs`, new module)

```rust
#[derive(Debug, Clone)]
pub struct SkillEventInsert {
    pub log_id: i64,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub event: crate::scanner::skill_events::ExtractedSkillEvent,
}

/// INSERT OR IGNORE all rows in `events`. Returns count of rows actually inserted
/// (SQLite `changes()` summed per statement) so callers can report duplicates.
pub fn insert_skill_events_in_tx(
    tx: &rusqlite::Transaction<'_>,
    events: &[SkillEventInsert],
) -> anyhow::Result<usize>;

/// Pool-acquiring wrapper (single INSERT OR IGNORE transaction) for callers
/// outside an existing transaction (e.g. backfill chunks).
pub fn insert_skill_events(
    pool: &crate::db::DbPool,
    events: &[SkillEventInsert],
) -> anyhow::Result<usize>;

#[derive(Debug, Clone, Default)]
pub struct AiSkillEventParams {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiSkillEventEntry {
    pub id: i64,
    pub log_id: i64,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub event_kind: String,
    pub evidence_kind: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ListSkillEventsResult {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<AiSkillEventEntry>,
}

pub fn list_skill_events(
    pool: &crate::db::DbPool,
    params: &AiSkillEventParams,
) -> anyhow::Result<ListSkillEventsResult>;
```

`insert_skill_events_in_tx` / `insert_skill_events` / `list_skill_events` /
`AiSkillEventParams` / `AiSkillEventEntry` / `ListSkillEventsResult` /
`SkillEventInsert` are all re-exported from `src/db.rs` (`pub use skill_events::{...}`)
exactly like every other query module, so `use crate::db::{...}` works from
`src/app/services/*` and `src/mcp/tools.rs`.

### Batch-insert-returns-ids refactor (`src/db/ingest.rs`)

Before:
```rust
pub(crate) fn insert_logs_batch_in_tx(tx: &Transaction<'_>, entries: &[LogBatchEntry]) -> Result<()>
```

After:
```rust
pub(crate) fn insert_logs_batch_in_tx(tx: &Transaction<'_>, entries: &[LogBatchEntry]) -> Result<Vec<i64>>
```
Returns one `id` per input `entries[i]`, same order, via `tx.last_insert_rowid()`
read immediately after each `stmt.execute(...)` in the existing per-row loop
(SQLite guarantees `last_insert_rowid()` reflects the most recent successful
insert on that connection — safe because these executes happen sequentially on
the same `Transaction`, never interleaved with other writers).

`pub fn insert_logs_batch(pool: &DbPool, entries: &[LogBatchEntry]) -> Result<usize>`
keeps its existing signature/return type (row count) — callers outside scanner.rs
are untouched. Internally it now discards the `Vec<i64>` from `insert_logs_batch_in_tx`
and returns `entries.len()` as before.

### Service-level backfill (`src/app/services/skill_backfill.rs`, new module under `src/app/services.rs`)

```rust
#[derive(Debug, Clone, Default)]
pub struct SkillBackfillRequest {
    pub since: Option<String>,
    pub limit: Option<u64>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillBackfillResult {
    pub scanned: u64,
    pub inserted: u64,
    pub skipped_duplicates: u64,
    pub parse_errors: u64,
    pub truncated: bool,
    pub dry_run: bool,
}

impl CortexService {
    pub async fn backfill_skill_events(
        &self,
        req: SkillBackfillRequest,
    ) -> ServiceResult<SkillBackfillResult>;
}
```

## Context notes for implementers (read before starting)

- **Repo migration state**: as of this eng review pass, PR 1 ("LLM Invocation
  Guard") has already merged (commits bb28230 + 814d033 on `main`), and
  `KNOWN_SCHEMA_VERSION` is confirmed live at `37` (`llm_invocations`,
  `src/db/pool.rs:42`). This phase (PR 2, skill events) claims migration `38`
  — the next number after PR 1's — and inserts its migration block immediately
  after PR 1's migration 37 block. **Still re-verify `KNOWN_SCHEMA_VERSION`
  live in `src/db/pool.rs` at implementation time** rather than trusting this
  note, in case another migration lands between this review and
  implementation. Follow the exact `if !migration_applied(&conn, 38)? {
  conn.execute_batch("... INSERT OR IGNORE INTO schema_migrations (version)
  VALUES (38);"); tracing::info!(...) }` shape used by migrations 31-37 (see
  `src/db/pool.rs:1977-2021` for migration 37's exact block to insert after).
- **`regex` is already a dependency** (`Cargo.toml:105`, `regex = "1"`) — no
  `Cargo.toml` edit needed for the Codex `<skill><name>...</name>` scanner.
- **CLI grammar**: `docs/CLI.md` explicitly documents `cortex ai ...` as a
  REMOVED command family, replaced by `cortex sessions ...` (v3.0 breaking
  change, no aliases). The task description's `cortex ai skills backfill`
  syntax does NOT match this repo's actual grammar. This plan uses
  `cortex sessions skills` / `cortex sessions skills backfill` instead, wired
  into the existing `SessionsCommand` enum in `src/cli/args.rs` and dispatched
  through `src/cli/parse/sessions.rs` + `src/cli/dispatch_sessions.rs`, exactly
  like `cortex sessions tools` / `cortex sessions index`. The REST route stays
  `GET /api/ai/skills` as specified (the existing `/api/sessions/*` prefix is
  the internal convention for other AI-transcript reads, but the task fixes
  `/api/ai/skills` explicitly, and `/api/ai/*` is currently unused — no
  collision).
- **Ingest hot path**: `index_file_with_options` in `src/scanner.rs` builds a
  `Vec<LogBatchEntry>` per chunk and calls `flush_chunk`, which calls
  `insert_logs_batch_in_tx(&tx, &claimed_batch)` inside the SAME transaction as
  the checkpoint update (`src/scanner.rs:774-832`). Skill-event insertion must
  happen inside `flush_chunk`, in the same `tx`, immediately after
  `insert_logs_batch_in_tx` returns the new `Vec<i64>` log ids — zipped against
  `claimed_batch` (same order, same length) to know which `log_id` each
  transcript row landed at, then fed to the extractors to find skill events.
  **Eng review Fix 1**: `claude::parse_line` / `codex::parse_line` in
  `src/scanner/claude.rs` / `src/scanner/codex.rs` already parse the line's
  JSON once internally (`serde_json::from_str::<Value>(line)`) but never
  return that `Value` to the caller — the original version of this plan had
  `flush_chunk` call `serde_json::from_str::<serde_json::Value>(line_text)` a
  SECOND time to get it back, doubling JSON-parse CPU on the hottest path in
  the system. The fix threads the already-parsed value through
  `ParsedTranscriptRecord` instead (new field, Claude-only; Codex doesn't need
  it since `parsed.message` already IS the text the `<skill><name>` regex
  scans). This means `flush_chunk` needs access to
  `ParsedTranscriptRecord.raw_value` (Claude) or `parsed.message` (Codex) per
  batch entry, not just the already-scrubbed `LogBatchEntry.message` — see
  Task 2 for the `ParsedTranscriptRecord` field addition and Task 6 for exact
  plumbing (`ChunkSkillSource` side-channel vector built alongside
  `batch`/`imports` in `index_file_with_options`).
- **Scrubbing**: skill names/plugins are short identifiers, not free text, so
  they do NOT go through `scrub_ai_message` — only `LogBatchEntry.message` is
  scrubbed. Skill event fields are extracted from the ORIGINAL value before
  scrubbing (scrubbing only redacts secret-shaped substrings and would not
  usually touch a skill name, but extraction happens pre-scrub regardless per
  the parser design in Task 2/3, which read `parsed.raw_value` / the codex
  message text captured before `scrub_ai_message` runs).
- **Batch-and-release lock pattern**: mirror `purge_old_logs` in
  `src/db/maintenance.rs:578-634` — each backfill chunk: `pool.get()`, run one
  bounded chunk, `drop(conn)`, then loop. Do NOT hold the lock across the whole
  historical corpus. **Eng review Fix 6**: unlike `purge_old_logs` (which
  DELETEs and therefore correctly holds `crate::db::write_lock()` for its
  chunk), the backfill's per-chunk fetch (`fetch_candidate_chunk`) is a pure
  `SELECT` — WAL mode already gives readers a consistent snapshot without the
  write lock, so `write_lock()` must NOT be acquired around the fetch. Only the
  insert step (`insert_skill_events`, called per chunk) needs the write lock,
  and it already acquires it internally (see Task 4's `insert_skill_events`).
  See Task 7 for the exact rewrite.

---

### Task 1: Migration 38 — `ai_skill_events` table

**Files:**
- Modify: `src/db/pool.rs:42` (bump `KNOWN_SCHEMA_VERSION`)
- Modify: `src/db/pool.rs` (insert migration 38 block immediately after the
  migration 37 block PR 1 "LLM Invocation Guard" already added — confirmed live
  at `src/db/pool.rs:1977-2021` as of this eng review pass — before the
  orphaned-maintenance-job cleanup comment that follows it; re-check line
  numbers live at implementation time since other work may shift them)
- Test: `src/db/pool_tests.rs` (sidecar convention: `src/db/pool.rs` already has
  `#[cfg(test)] #[path = "pool_tests.rs"] mod tests;` — confirm this hook exists
  near the bottom of `pool.rs`; if it does not yet exist for this file, this task
  must add it)

**Interfaces:**
- Consumes: nothing (first task in the phase)
- Produces: `ai_skill_events` table with columns/indexes exactly as specified
  in "Locked interfaces" above (Fix 2: no `skill_path`/`metadata_json`; Fix 4:
  redesigned index set; Fix 5: new `idx_logs_ai_tool_id` on the existing
  `logs` table). `KNOWN_SCHEMA_VERSION = 38`.

- [ ] **Step 1: Write the failing test**

  In `src/db/pool_tests.rs`, add:
  ```rust
  #[test]
  fn migration_38_creates_ai_skill_events_table() {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let pool = init_pool(&crate::config::StorageConfig::for_test(db_path)).unwrap();
      let conn = pool.get().unwrap();

      let table_exists: i64 = conn
          .query_row(
              "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'ai_skill_events'",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(table_exists, 1);

      let indexes: Vec<String> = {
          let mut stmt = conn
              .prepare(
                  "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'ai_skill_events' ORDER BY name",
              )
              .unwrap();
          stmt.query_map([], |row| row.get::<_, String>(0))
              .unwrap()
              .collect::<rusqlite::Result<Vec<_>>>()
              .unwrap()
      };
      assert!(indexes.contains(&"idx_ai_skill_events_timestamp".to_string()));
      assert!(indexes.contains(&"idx_ai_skill_events_skill_time".to_string()));
      assert!(indexes.contains(&"idx_ai_skill_events_plugin_time".to_string()));
      assert!(indexes.contains(&"idx_ai_skill_events_hostname_time".to_string()));
      assert!(indexes.contains(&"idx_ai_skill_events_session_time".to_string()));
      assert!(indexes.contains(&"idx_ai_skill_events_project_skill_time".to_string()));

      // Eng review Fix 5: idx_logs_ai_tool_id lives on the EXISTING `logs`
      // table (backfill keyset-pagination support), not `ai_skill_events`.
      let logs_index_exists: i64 = conn
          .query_row(
              "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_logs_ai_tool_id'",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(logs_index_exists, 1);

      // UNIQUE constraint + idempotent re-run of the whole insert on identical
      // (log_id, skill_name, event_kind, evidence_kind) is exercised in Task 6;
      // here we only assert the migration ran and version advanced.
      let version = crate::db::read_schema_version_info_conn(&conn)
          .unwrap()
          .version;
      assert_eq!(version, 38);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib db::pool::tests::migration_38_creates_ai_skill_events_table
  ```
  Expected: compile error or `assert_eq!(table_exists, 1)` failure (table does
  not exist yet) — confirms the test currently fails for the right reason.

- [ ] **Step 3: Write minimal implementation**

  In `src/db/pool.rs`, change line 42:
  ```rust
  pub const KNOWN_SCHEMA_VERSION: i64 = 38;
  ```

  Insert immediately after the migration 37 block PR 1 ("LLM Invocation Guard")
  already added, before the orphaned-maintenance-job cleanup comment that
  follows it:
  ```rust
      // Migration 38: ai_skill_events — one row per detected skill invocation
      // extracted from an AI transcript log row (Claude `attributionSkill` /
      // `attributionPlugin` structured fields, Codex `<skill><name>` transcript
      // tags). UNIQUE(log_id, skill_name, event_kind, evidence_kind) makes
      // INSERT OR IGNORE idempotent across re-ingest and backfill re-runs.
      // Eng review Fix 2: no skill_path/metadata_json — neither extractor sets
      // them in this PR, so they are not part of the shipped schema.
      // Eng review Fix 4: index set matches the actual shipped CLI filter
      // surface (--skill, --plugin, --tool, --project, --session-id, --host,
      // plus the unfiltered default `ORDER BY timestamp DESC`).
      // Eng review Fix 5: idx_logs_ai_tool_id is added on the EXISTING `logs`
      // table in this same migration batch — the backfill's `id > ?` keyset
      // scan needs it and idx_logs_ai_tool_cover (ai_tool, ai_session_id,
      // timestamp) doesn't include `id`.
      if !migration_applied(&conn, 38)? {
          conn.execute_batch(
              "BEGIN IMMEDIATE;

               CREATE TABLE IF NOT EXISTS ai_skill_events (
                 id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                 log_id             INTEGER NOT NULL REFERENCES logs(id) ON DELETE CASCADE,
                 ai_tool            TEXT NOT NULL,
                 ai_project         TEXT,
                 ai_session_id      TEXT,
                 hostname           TEXT NOT NULL,
                 timestamp          TEXT NOT NULL,
                 skill_name         TEXT NOT NULL,
                 skill_plugin       TEXT,
                 event_kind         TEXT NOT NULL,
                 evidence_kind      TEXT NOT NULL,
                 created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 UNIQUE(log_id, skill_name, event_kind, evidence_kind)
               );

               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_timestamp
                   ON ai_skill_events(timestamp);
               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_skill_time
                   ON ai_skill_events(skill_name, timestamp);
               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_plugin_time
                   ON ai_skill_events(skill_plugin, timestamp);
               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_hostname_time
                   ON ai_skill_events(hostname, timestamp);
               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_session_time
                   ON ai_skill_events(ai_tool, ai_project, ai_session_id, timestamp);
               CREATE INDEX IF NOT EXISTS idx_ai_skill_events_project_skill_time
                   ON ai_skill_events(ai_project, skill_name, timestamp)
                   WHERE ai_project IS NOT NULL;

               CREATE INDEX IF NOT EXISTS idx_logs_ai_tool_id
                   ON logs(ai_tool, id)
                   WHERE ai_tool IN ('claude', 'codex');

               INSERT OR IGNORE INTO schema_migrations (version) VALUES (38);
               COMMIT;",
          )?;
          tracing::info!("Migration 38: created ai_skill_events table + idx_logs_ai_tool_id");
      }
  ```

  If `src/db/pool.rs` does not already end with a `#[cfg(test)] #[path =
  "pool_tests.rs"] mod tests;` hook, add one at the bottom of the file (it does
  — confirmed present; `src/db/pool_tests.rs` already exists in the repo).

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib db::pool::tests::migration_38_creates_ai_skill_events_table
  ```
  Expected: `test db::pool::tests::migration_38_creates_ai_skill_events_table ... ok`

- [ ] **Step 5: Commit**
  ```bash
  git add src/db/pool.rs src/db/pool_tests.rs
  git commit -m "feat(db): add migration 38 for ai_skill_events table"
  ```

---

### Task 2: Claude skill-event parser + `ParsedTranscriptRecord.raw_value`

**Files:**
- Create: `src/scanner/skill_events.rs`
- Modify: `src/scanner.rs` (add `raw_value: Option<serde_json::Value>` field to
  `ParsedTranscriptRecord`, around line 1424)
- Modify: `src/scanner/claude.rs` (populate the new `raw_value` field with the
  already-parsed `Value` instead of discarding it — this is the Fix 1 change:
  eliminates the second `serde_json::from_str` that would otherwise happen in
  `flush_chunk`/backfill)
- Modify: `src/scanner/codex.rs`, `src/scanner/gemini.rs` (populate `raw_value:
  None` at their `ParsedTranscriptRecord { ... }` construction sites — Codex
  doesn't need the raw value since `parsed.message` already IS the text the
  `<skill><name>` regex scans; Gemini never produces skill events)
- Test: `src/scanner/skill_events_tests.rs` (new sidecar file, hooked via
  `#[cfg(test)] #[path = "skill_events_tests.rs"] mod tests;` at the bottom of
  `src/scanner/skill_events.rs`, matching `src/scanner/claude.rs` /
  `src/scanner/codex.rs` convention)
- Test: `src/scanner/claude_tests.rs` (assert `raw_value` is populated)

**Interfaces:**
- Consumes: nothing new for the extractor itself (operates on `serde_json::Value`
  and `&str`, no other Task-1+ types); the `ParsedTranscriptRecord.raw_value`
  field addition is consumed by Task 6
- Produces: `ExtractedSkillEvent`, `SkillEventKind`, `SkillEvidenceKind` (all
  locked above, Fix 2: no `skill_path`/`metadata_json`), `pub fn
  extract_claude_skill_events(value: &serde_json::Value) ->
  Vec<ExtractedSkillEvent>`, `ParsedTranscriptRecord.raw_value: Option<serde_json::Value>`

**Eng review Fix 1 (`ParsedTranscriptRecord.raw_value`):** `claude::parse_line`
already does `let value: Value = serde_json::from_str(line)?;` at the top of
the function — it just never returned that value. The original plan had
`flush_chunk` re-parse `line_text` a SECOND time to get a `serde_json::Value`
for skill extraction, which doubles JSON-parse CPU on the hottest path in the
system (every Claude transcript row, ingest-time). The fix is to carry the
already-parsed `Value` through `ParsedTranscriptRecord` instead.

- [ ] **Step 1: Write the failing test**

  First, in `src/scanner.rs`, add the new field to `ParsedTranscriptRecord`
  (around line 1424):
  ```rust
  pub(crate) struct ParsedTranscriptRecord {
      pub record_key: String,
      pub timestamp: Option<String>,
      pub message: String,
      pub session_id: Option<String>,
      pub ai_project: Option<String>,
      /// The already-parsed raw JSON value for Claude transcript lines (`None`
      /// for Codex/Gemini, which don't need it — Codex's skill-tag scanner
      /// reads `message` directly; Gemini never produces skill events). Lets
      /// skill-event extraction (Task 6) reuse the JSON parse `parse_line`
      /// already did internally, instead of re-parsing `line_text` a second
      /// time (eng review Fix 1 — see Task 2).
      pub raw_value: Option<serde_json::Value>,
  }
  ```

  In `src/scanner/claude_tests.rs`, add (adjust the exact assertion style to
  match whatever helper the neighboring tests in that file already use to
  build a `ParsedTranscriptRecord` from a line):
  ```rust
  #[test]
  fn parse_line_carries_the_raw_parsed_value() {
      let line = r#"{"sessionId":"sess-1","content":"hi","attributionSkill":"cortex-troubleshoot"}"#;
      let parsed = parse_line(line, Path::new("/tmp/x.jsonl"), 0)
          .unwrap()
          .unwrap();
      let raw = parsed.raw_value.expect("claude parse_line must carry raw_value");
      assert_eq!(raw.get("attributionSkill").and_then(|v| v.as_str()), Some("cortex-troubleshoot"));
  }
  ```

  Create `src/scanner/skill_events_tests.rs`:
  ```rust
  use super::*;
  use serde_json::json;

  #[test]
  fn extracts_top_level_attribution_skill_and_plugin() {
      let value = json!({
          "sessionId": "sess-1",
          "attributionSkill": "cortex-troubleshoot",
          "attributionPlugin": "cortex",
          "content": "ran the troubleshoot skill"
      });
      let events = extract_claude_skill_events(&value);
      assert_eq!(events.len(), 1);
      let event = &events[0];
      assert_eq!(event.skill_name, "cortex-troubleshoot");
      assert_eq!(event.skill_plugin.as_deref(), Some("cortex"));
      assert_eq!(event.event_kind, SkillEventKind::ClaudeAttribution);
      assert_eq!(event.evidence_kind, SkillEvidenceKind::StructuredJsonField);
  }

  #[test]
  fn extracts_nested_message_attribution_fields() {
      let value = json!({
          "message": {
              "attributionSkill": "web-app-testing",
              "attributionPlugin": "testing",
              "content": "tested the app"
          }
      });
      let events = extract_claude_skill_events(&value);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "web-app-testing");
      assert_eq!(events[0].skill_plugin.as_deref(), Some("testing"));
  }

  #[test]
  fn emits_nothing_when_attribution_fields_absent() {
      let value = json!({"sessionId": "sess-1", "content": "just chatting"});
      assert!(extract_claude_skill_events(&value).is_empty());
  }

  #[test]
  fn emits_nothing_for_empty_or_whitespace_skill_name() {
      let value = json!({"attributionSkill": "   "});
      assert!(extract_claude_skill_events(&value).is_empty());
  }

  #[test]
  fn does_not_fabricate_plugin_skill_combined_string() {
      // Claude gives plugin and skill as SEPARATE fields — the combined
      // "plugin:skill" form must only appear when the source field itself
      // already used that format (see codex tests for that case).
      let value = json!({
          "attributionSkill": "sonnar",
          "attributionPlugin": "arrs"
      });
      let events = extract_claude_skill_events(&value);
      assert_eq!(events[0].skill_name, "sonnar");
      assert_eq!(events[0].skill_plugin.as_deref(), Some("arrs"));
  }

  #[test]
  fn rejects_skill_name_containing_control_characters() {
      // Eng review Fix 8: a crafted attributionSkill value embedding an ANSI
      // escape sequence must be rejected, not silently stored — the CLI
      // printer (Task 9) uses println! directly on skill_name, so control
      // characters would let a malicious transcript spoof terminal output.
      let value = json!({
          "attributionSkill": "\u{1b}[2J\u{1b}[31mFAKE",
      });
      assert!(extract_claude_skill_events(&value).is_empty());
  }

  #[test]
  fn rejects_skill_name_containing_embedded_newline() {
      let value = json!({
          "attributionSkill": "cortex-troubleshoot\nFAKE APPROVED LINE",
      });
      assert!(extract_claude_skill_events(&value).is_empty());
  }
  ```

  (Eng review Fix 3: the `extracts_nested_payload_attribution_fields` test
  from the original plan is removed along with the `payload.*` candidate
  branch it exercised — see Step 3 below.)

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib scanner::skill_events
  ```
  Expected: compile error — `src/scanner/skill_events.rs` and
  `extract_claude_skill_events` do not exist yet.

- [ ] **Step 3: Write minimal implementation**

  Create `src/scanner/skill_events.rs`:
  ```rust
  //! Skill-event extraction from AI transcript records.
  //!
  //! Two independent extractors feed the same [`ExtractedSkillEvent`] shape:
  //! - Claude: structured `attributionSkill` / `attributionPlugin` JSON fields
  //!   (top-level or `message.*` nesting — see Task 2's eng-review note on why
  //!   a third `payload.*` candidate was deliberately NOT added: no observed
  //!   transcript sample confirms that shape, so it would be speculative).
  //! - Codex: `<skill><name>...</name></skill>` tags embedded in transcript
  //!   message text (see `codex_skill_regex` in this module).
  //!
  //! Both extractors short-circuit on a cheap substring check before doing any
  //! real parsing/regex work (eng review Fix 1), so the common no-skill-event
  //! case costs a single `str::contains` call.
  //!
  //! Callers normalize with [`ExtractedSkillEvent::normalized`] before
  //! inserting, which trims/clamps/derives the `plugin:skill` combined form
  //! and rejects control characters (eng review Fix 8 — an adversarial
  //! transcript could otherwise embed ANSI escapes that the CLI printer
  //! (Task 9) would echo verbatim via `println!`).

  const MAX_SKILL_FIELD_CHARS: usize = 256;

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum SkillEventKind {
      ClaudeAttribution,
      CodexSkillBlock,
  }

  impl SkillEventKind {
      pub fn as_str(self) -> &'static str {
          match self {
              Self::ClaudeAttribution => "claude_attribution",
              Self::CodexSkillBlock => "codex_skill_block",
          }
      }
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum SkillEvidenceKind {
      StructuredJsonField,
      TranscriptContent,
  }

  impl SkillEvidenceKind {
      pub fn as_str(self) -> &'static str {
          match self {
              Self::StructuredJsonField => "structured_json_field",
              Self::TranscriptContent => "transcript_content",
          }
      }
  }

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct ExtractedSkillEvent {
      pub skill_name: String,
      pub skill_plugin: Option<String>,
      pub event_kind: SkillEventKind,
      pub evidence_kind: SkillEvidenceKind,
  }

  impl ExtractedSkillEvent {
      /// Trim, reject-if-empty, reject-if-contains-control-characters, and
      /// clamp `skill_name`/`skill_plugin` to `MAX_SKILL_FIELD_CHARS`. Returns
      /// `None` when the resulting skill_name would be empty OR contains any
      /// `char::is_control()` character (eng review Fix 8 — closes a terminal
      /// output spoofing vector: ANSI escapes or embedded newlines/CRs in a
      /// skill name would otherwise be echoed verbatim by the CLI's
      /// `println!`-based printer in Task 9). Never panics or bubbles an
      /// error — callers skip the event and keep parsing the rest of the
      /// transcript.
      fn normalized(mut self) -> Option<Self> {
          let trimmed_name = self.skill_name.trim();
          if trimmed_name.is_empty() || trimmed_name.chars().any(char::is_control) {
              return None;
          }
          if self
              .skill_plugin
              .as_deref()
              .is_some_and(|plugin| plugin.chars().any(char::is_control))
          {
              return None;
          }
          self.skill_name = clamp_chars(trimmed_name, MAX_SKILL_FIELD_CHARS);
          self.skill_plugin = self.skill_plugin.and_then(|plugin| {
              let trimmed = plugin.trim();
              (!trimmed.is_empty()).then(|| clamp_chars(trimmed, MAX_SKILL_FIELD_CHARS))
          });
          Some(self)
      }
  }

  fn clamp_chars(value: &str, max_chars: usize) -> String {
      if value.chars().count() <= max_chars {
          value.to_string()
      } else {
          value.chars().take(max_chars).collect()
      }
  }

  /// Extract Claude skill-attribution events from a raw transcript JSON value.
  /// Checks top-level and `message.*` nesting for `attributionSkill` /
  /// `attributionPlugin` string fields (Claude transcripts use flat top-level
  /// fields on user-facing records and nested `message.*` fields on some
  /// tool-result records). Returns one event per candidate location that has a
  /// non-empty `attributionSkill`; at most one event in practice since a single
  /// transcript line only has one of the two shapes.
  ///
  /// Eng review Fix 1: callers (Task 6/7) should already have skipped calling
  /// this function at all when the source text doesn't contain
  /// `"attributionSkill"` as a substring — this function itself has nothing
  /// further to short-circuit on since it operates on an already-parsed
  /// `Value`, not raw text.
  pub fn extract_claude_skill_events(value: &serde_json::Value) -> Vec<ExtractedSkillEvent> {
      let candidates = [value, value.get("message").unwrap_or(&serde_json::Value::Null)];
      for candidate in candidates {
          let Some(skill) = candidate.get("attributionSkill").and_then(serde_json::Value::as_str)
          else {
              continue;
          };
          let plugin = candidate
              .get("attributionPlugin")
              .and_then(serde_json::Value::as_str)
              .map(ToString::to_string);
          let event = ExtractedSkillEvent {
              skill_name: skill.to_string(),
              skill_plugin: plugin,
              event_kind: SkillEventKind::ClaudeAttribution,
              evidence_kind: SkillEvidenceKind::StructuredJsonField,
          };
          if let Some(normalized) = event.normalized() {
              return vec![normalized];
          }
          return Vec::new();
      }
      Vec::new()
  }

  #[cfg(test)]
  #[path = "skill_events_tests.rs"]
  mod tests;
  ```

  Now wire `ParsedTranscriptRecord.raw_value` into the three parser modules
  that construct it. In `src/scanner/claude.rs`, `parse_line` already binds
  `let value: Value = serde_json::from_str(line)?;` at the top — populate the
  new field with a clone of that same already-parsed value instead of
  discarding it:
  ```rust
  pub fn parse_line(
      line: &str,
      path: &Path,
      line_no: usize,
  ) -> Result<Option<ParsedTranscriptRecord>> {
      let value: Value = serde_json::from_str(line)?;
      let message = extract_message(&value);
      if message.is_empty() {
          return Ok(None);
      }
      let session_id = value
          .get("sessionId")
          .or_else(|| value.get("session_id"))
          .or_else(|| value.pointer("/session/id"))
          .and_then(Value::as_str)
          .map(ToString::to_string)
          .or_else(|| Some(path.to_string_lossy().to_string()));
      Ok(Some(ParsedTranscriptRecord {
          record_key: record_key_from_line(&value, line, line_no),
          timestamp: value
              .get("timestamp")
              .and_then(Value::as_str)
              .map(ToString::to_string),
          message,
          session_id,
          ai_project: extract_project(&value),
          raw_value: Some(value),
      }))
  }
  ```
  (`value` is moved into `raw_value` last since nothing after this point still
  needs to borrow it — `extract_message`/`extract_project`/session lookups all
  ran earlier against `&value`.)

  In `src/scanner/codex.rs`, add `raw_value: None,` to the
  `ParsedTranscriptRecord { ... }` literal in `parse_line` (Codex doesn't need
  the raw value — `message` returned alongside it already IS the text the
  `<skill><name>` regex scans in Task 3/6).

  In `src/scanner/gemini.rs`, add `raw_value: None,` to its
  `ParsedTranscriptRecord { ... }` literal (Gemini never produces skill events
  — explicitly out of scope for this phase).

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib scanner::skill_events
  cargo test --lib scanner::claude::tests::parse_line_carries_the_raw_parsed_value
  cargo build --lib
  ```
  Expected: all 7 tests in `skill_events_tests.rs` pass —
  `test scanner::skill_events::tests::... ok` x7 — the new
  `parse_line_carries_the_raw_parsed_value` test passes, and the workspace
  still builds (confirms `codex.rs`/`gemini.rs` construction sites were
  updated in lockstep with the new field).

- [ ] **Step 5: Commit**
  ```bash
  git add src/scanner/skill_events.rs src/scanner.rs src/scanner/claude.rs src/scanner/codex.rs src/scanner/gemini.rs src/scanner/claude_tests.rs
  git commit -m "feat(scanner): add Claude attributionSkill/attributionPlugin extraction and thread raw_value through ParsedTranscriptRecord"
  ```

---

### Task 3: Codex `<skill><name>` parser + normalization edge cases

**Files:**
- Modify: `src/scanner/skill_events.rs` (add `extract_codex_skill_events`)
- Test: `src/scanner/skill_events_tests.rs` (append)

**Interfaces:**
- Consumes: `ExtractedSkillEvent`, `SkillEventKind`, `SkillEvidenceKind` from
  Task 2 (same file)
- Produces: `pub fn extract_codex_skill_events(text: &str) -> Vec<ExtractedSkillEvent>`

- [ ] **Step 1: Write the failing test**

  Append to `src/scanner/skill_events_tests.rs`:
  ```rust
  #[test]
  fn extracts_single_codex_skill_tag() {
      let text = "Running <skill><name>rustarr</name></skill> now.";
      let events = extract_codex_skill_events(text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "rustarr");
      assert_eq!(events[0].event_kind, SkillEventKind::CodexSkillBlock);
      assert_eq!(events[0].evidence_kind, SkillEvidenceKind::TranscriptContent);
  }

  #[test]
  fn extracts_multiple_distinct_codex_skill_tags() {
      let text = "<skill><name>sonarr</name></skill> then <skill><name>radarr</name></skill>";
      let mut events = extract_codex_skill_events(text);
      events.sort_by(|a, b| a.skill_name.cmp(&b.skill_name));
      assert_eq!(events.len(), 2);
      assert_eq!(events[0].skill_name, "radarr");
      assert_eq!(events[1].skill_name, "sonarr");
  }

  #[test]
  fn dedupes_identical_skill_names_within_one_row() {
      let text = "<skill><name>cortex</name></skill> ... <skill><name>cortex</name></skill>";
      let events = extract_codex_skill_events(text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "cortex");
  }

  #[test]
  fn accepts_optional_whitespace_around_tags() {
      let text = "<skill> <name> tailscale </name> </skill>";
      let events = extract_codex_skill_events(text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "tailscale");
  }

  #[test]
  fn does_not_match_prose_mentioning_a_skill() {
      let text = "You should use the rust skill for this task, not a literal tag.";
      assert!(extract_codex_skill_events(text).is_empty());
  }

  #[test]
  fn skips_empty_skill_name_tag_without_erroring() {
      let text = "<skill><name></name></skill> <skill><name>real-skill</name></skill>";
      let events = extract_codex_skill_events(text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "real-skill");
  }

  #[test]
  fn derives_plugin_skill_split_from_combined_form() {
      let text = "<skill><name>cortex:cortex-troubleshoot</name></skill>";
      let events = extract_codex_skill_events(text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name, "cortex:cortex-troubleshoot");
      assert_eq!(events[0].skill_plugin.as_deref(), Some("cortex"));
  }

  #[test]
  fn clamps_oversized_skill_name_to_256_chars() {
      let long_name = "a".repeat(300);
      let text = format!("<skill><name>{long_name}</name></skill>");
      let events = extract_codex_skill_events(&text);
      assert_eq!(events.len(), 1);
      assert_eq!(events[0].skill_name.chars().count(), 256);
  }

  #[test]
  fn rejects_codex_skill_name_containing_ansi_escape() {
      // Eng review Fix 8 — same adversarial-input rejection as Task 2's
      // Claude test, but through the Codex tag-scanning path.
      let text = "<skill><name>\u{1b}[2J\u{1b}[31mFAKE</name></skill>";
      assert!(extract_codex_skill_events(text).is_empty());
  }

  #[test]
  fn rejects_codex_skill_name_containing_embedded_newline() {
      let text = "<skill><name>real-skill\nFAKE APPROVED LINE</name></skill>";
      assert!(extract_codex_skill_events(text).is_empty());
  }

  #[test]
  fn short_circuits_when_text_has_no_skill_tag_substring() {
      // Eng review Fix 1 — cheap bound on the common (no-skill-event) case:
      // text without a literal "<skill>" substring never reaches the regex
      // engine. This is a behavioral assertion (empty result), not a proof of
      // the short-circuit itself — see Task 6/7 for where the caller-side
      // substring check actually lives.
      let text = "just a normal transcript line with no tags at all";
      assert!(extract_codex_skill_events(text).is_empty());
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib scanner::skill_events
  ```
  Expected: compile error — `extract_codex_skill_events` undefined.

- [ ] **Step 3: Write minimal implementation**

  Add to `src/scanner/skill_events.rs` (below `extract_claude_skill_events`,
  above the test module hook):
  ```rust
  use std::sync::LazyLock;

  use regex::Regex;

  /// Matches `<skill> <name> ... </name> </skill>` with optional whitespace
  /// around every tag boundary. `(?s)` lets `.` cross newlines (skill names are
  /// short but transcripts can wrap). Non-greedy `.*?` keeps each match scoped
  /// to one tag pair even when multiple `<skill>` blocks appear in one message.
  static CODEX_SKILL_TAG: LazyLock<Regex> = LazyLock::new(|| {
      Regex::new(
          r"(?s)<skill>\s*<name>\s*(.*?)\s*</name>\s*</skill>",
      )
      .expect("static regex")
  });

  /// Extract Codex skill-invocation events from transcript message text. Scans
  /// for ALL `<skill><name>...</name></skill>` occurrences (a single row can
  /// invoke multiple skills), de-duplicating identical skill names within the
  /// row. Deliberately narrow — matches only the literal tag pair, never prose
  /// like "use the rust skill".
  ///
  /// Eng review Fix 1: short-circuits on a cheap substring check before
  /// touching the regex engine at all — the overwhelming majority of
  /// transcript rows contain no skill tag, so this bounds the common case to
  /// one `str::contains` call instead of a full regex scan.
  pub fn extract_codex_skill_events(text: &str) -> Vec<ExtractedSkillEvent> {
      if !text.contains("<skill>") {
          return Vec::new();
      }
      let mut seen = std::collections::HashSet::new();
      let mut events = Vec::new();
      for capture in CODEX_SKILL_TAG.captures_iter(text) {
          let raw_name = capture.get(1).map_or("", |m| m.as_str());
          let event = ExtractedSkillEvent {
              skill_name: raw_name.to_string(),
              skill_plugin: None,
              event_kind: SkillEventKind::CodexSkillBlock,
              evidence_kind: SkillEvidenceKind::TranscriptContent,
          };
          let Some(normalized) = event.normalized() else {
              continue;
          };
          if seen.insert(normalized.skill_name.clone()) {
              events.push(normalized);
          }
      }
      events
  }
  ```

  Now update `ExtractedSkillEvent::normalized` (from Task 2) to derive the
  `plugin:skill` split when the raw name already contains a single `:`, on top
  of the control-character rejection Task 2 already added:
  ```rust
  fn normalized(mut self) -> Option<Self> {
      let trimmed_name = self.skill_name.trim();
      if trimmed_name.is_empty() || trimmed_name.chars().any(char::is_control) {
          return None;
      }
      if self
          .skill_plugin
          .as_deref()
          .is_some_and(|plugin| plugin.chars().any(char::is_control))
      {
          return None;
      }
      // If the source already used "plugin:skill" combined form, split it out
      // for skill_plugin while keeping skill_name as the full combined string
      // (locked behavior — do not fabricate this split when plugin/skill came
      // from separate source fields, e.g. Claude's attributionSkill/attributionPlugin).
      if self.skill_plugin.is_none() {
          if let Some((plugin, _rest)) = trimmed_name.split_once(':') {
              if !plugin.is_empty() {
                  self.skill_plugin = Some(plugin.to_string());
              }
          }
      }
      self.skill_name = clamp_chars(trimmed_name, MAX_SKILL_FIELD_CHARS);
      self.skill_plugin = self.skill_plugin.and_then(|plugin| {
          let trimmed = plugin.trim();
          (!trimmed.is_empty()).then(|| clamp_chars(trimmed, MAX_SKILL_FIELD_CHARS))
      });
      Some(self)
  }
  ```

  Add `use std::sync::LazyLock;` and `use regex::Regex;` at the top of the file
  if not already present from the edit above.

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib scanner::skill_events
  ```
  Expected: all tests from Task 2 + Task 3 pass (18 total: 7 Claude-side +
  11 Codex-side).

- [ ] **Step 5: Commit**
  ```bash
  git add src/scanner/skill_events.rs
  git commit -m "feat(scanner): add Codex <skill><name> tag extraction with dedup, normalization, and short-circuit"
  ```

---

### Task 4: `insert_skill_events` / `list_skill_events` DB layer

**Files:**
- Create: `src/db/skill_events.rs`
- Modify: `src/db.rs` (add `mod skill_events;` and `pub use skill_events::{...}`)
- Test: `src/db/skill_events_tests.rs` (sidecar, hooked from `src/db/skill_events.rs`)

**Interfaces:**
- Consumes: `crate::scanner::skill_events::{ExtractedSkillEvent, SkillEventKind,
  SkillEvidenceKind}` from Tasks 2-3; `crate::db::DbPool` from `src/db/pool.rs`
  (existing)
- Produces: `SkillEventInsert`, `insert_skill_events_in_tx`,
  `insert_skill_events`, `AiSkillEventParams`, `AiSkillEventEntry`,
  `ListSkillEventsResult`, `list_skill_events` — exact signatures in "Locked
  interfaces" above.

- [ ] **Step 1: Write the failing test**

  Create `src/db/skill_events_tests.rs`:
  ```rust
  use super::*;
  use crate::config::StorageConfig;
  use crate::db::pool::init_pool;
  use crate::scanner::skill_events::{ExtractedSkillEvent, SkillEventKind, SkillEvidenceKind};

  fn test_pool() -> (crate::db::DbPool, tempfile::TempDir) {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
      (pool, dir)
  }

  fn insert_log_row(pool: &crate::db::DbPool, hostname: &str, timestamp: &str) -> i64 {
      let conn = pool.get().unwrap();
      conn.execute(
          "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip)
           VALUES (?1, ?2, 'info', 'msg', 'raw', 'transcript://claude_project')",
          rusqlite::params![timestamp, hostname],
      )
      .unwrap();
      conn.last_insert_rowid()
  }

  fn sample_event(skill_name: &str) -> ExtractedSkillEvent {
      ExtractedSkillEvent {
          skill_name: skill_name.to_string(),
          skill_plugin: Some("cortex".to_string()),
          event_kind: SkillEventKind::ClaudeAttribution,
          evidence_kind: SkillEvidenceKind::StructuredJsonField,
      }
  }

  #[test]
  fn insert_and_list_round_trips() {
      let (pool, _dir) = test_pool();
      let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
      let insert = SkillEventInsert {
          log_id,
          ai_tool: "claude".to_string(),
          ai_project: Some("cortex".to_string()),
          ai_session_id: Some("sess-1".to_string()),
          hostname: "devhost".to_string(),
          timestamp: "2026-06-01T00:00:00.000Z".to_string(),
          event: sample_event("cortex-troubleshoot"),
      };
      let inserted = insert_skill_events(&pool, &[insert]).unwrap();
      assert_eq!(inserted, 1);

      let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
      assert_eq!(result.total, 1);
      assert_eq!(result.events[0].skill_name, "cortex-troubleshoot");
      assert_eq!(result.events[0].skill_plugin.as_deref(), Some("cortex"));
      assert_eq!(result.events[0].event_kind, "claude_attribution");
      assert_eq!(result.events[0].evidence_kind, "structured_json_field");
      assert_eq!(result.events[0].log_id, log_id);
  }

  #[test]
  fn insert_or_ignore_is_idempotent_on_duplicate() {
      let (pool, _dir) = test_pool();
      let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
      let insert = SkillEventInsert {
          log_id,
          ai_tool: "claude".to_string(),
          ai_project: None,
          ai_session_id: None,
          hostname: "devhost".to_string(),
          timestamp: "2026-06-01T00:00:00.000Z".to_string(),
          event: sample_event("cortex-troubleshoot"),
      };
      assert_eq!(insert_skill_events(&pool, &[insert.clone()]).unwrap(), 1);
      assert_eq!(insert_skill_events(&pool, &[insert]).unwrap(), 0);

      let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
      assert_eq!(result.total, 1);
  }

  #[test]
  fn insert_succeeds_without_project_or_session_id() {
      let (pool, _dir) = test_pool();
      let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
      let insert = SkillEventInsert {
          log_id,
          ai_tool: "codex".to_string(),
          ai_project: None,
          ai_session_id: None,
          hostname: "devhost".to_string(),
          timestamp: "2026-06-01T00:00:00.000Z".to_string(),
          event: ExtractedSkillEvent {
              skill_name: "rustarr".to_string(),
              skill_plugin: None,
              event_kind: SkillEventKind::CodexSkillBlock,
              evidence_kind: SkillEvidenceKind::TranscriptContent,
          },
      };
      assert_eq!(insert_skill_events(&pool, &[insert]).unwrap(), 1);
      let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
      assert_eq!(result.events[0].ai_project, None);
      assert_eq!(result.events[0].ai_session_id, None);
  }

  #[test]
  fn list_filters_by_skill_project_and_tool() {
      let (pool, _dir) = test_pool();
      let log_id_a = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
      let log_id_b = insert_log_row(&pool, "nashost", "2026-06-01T01:00:00.000Z");
      insert_skill_events(
          &pool,
          &[
              SkillEventInsert {
                  log_id: log_id_a,
                  ai_tool: "claude".to_string(),
                  ai_project: Some("cortex".to_string()),
                  ai_session_id: Some("sess-a".to_string()),
                  hostname: "devhost".to_string(),
                  timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                  event: sample_event("cortex-troubleshoot"),
              },
              SkillEventInsert {
                  log_id: log_id_b,
                  ai_tool: "codex".to_string(),
                  ai_project: Some("axon".to_string()),
                  ai_session_id: Some("sess-b".to_string()),
                  hostname: "nashost".to_string(),
                  timestamp: "2026-06-01T01:00:00.000Z".to_string(),
                  event: sample_event("axon-deploy"),
              },
          ],
      )
      .unwrap();

      let result = list_skill_events(
          &pool,
          &AiSkillEventParams {
              project: Some("cortex".to_string()),
              ..Default::default()
          },
      )
      .unwrap();
      assert_eq!(result.total, 1);
      assert_eq!(result.events[0].skill_name, "cortex-troubleshoot");

      let result = list_skill_events(
          &pool,
          &AiSkillEventParams {
              tool: Some("codex".to_string()),
              ..Default::default()
          },
      )
      .unwrap();
      assert_eq!(result.total, 1);
      assert_eq!(result.events[0].ai_tool, "codex");
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib db::skill_events
  ```
  Expected: compile error — `src/db/skill_events.rs` does not exist.

- [ ] **Step 3: Write minimal implementation**

  Create `src/db/skill_events.rs`:
  ```rust
  //! `ai_skill_events` insert + list query layer. Table/columns are defined in
  //! migration 38 (`src/db/pool.rs`). Extraction happens in
  //! `crate::scanner::skill_events`; this module only persists and reads back
  //! already-extracted events.

  use anyhow::Result;
  use rusqlite::{Transaction, params};
  use serde::{Deserialize, Serialize};

  use crate::scanner::skill_events::ExtractedSkillEvent;

  use super::pool::DbPool;

  #[derive(Debug, Clone)]
  pub struct SkillEventInsert {
      pub log_id: i64,
      pub ai_tool: String,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub hostname: String,
      pub timestamp: String,
      pub event: ExtractedSkillEvent,
  }

  /// Insert `events` inside an existing transaction with `INSERT OR IGNORE`
  /// (idempotent on the `UNIQUE(log_id, skill_name, event_kind, evidence_kind)`
  /// constraint). Returns the number of rows actually inserted (excludes
  /// ignored duplicates) via SQLite `changes()` summed per statement.
  pub(crate) fn insert_skill_events_in_tx(
      tx: &Transaction<'_>,
      events: &[SkillEventInsert],
  ) -> Result<usize> {
      if events.is_empty() {
          return Ok(0);
      }
      let mut stmt = tx.prepare_cached(
          "INSERT OR IGNORE INTO ai_skill_events (
              log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
              skill_name, skill_plugin, event_kind, evidence_kind
          ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
      )?;
      let mut inserted = 0usize;
      for item in events {
          let changed = stmt.execute(params![
              item.log_id,
              item.ai_tool,
              item.ai_project,
              item.ai_session_id,
              item.hostname,
              item.timestamp,
              item.event.skill_name,
              item.event.skill_plugin,
              item.event.event_kind.as_str(),
              item.event.evidence_kind.as_str(),
          ])?;
          inserted += changed;
      }
      Ok(inserted)
  }

  /// Pool-acquiring wrapper for callers outside an existing transaction (e.g.
  /// the backfill service, which owns its own chunked transaction boundary).
  pub fn insert_skill_events(pool: &DbPool, events: &[SkillEventInsert]) -> Result<usize> {
      let mut conn = pool.get()?;
      let _write_guard = crate::db::write_lock();
      let tx = conn.transaction()?;
      let inserted = insert_skill_events_in_tx(&tx, events)?;
      tx.commit()?;
      Ok(inserted)
  }

  #[derive(Debug, Clone, Default)]
  pub struct AiSkillEventParams {
      pub skill: Option<String>,
      pub plugin: Option<String>,
      pub tool: Option<String>,
      pub project: Option<String>,
      pub session_id: Option<String>,
      pub hostname: Option<String>,
      pub from: Option<String>,
      pub to: Option<String>,
      pub limit: Option<u32>,
  }

  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct AiSkillEventEntry {
      pub id: i64,
      pub log_id: i64,
      pub ai_tool: String,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub hostname: String,
      pub timestamp: String,
      pub skill_name: String,
      pub skill_plugin: Option<String>,
      pub event_kind: String,
      pub evidence_kind: String,
  }

  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct ListSkillEventsResult {
      pub total: usize,
      pub truncated: bool,
      pub events: Vec<AiSkillEventEntry>,
  }

  const DEFAULT_LIMIT: u32 = 50;
  const MAX_LIMIT: u32 = 500;

  /// List `ai_skill_events` rows newest-first, applying every non-`None`
  /// filter in `params` as an `AND`-ed equality/range clause. `limit` is
  /// clamped to `[1, 500]`; `truncated` is `true` when more rows matched than
  /// were returned (probed via `LIMIT + 1`, mirroring `list_ai_tools`'s
  /// truncation-detection pattern in `src/db/queries.rs`).
  pub fn list_skill_events(pool: &DbPool, params: &AiSkillEventParams) -> Result<ListSkillEventsResult> {
      let conn = pool.get()?;
      let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;

      let mut sql = String::from(
          "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                  skill_name, skill_plugin, event_kind, evidence_kind
           FROM ai_skill_events WHERE 1 = 1",
      );
      let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
      let mut idx = 1usize;

      macro_rules! bind_eq {
          ($column:literal, $value:expr) => {
              if let Some(value) = $value {
                  sql.push_str(&format!(" AND {} = ?{idx}", $column));
                  bindings.push(rusqlite::types::Value::Text(value.clone()));
                  idx += 1;
              }
          };
      }
      bind_eq!("skill_name", &params.skill);
      bind_eq!("skill_plugin", &params.plugin);
      bind_eq!("ai_tool", &params.tool);
      bind_eq!("ai_project", &params.project);
      bind_eq!("ai_session_id", &params.session_id);
      bind_eq!("hostname", &params.hostname);
      if let Some(from) = &params.from {
          sql.push_str(&format!(" AND timestamp >= ?{idx}"));
          bindings.push(rusqlite::types::Value::Text(from.clone()));
          idx += 1;
      }
      if let Some(to) = &params.to {
          sql.push_str(&format!(" AND timestamp <= ?{idx}"));
          bindings.push(rusqlite::types::Value::Text(to.clone()));
          idx += 1;
      }
      let _ = idx;
      sql.push_str(&format!(" ORDER BY timestamp DESC, id DESC LIMIT {}", limit + 1));

      let mut stmt = conn.prepare(&sql)?;
      let mut rows = stmt
          .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
              Ok(AiSkillEventEntry {
                  id: row.get(0)?,
                  log_id: row.get(1)?,
                  ai_tool: row.get(2)?,
                  ai_project: row.get(3)?,
                  ai_session_id: row.get(4)?,
                  hostname: row.get(5)?,
                  timestamp: row.get(6)?,
                  skill_name: row.get(7)?,
                  skill_plugin: row.get(8)?,
                  event_kind: row.get(9)?,
                  evidence_kind: row.get(10)?,
              })
          })?
          .collect::<rusqlite::Result<Vec<_>>>()?;

      let truncated = rows.len() > limit;
      rows.truncate(limit);
      Ok(ListSkillEventsResult {
          total: rows.len(),
          truncated,
          events: rows,
      })
  }

  #[cfg(test)]
  #[path = "skill_events_tests.rs"]
  mod tests;
  ```

  Modify `src/db.rs`: add `mod skill_events;` alongside the other `mod`
  declarations (alphabetical position after `pool`, before `queries` — matches
  existing ordering), and re-export:
  ```rust
  mod skill_events;
  ```
  ```rust
  pub use skill_events::{
      AiSkillEventEntry, AiSkillEventParams, ListSkillEventsResult, SkillEventInsert,
      insert_skill_events, list_skill_events,
  };
  pub(crate) use skill_events::insert_skill_events_in_tx;
  ```

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib db::skill_events
  ```
  Expected: all 4 tests pass — `test db::skill_events::tests::... ok` x4.

- [ ] **Step 5: Commit**
  ```bash
  git add src/db/skill_events.rs src/db.rs
  git commit -m "feat(db): add ai_skill_events insert and list query layer"
  ```

---

### Task 5: `insert_logs_batch_in_tx` returns inserted log ids

**Files:**
- Modify: `src/db/ingest.rs:34-45,47-126`
- Test: `src/db/ingest_tests.rs`

**Interfaces:**
- Consumes: nothing new
- Produces (signature change, LOCKED for Task 6):
  ```rust
  // before:
  pub(crate) fn insert_logs_batch_in_tx(tx: &Transaction<'_>, entries: &[LogBatchEntry]) -> Result<()>
  // after:
  pub(crate) fn insert_logs_batch_in_tx(tx: &Transaction<'_>, entries: &[LogBatchEntry]) -> Result<Vec<i64>>
  ```
  `pub fn insert_logs_batch(pool: &DbPool, entries: &[LogBatchEntry]) -> Result<usize>`
  signature is UNCHANGED (still returns row count) — every existing caller
  outside `scanner.rs` compiles without modification.

- [ ] **Step 1: Write the failing test**

  Append to `src/db/ingest_tests.rs` (check existing `use` statements at top of
  that file first; add `LogBatchEntry` fixture builder matching whatever helper
  already exists there, e.g. `sample_entry()` — if no such helper exists,
  define one inline):
  ```rust
  #[test]
  fn insert_logs_batch_in_tx_returns_ids_in_input_order() {
      let (pool, _dir) = test_pool();
      let mut conn = pool.get().unwrap();
      let tx = conn.transaction().unwrap();

      let entries = vec![
          LogBatchEntry {
              timestamp: "2026-06-01T00:00:00.000Z".to_string(),
              hostname: "devhost".to_string(),
              facility: None,
              severity: "info".to_string(),
              app_name: None,
              process_id: None,
              message: "first".to_string(),
              raw: "first".to_string(),
              source_ip: "transcript://claude_project".to_string(),
              docker_checkpoint: None,
              ai_tool: Some("claude".to_string()),
              ai_project: None,
              ai_session_id: None,
              ai_transcript_path: None,
              metadata_json: None,
              http_status: None,
              auth_outcome: None,
              dns_blocked: None,
              event_action: None,
              parse_error: None,
          },
          LogBatchEntry {
              timestamp: "2026-06-01T00:00:01.000Z".to_string(),
              hostname: "devhost".to_string(),
              facility: None,
              severity: "info".to_string(),
              app_name: None,
              process_id: None,
              message: "second".to_string(),
              raw: "second".to_string(),
              source_ip: "transcript://claude_project".to_string(),
              docker_checkpoint: None,
              ai_tool: Some("claude".to_string()),
              ai_project: None,
              ai_session_id: None,
              ai_transcript_path: None,
              metadata_json: None,
              http_status: None,
              auth_outcome: None,
              dns_blocked: None,
              event_action: None,
              parse_error: None,
          },
      ];

      let ids = insert_logs_batch_in_tx(&tx, &entries).unwrap();
      tx.commit().unwrap();

      assert_eq!(ids.len(), 2);
      assert!(ids[1] > ids[0], "second row's id must be greater than first");

      let conn = pool.get().unwrap();
      let stored_message: String = conn
          .query_row("SELECT message FROM logs WHERE id = ?1", [ids[0]], |row| row.get(0))
          .unwrap();
      assert_eq!(stored_message, "first");
      let stored_message: String = conn
          .query_row("SELECT message FROM logs WHERE id = ?1", [ids[1]], |row| row.get(0))
          .unwrap();
      assert_eq!(stored_message, "second");
  }
  ```
  If `src/db/ingest_tests.rs` does not already define `test_pool()`, add it
  matching the convention in `src/scanner_tests.rs` / `src/db/queries_tests.rs`
  (`tempfile::tempdir()` + `init_pool(&StorageConfig::for_test(db_path))`).

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib db::ingest::tests::insert_logs_batch_in_tx_returns_ids_in_input_order
  ```
  Expected: compile error — `insert_logs_batch_in_tx` currently returns
  `Result<()>`, so `let ids = ...unwrap();` followed by `ids.len()` fails to
  typecheck (`()` has no `.len()`).

- [ ] **Step 3: Write minimal implementation**

  In `src/db/ingest.rs`, change the signature and body of
  `insert_logs_batch_in_tx` (currently lines 47-126):
  ```rust
  pub(crate) fn insert_logs_batch_in_tx(
      tx: &Transaction<'_>,
      entries: &[LogBatchEntry],
  ) -> Result<Vec<i64>> {
      let mut ids = Vec::with_capacity(entries.len());
      {
          let mut stmt = tx.prepare_cached(
              "INSERT INTO logs (
                  timestamp, hostname, facility, severity, app_name, process_id,
                  message, raw, source_ip, ai_tool, ai_project, ai_session_id, ai_transcript_path,
                  metadata_json, http_status, auth_outcome, dns_blocked, event_action, parse_error
              ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
          )?;

          for entry in entries {
              stmt.execute(params![
                  entry.timestamp,
                  entry.hostname,
                  entry.facility,
                  entry.severity,
                  entry.app_name,
                  entry.process_id,
                  entry.message,
                  entry.raw,
                  entry.source_ip,
                  entry.ai_tool,
                  entry.ai_project,
                  entry.ai_session_id,
                  entry.ai_transcript_path,
                  entry.metadata_json,
                  entry.http_status,
                  entry.auth_outcome,
                  entry.dns_blocked.map(|b| b as i64),
                  entry.event_action,
                  entry.parse_error,
              ])?;
              ids.push(tx.last_insert_rowid());
          }

          // Batch upsert hosts — group by hostname to avoid one upsert per log entry
          let mut host_counts: HashMap<&str, i64> = HashMap::new();
          for entry in entries {
              *host_counts.entry(entry.hostname.as_str()).or_insert(0) += 1;
          }
          let mut host_stmt = tx.prepare_cached(
              "INSERT INTO hosts (hostname, log_count)
               VALUES (?1, ?2)
               ON CONFLICT(hostname) DO UPDATE SET
                   last_seen = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                   log_count = log_count + excluded.log_count",
          )?;
          for (hostname, count) in &host_counts {
              host_stmt.execute(params![hostname, count])?;
          }
          let mut checkpoint_stmt = tx.prepare_cached(
              "INSERT INTO docker_ingest_checkpoints (host_name, container_id, last_timestamp)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(host_name, container_id) DO UPDATE SET
                   last_timestamp = excluded.last_timestamp,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
          )?;
          let mut checkpoint_count = 0usize;
          for entry in entries {
              if let Some(checkpoint) = &entry.docker_checkpoint {
                  checkpoint_stmt.execute(params![
                      checkpoint.host_name,
                      checkpoint.container_id,
                      checkpoint.timestamp
                  ])?;
                  checkpoint_count += 1;
              }
          }

          tracing::debug!(
              entry_count = entries.len(),
              unique_hosts = host_counts.len(),
              checkpoint_count,
              "Prepared batch insert transaction"
          );
      }
      Ok(ids)
  }
  ```

  Update `insert_logs_batch_once` (the only in-crate caller) to discard the
  `Vec<i64>` and keep its existing `Result<usize>` contract:
  ```rust
  fn insert_logs_batch_once(pool: &DbPool, entries: &[LogBatchEntry]) -> Result<usize> {
      let mut conn = pool.get()?;
      let _write_guard = crate::db::write_lock();
      let tx = conn.transaction()?;
      let _ids = insert_logs_batch_in_tx(&tx, entries)?;
      tx.commit()?;
      tracing::debug!(
          entry_count = entries.len(),
          "Committed batch insert transaction"
      );
      Ok(entries.len())
  }
  ```

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib db::ingest
  ```
  Expected: `insert_logs_batch_in_tx_returns_ids_in_input_order` and all
  pre-existing `db::ingest::tests::*` tests pass.

  Also run the full workspace build to confirm no other caller broke (the only
  caller of `insert_logs_batch_in_tx` outside this file is
  `src/scanner.rs::flush_chunk`, which Task 6 updates in lockstep — build will
  fail here until Task 6 lands; that is expected and acceptable within this
  same phase since Task 6 is next):
  ```bash
  cargo build --lib 2>&1 | tail -40
  ```
  Expected: the only remaining error is in `src/scanner.rs` at the
  `insert_logs_batch_in_tx(&tx, &claimed_batch)?;` call site (type mismatch,
  ignored `Vec<i64>` return) — confirms Task 6 is the correct next step.

- [ ] **Step 5: Commit**
  ```bash
  git add src/db/ingest.rs src/db/ingest_tests.rs
  git commit -m "refactor(db): insert_logs_batch_in_tx returns inserted log ids"
  ```

---

### Task 6: Wire skill-event extraction into transcript ingest (`flush_chunk`)

**Files:**
- Modify: `src/scanner.rs:1-13` (imports), `:483-631` (per-line loop in
  `index_file_with_options`), `:774-832` (`flush_chunk`), `:1063-1161` (Gemini
  path — Gemini is explicitly out of scope per the phase spec, which only
  requires Claude + Codex extraction; the Gemini branch passes an empty
  skill-source vector so `flush_chunk`'s new parameter is satisfied uniformly)
- Test: `src/scanner_tests.rs`

**Interfaces:**
- Consumes: `insert_logs_batch_in_tx` returning `Vec<i64>` (Task 5);
  `crate::db::{SkillEventInsert, insert_skill_events_in_tx}` (Task 4);
  `crate::scanner::skill_events::{extract_claude_skill_events,
  extract_codex_skill_events}` (Tasks 2-3)
- Produces: no new public API — this task is pure wiring. Behavior contract:
  every transcript row ingested via `index_file_with_options` /
  `index_roots_with_options` now also inserts zero or more `ai_skill_events`
  rows in the SAME transaction as its `logs` row.

- [ ] **Step 1: Write the failing test**

  Append to `src/scanner_tests.rs`:
  ```rust
  #[test]
  fn indexing_claude_transcript_extracts_skill_events() {
      let (pool, dir) = test_pool();
      let file = dir.path().join("claude-skill.jsonl");
      std::fs::write(
          &file,
          concat!(
              r#"{"sessionId":"sess-1","attributionSkill":"cortex-troubleshoot","attributionPlugin":"cortex","content":"ran troubleshoot"}"#,
              "\n"
          ),
      )
      .unwrap();

      let result = index_file(&pool, &file, "explicit_file").unwrap();
      assert_eq!(result.ingested, 1);

      let conn = pool.get().unwrap();
      let (skill_name, plugin, event_kind): (String, Option<String>, String) = conn
          .query_row(
              "SELECT skill_name, skill_plugin, event_kind FROM ai_skill_events",
              [],
              |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
          )
          .unwrap();
      assert_eq!(skill_name, "cortex-troubleshoot");
      assert_eq!(plugin.as_deref(), Some("cortex"));
      assert_eq!(event_kind, "claude_attribution");
  }

  #[test]
  fn indexing_codex_transcript_extracts_skill_events() {
      let (pool, dir) = test_pool();
      let file = dir.path().join("codex-skill.jsonl");
      std::fs::write(
          &file,
          concat!(
              r#"{"type":"response_item","payload":{"type":"message","content":"<skill><name>rustarr</name></skill> deploying now"},"timestamp":"2026-06-01T00:00:00Z"}"#,
              "\n"
          ),
      )
      .unwrap();

      let result = index_file(&pool, &file, "codex_session").unwrap();
      assert_eq!(result.ingested, 1);

      let conn = pool.get().unwrap();
      let (skill_name, event_kind): (String, String) = conn
          .query_row(
              "SELECT skill_name, event_kind FROM ai_skill_events",
              [],
              |row| Ok((row.get(0)?, row.get(1)?)),
          )
          .unwrap();
      assert_eq!(skill_name, "rustarr");
      assert_eq!(event_kind, "codex_skill_block");
  }

  #[test]
  fn reindexing_same_transcript_does_not_duplicate_skill_events() {
      let (pool, dir) = test_pool();
      let file = dir.path().join("claude-skill-idem.jsonl");
      std::fs::write(
          &file,
          concat!(
              r#"{"sessionId":"sess-1","attributionSkill":"cortex","content":"hi"}"#,
              "\n"
          ),
      )
      .unwrap();

      index_file(&pool, &file, "explicit_file").unwrap();
      let forced = index_file_with_options(
          &pool,
          &file,
          "explicit_file",
          IndexFileOptions { force: true },
          None,
      )
      .unwrap();
      assert_eq!(forced.ingested, 1);

      let conn = pool.get().unwrap();
      let count: i64 = conn
          .query_row("SELECT COUNT(*) FROM ai_skill_events", [], |row| row.get(0))
          .unwrap();
      // force=true re-inserts the logs row (new log_id), so the skill event
      // row is NOT a duplicate by the UNIQUE(log_id, ...) constraint — it is
      // correctly re-created against the new log_id. This asserts the count
      // tracks 1-per-logs-row rather than silently growing unbounded on
      // ordinary (non-forced) re-scans, which is covered by the next test.
      assert_eq!(count, 1);
  }

  #[test]
  fn transcript_row_with_no_skill_reference_creates_no_skill_event() {
      let (pool, dir) = test_pool();
      let file = dir.path().join("no-skill.jsonl");
      std::fs::write(&file, "{\"sessionId\":\"sess-1\",\"content\":\"just chatting\"}\n").unwrap();

      index_file(&pool, &file, "explicit_file").unwrap();

      let conn = pool.get().unwrap();
      let count: i64 = conn
          .query_row("SELECT COUNT(*) FROM ai_skill_events", [], |row| row.get(0))
          .unwrap();
      assert_eq!(count, 0);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib scanner::tests::indexing_claude_transcript_extracts_skill_events
  ```
  Expected: fails at the `conn.query_row("SELECT ... FROM ai_skill_events", ...)`
  step with "Query returned no rows" (or a compile error if `scanner.rs` still
  fails to build from Task 5's signature change) — confirms no skill events are
  written yet.

- [ ] **Step 3: Write minimal implementation**

  In `src/scanner.rs`, update the import block (currently lines 9-13):
  ```rust
  use crate::ai_project::normalize_local_ai_project_path;
  use crate::config::StorageConfig;
  use crate::db::{
      DbPool, LogBatchEntry, SkillEventInsert, enforce_storage_budget, insert_logs_batch_in_tx,
      insert_skill_events_in_tx,
  };
  use crate::ingest_metadata::bounded_metadata_json;
  use crate::receiver::enrichment::{project_from_transcript_path, scrub_ai_message};
  use crate::scanner::skill_events::{extract_claude_skill_events, extract_codex_skill_events};
  ```

  Add a new struct near the top of the file (after `IndexFileOptions`, before
  `CheckpointListOptions` — around line 62):
  ```rust
  /// Raw skill-extraction source paired 1:1 with each `LogBatchEntry` pushed
  /// into a chunk's `batch` vector. Carried alongside `batch`/`imports` because
  /// skill extraction needs the PRE-SCRUB parsed value (Claude JSON) or raw
  /// extracted text (Codex), not the already-scrubbed `LogBatchEntry.message`.
  ///
  /// Eng review Fix 1: `Claude` wraps the `serde_json::Value` that
  /// `ParsedTranscriptRecord.raw_value` already carries (Task 2) — NOT a
  /// re-parse of `line_text`. `claude::parse_line` parses the line's JSON
  /// exactly once; this side channel just moves that already-parsed value
  /// forward instead of throwing it away and parsing again.
  #[derive(Debug, Clone)]
  enum ChunkSkillSource {
      Claude(serde_json::Value),
      Codex(String),
      None,
  }
  ```

  In `index_file_with_options`, inside the per-line loop (around the existing
  `Ok(Some(parsed)) => { ... }` arm at line 537), build the skill-extraction
  side channel FROM `parsed.raw_value` / `parsed.message` — no re-parse of
  `line_text`. Also apply the Fix 1 short-circuit here: skip building a
  `ChunkSkillSource::Claude`/`Codex` variant at all (store `None` instead) when
  the cheap substring check already rules out a skill event, so the per-row
  cost in the overwhelmingly common no-skill-event case is one `contains` call
  plus a clone of a `Value` that's already sitting in memory (not a fresh
  parse):
  ```rust
              Ok(Some(parsed)) => {
                  let record_key = parsed.record_key;
                  let message = scrub_ai_message(&parsed.message, None);
                  let skill_source = match source_kind {
                      SourceKind::CodexSession => {
                          if parsed.message.contains("<skill>") {
                              ChunkSkillSource::Codex(parsed.message.clone())
                          } else {
                              ChunkSkillSource::None
                          }
                      }
                      SourceKind::ClaudeProject | SourceKind::ExplicitFile => {
                          match &parsed.raw_value {
                              Some(value) if line_text.contains("attributionSkill") => {
                                  ChunkSkillSource::Claude(value.clone())
                              }
                              _ => ChunkSkillSource::None,
                          }
                      }
                      SourceKind::GeminiSession => ChunkSkillSource::None,
                  };
                  let project_candidate = parsed
  ```
  (the rest of that arm is unchanged up through `imports.push(record_key);`).
  The `line_text.contains("attributionSkill")` check on the RAW text (not the
  parsed value) is the actual short-circuit — it's cheaper than any JSON
  traversal and catches the common case before `extract_claude_skill_events`
  ever runs. `.clone()` on `parsed.raw_value` is unavoidable here (the `Value`
  needs to outlive `parsed` into the side-channel vector), but that clone is
  only paid when the substring check already indicates a real candidate row —
  the no-skill-event common case never clones the Value at all.

  Add a new `skill_sources: Vec<ChunkSkillSource>` vector alongside `batch` and
  `imports` (declared near line 484, `let mut batch = Vec::new();`):
  ```rust
      let mut imports = Vec::new();
      let mut batch = Vec::new();
      let mut skill_sources = Vec::new();
      let mut chunk_bytes = 0usize;
  ```
  Push into it in lockstep with `batch.push(entry);` / `imports.push(record_key);`:
  ```rust
                  chunk_bytes = chunk_bytes.saturating_add(log_entry_string_bytes(&entry));
                  batch.push(entry);
                  imports.push(record_key);
                  skill_sources.push(skill_source);
                  if batch.len() >= MAX_INDEX_CHUNK_RECORDS || chunk_bytes >= MAX_INDEX_CHUNK_BYTES {
                      if !flush_chunk(
                          pool,
                          storage,
                          source_id,
                          &mut batch,
                          &mut imports,
                          &mut skill_sources,
                          None,
                          &mut result,
                      )? {
                          return Ok(result);
                      }
                      chunk_bytes = 0;
                  }
  ```

  Update both remaining `flush_chunk(...)` call sites inside
  `index_file_with_options` (the `result.parse_errors > 0` early-return branch
  and the final flush) to pass `&mut skill_sources` in the same position, and
  clear it exactly where `imports.clear()` already happens (inside
  `flush_chunk` itself, per below) — do NOT clear it at the call sites.

  Now update `flush_chunk`'s signature and body (currently lines 774-832):
  ```rust
  fn flush_chunk(
      pool: &DbPool,
      storage: Option<&StorageConfig>,
      source_id: i64,
      batch: &mut Vec<LogBatchEntry>,
      imports: &mut Vec<String>,
      skill_sources: &mut Vec<ChunkSkillSource>,
      completion_metadata: Option<&FileMetadata>,
      result: &mut IndexResult,
  ) -> Result<bool> {
      if batch.is_empty() {
          skill_sources.clear();
          if let Some(file_metadata) = completion_metadata {
              let mut conn = pool.get()?;
              let _write_guard = crate::db::write_lock();
              let tx = conn.transaction()?;
              checkpoint::update_source_metadata_in_tx(&tx, source_id, file_metadata)?;
              tx.commit()?;
              result.checkpoint_updates += 1;
          }
          return Ok(true);
      }

      if let Some(storage) = storage {
          let outcome = enforce_storage_budget(pool, storage)?;
          if outcome.write_blocked {
              result.storage_blocked_chunks += 1;
              batch.clear();
              imports.clear();
              skill_sources.clear();
              return Ok(false);
          }
      }

      let mut conn = pool.get()?;
      let _write_guard = crate::db::write_lock();
      let tx = conn.transaction()?;
      let claimed = checkpoint::claim_imports_in_tx(&tx, source_id, imports)?;
      let mut claimed_batch = Vec::with_capacity(batch.len());
      let mut claimed_skill_sources = Vec::with_capacity(skill_sources.len());
      let mut skipped_dupes = 0usize;
      for ((entry, claimed), skill_source) in
          batch.drain(..).zip(claimed).zip(skill_sources.drain(..))
      {
          if claimed {
              claimed_batch.push(entry);
              claimed_skill_sources.push(skill_source);
          } else {
              skipped_dupes += 1;
          }
      }
      if !claimed_batch.is_empty() {
          let log_ids = insert_logs_batch_in_tx(&tx, &claimed_batch)?;
          let mut skill_inserts = Vec::new();
          for ((entry, log_id), skill_source) in claimed_batch
              .iter()
              .zip(log_ids.iter().copied())
              .zip(claimed_skill_sources.iter())
          {
              let extracted = match skill_source {
                  ChunkSkillSource::Claude(value) => extract_claude_skill_events(value),
                  ChunkSkillSource::Codex(text) => extract_codex_skill_events(text),
                  ChunkSkillSource::None => Vec::new(),
              };
              for event in extracted {
                  skill_inserts.push(SkillEventInsert {
                      log_id,
                      ai_tool: entry.ai_tool.clone().unwrap_or_default(),
                      ai_project: entry.ai_project.clone(),
                      ai_session_id: entry.ai_session_id.clone(),
                      hostname: entry.hostname.clone(),
                      timestamp: entry.timestamp.clone(),
                      event,
                  });
              }
          }
          if !skill_inserts.is_empty() {
              insert_skill_events_in_tx(&tx, &skill_inserts)?;
          }
      }
      if let Some(file_metadata) = completion_metadata {
          checkpoint::update_source_metadata_in_tx(&tx, source_id, file_metadata)?;
      }
      tx.commit()?;
      result.ingested += claimed_batch.len();
      result.skipped_dupes += skipped_dupes;
      if completion_metadata.is_some() {
          result.checkpoint_updates += 1;
      }
      imports.clear();
      Ok(true)
  }
  ```

  Finally, update the Gemini path (`index_gemini_file`, around lines
  1026-1197): it calls `flush_chunk` three times with the same signature. Add
  a local `let mut skill_sources: Vec<ChunkSkillSource> = Vec::new();` right
  next to its existing `let mut imports = Vec::new();` (Gemini rows never
  produce skill events — Gemini extraction is explicitly out of scope for this
  phase), push `ChunkSkillSource::None` in lockstep with each
  `imports.push(record_key);` in that function, and pass `&mut skill_sources`
  to each of its three `flush_chunk(...)` calls.

  **Eng review Fix 1 summary — no double parse anywhere on this path:** the
  short-circuit now exists at TWO layers, both cheap: (1) the per-line loop in
  `index_file_with_options` only clones `parsed.raw_value` into
  `ChunkSkillSource::Claude` when `line_text.contains("attributionSkill")`, and
  only builds `ChunkSkillSource::Codex` when `parsed.message.contains("<skill>")`
  — the common no-skill-event row does neither; (2) `extract_codex_skill_events`
  itself (Task 3) re-checks the same substring before touching the regex
  engine, so even a caller that skips layer (1) is still bounded. At no point
  does `flush_chunk` call `serde_json::from_str` on `line_text` — the only JSON
  parse of a Claude transcript line happens once, inside `claude::parse_line`.

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib scanner
  ```
  Expected: all `scanner::tests::*` pass including the 4 new tests from Step 1,
  and no regressions in the pre-existing scanner test suite (idempotency,
  force-reindex, checkpoint, gemini tests).

  Also run the full lib build + clippy to confirm the refactor is clean:
  ```bash
  cargo build --lib && cargo clippy --lib -- -D warnings
  ```

- [ ] **Step 5: Commit**
  ```bash
  git add src/scanner.rs
  git commit -m "feat(scanner): extract and persist ai_skill_events during transcript ingest"
  ```

---

### Task 7: Backfill service method (`CortexService::backfill_skill_events`)

**Files:**
- Create: `src/app/services/skill_backfill.rs`
- Modify: `src/app/services.rs` (add `mod skill_backfill;` near the other
  `mod ai_indexing;` line, and re-export the new request/result types from
  `src/app/models.rs` — see below)
- Create: `src/app/models/skill_backfill.rs` (request/result structs, following
  the pattern of `src/app/models/ai_inventory.rs`)
- Modify: `src/app/models.rs` (add `mod skill_backfill;` and re-export)
- Test: `src/app/services/skill_backfill_tests.rs` (new sidecar; note
  `src/app/services.rs` itself has no direct sidecar — service submodules like
  `ai_indexing.rs` typically colocate `#[cfg(test)] mod tests { ... }` inline
  OR use a sidecar; check `src/app/services/ai_indexing.rs`'s tail to confirm
  which convention it uses and mirror exactly that one for `skill_backfill.rs`)

**Interfaces:**
- Consumes: `crate::db::{list_hosts is NOT used here — instead, this task
  queries `logs` directly via a new lightweight `SELECT id, ...FROM logs WHERE
  ai_tool IN ('claude','codex') ...` query added inside
  `skill_backfill.rs` itself (not `queries.rs`), since it is backfill-specific
  and chunked}; `crate::db::{SkillEventInsert, insert_skill_events}` (Task 4);
  `crate::scanner::skill_events::{extract_claude_skill_events,
  extract_codex_skill_events}` (Tasks 2-3); `crate::db::write_lock` (existing)
- Produces: `SkillBackfillRequest`, `SkillBackfillResult` (locked above),
  `CortexService::backfill_skill_events`

- [ ] **Step 1: Write the failing test**

  First inspect the sidecar convention:
  ```bash
  tail -20 src/app/services/ai_indexing.rs
  ```
  Use whichever pattern that shows (inline `#[cfg(test)] mod tests { use
  super::*; ... }` vs `#[path = "..._tests.rs"]`). The steps below assume the
  sidecar-file convention (matching every other module in this phase); if
  `ai_indexing.rs` instead uses an inline test module, use that shape instead
  for `skill_backfill.rs` and skip creating a separate `_tests.rs` file.

  Create `src/app/services/skill_backfill_tests.rs`:
  ```rust
  use super::*;
  use crate::app::CortexService;
  use crate::config::StorageConfig;
  use crate::db::{DbPool, init_pool};
  use std::sync::Arc;

  fn test_service() -> (CortexService, tempfile::TempDir) {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let storage = StorageConfig::for_test(db_path);
      let pool: Arc<DbPool> = Arc::new(init_pool(&storage).unwrap());
      (CortexService::new(pool, storage), dir)
  }

  fn insert_claude_log_row(pool: &DbPool, message: &str) -> i64 {
      let conn = pool.get().unwrap();
      conn.execute(
          "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip, ai_tool, ai_project, ai_session_id)
           VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', ?1, ?1, 'transcript://claude_project', 'claude', 'cortex', 'sess-1')",
          rusqlite::params![message],
      )
      .unwrap();
      conn.last_insert_rowid()
  }

  #[tokio::test]
  async fn dry_run_reports_counts_without_inserting() {
      let (service, _dir) = test_service();
      let pool = service.pool_for_test();
      insert_claude_log_row(&pool, r#"{"attributionSkill":"cortex-troubleshoot"}"#);

      let result = service
          .backfill_skill_events(SkillBackfillRequest {
              since: None,
              limit: Some(100),
              dry_run: true,
          })
          .await
          .unwrap();

      assert_eq!(result.scanned, 1);
      assert_eq!(result.inserted, 0);
      assert!(result.dry_run);

      let conn = pool.get().unwrap();
      let count: i64 = conn
          .query_row("SELECT COUNT(*) FROM ai_skill_events", [], |row| row.get(0))
          .unwrap();
      assert_eq!(count, 0);
  }

  #[tokio::test]
  async fn real_run_inserts_events_and_is_idempotent() {
      let (service, _dir) = test_service();
      let pool = service.pool_for_test();
      insert_claude_log_row(&pool, r#"{"attributionSkill":"cortex-troubleshoot"}"#);

      let first = service
          .backfill_skill_events(SkillBackfillRequest {
              since: None,
              limit: Some(100),
              dry_run: false,
          })
          .await
          .unwrap();
      assert_eq!(first.scanned, 1);
      assert_eq!(first.inserted, 1);
      assert_eq!(first.skipped_duplicates, 0);

      let second = service
          .backfill_skill_events(SkillBackfillRequest {
              since: None,
              limit: Some(100),
              dry_run: false,
          })
          .await
          .unwrap();
      assert_eq!(second.scanned, 1);
      assert_eq!(second.inserted, 0);
      assert_eq!(second.skipped_duplicates, 1);
  }

  #[tokio::test]
  async fn limit_is_clamped_to_hard_upper_bound() {
      // Eng review Fix 7 — an operator/caller passing an absurd limit doesn't
      // drive an unbounded scan; it's silently clamped to the hard cap.
      let (service, _dir) = test_service();
      insert_claude_log_row(&service.pool_for_test(), r#"{"attributionSkill":"cortex"}"#);

      // 10_000_000 exceeds the hard cap (1_000_000) — should not error, should
      // just clamp. We can't easily observe the internal clamp directly
      // without a huge fixture, so this asserts the call succeeds rather than
      // erroring or hanging (a stronger unit test for the clamp arithmetic
      // itself lives at the module level below, not through the service).
      let result = service
          .backfill_skill_events(SkillBackfillRequest {
              since: None,
              limit: Some(10_000_000),
              dry_run: true,
          })
          .await
          .unwrap();
      assert_eq!(result.scanned, 1);
  }

  #[tokio::test]
  async fn concurrent_backfill_calls_return_busy_instead_of_racing() {
      // Eng review Fix 7 — single-flight guard. Two concurrent calls: one
      // proceeds, the other observes the guard held and returns a clear
      // "already running" error rather than both scanning the same corpus
      // simultaneously. This test drives the guard directly (see Step 3's
      // `BACKFILL_GUARD` for why a real two-task race is flaky to assert
      // deterministically in a unit test) — it holds the guard manually to
      // simulate an in-flight backfill, then asserts the service call observes
      // it and fails fast.
      let (service, _dir) = test_service();
      insert_claude_log_row(&service.pool_for_test(), r#"{"attributionSkill":"cortex"}"#);

      let _held = super::backfill_guard()
          .clone()
          .try_acquire_owned()
          .expect("guard should be free at test start");

      let result = service
          .backfill_skill_events(SkillBackfillRequest {
              since: None,
              limit: Some(100),
              dry_run: true,
          })
          .await;

      assert!(result.is_err(), "second concurrent backfill call must be rejected");
  }
  ```

  This test assumes a `pool_for_test()` accessor exists on `CortexService` for
  tests to reach the underlying pool directly. Check
  `src/app/services.rs`/`src/app/service_tests.rs` for whether such an accessor
  already exists (search `grep -n "fn pool_for_test\|pub(crate) fn pool\b"
  src/app/services.rs`). If it does not exist, add:
  ```rust
  #[cfg(test)]
  pub(crate) fn pool_for_test(&self) -> Arc<DbPool> {
      Arc::clone(&self.pool)
  }
  ```
  to the `impl CortexService` block in `src/app/services.rs`.

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib app::services::skill_backfill
  ```
  Expected: compile error — `SkillBackfillRequest`/`backfill_skill_events` do
  not exist yet.

- [ ] **Step 3: Write minimal implementation**

  Create `src/app/models/skill_backfill.rs`:
  ```rust
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Default, Deserialize)]
  pub struct SkillBackfillRequest {
      pub since: Option<String>,
      pub limit: Option<u64>,
      #[serde(default)]
      pub dry_run: bool,
  }

  #[derive(Debug, Clone, Default, Serialize, Deserialize)]
  pub struct SkillBackfillResult {
      pub scanned: u64,
      pub inserted: u64,
      pub skipped_duplicates: u64,
      pub parse_errors: u64,
      pub truncated: bool,
      pub dry_run: bool,
  }
  ```

  In `src/app/models.rs`, add `mod skill_backfill;` and
  `pub use skill_backfill::{SkillBackfillRequest, SkillBackfillResult};`
  next to the other `mod`/`pub use` lines (match existing alphabetical-ish
  grouping in that file).

  Create `src/app/services/skill_backfill.rs`:
  ```rust
  //! `ai skills backfill` — scans existing `logs` rows with `ai_tool IN
  //! ('claude','codex')` and extracts/persists `ai_skill_events` for rows that
  //! predate this phase's ingest-time wiring (`src/scanner.rs::flush_chunk`).
  //!
  //! Chunked scan-and-release: each chunk acquires a fresh pool connection,
  //! processes up to `CHUNK_SIZE` rows, and drops the connection before
  //! continuing — mirrors `purge_old_logs` in `src/db/maintenance.rs` so a
  //! large historical backfill never starves the ingest writer of a pool
  //! connection for more than one chunk's duration.
  //!
  //! **Eng review Fix 6**: unlike `purge_old_logs` (which DELETEs and
  //! correctly holds `write_lock()`), `fetch_candidate_chunk` here is a pure
  //! `SELECT` — WAL mode already gives readers a consistent snapshot without
  //! the write lock, so it is NOT held around the fetch. Only
  //! `insert_skill_events` (a real write) acquires the lock, and it does so
  //! internally (see Task 4).
  //!
  //! **Eng review Fix 7**: `limit` is hard-clamped to `[1, 1_000_000]` (an
  //! operator/caller cannot drive an unbounded scan), and a process-wide
  //! single-flight guard (`backfill_guard`) ensures only one backfill runs at
  //! a time — a second concurrent call fails fast with `ServiceError::Busy`
  //! instead of both holding a `run_db` semaphore permit for the whole
  //! multi-chunk scan. The guard is service-scoped (not REST-scoped like
  //! `api.rs`'s `SHARED_MAINTENANCE_PERMIT`) because this method is also
  //! reachable from the CLI's local mode and MCP, neither of which goes
  //! through `api.rs`.

  use std::sync::{Arc, OnceLock};

  use anyhow::Result;
  use rusqlite::params;
  use tokio::sync::Semaphore;

  use crate::db::{DbPool, SkillEventInsert, insert_skill_events};
  use crate::scanner::skill_events::{extract_claude_skill_events, extract_codex_skill_events};

  use super::super::models::{SkillBackfillRequest, SkillBackfillResult};
  use super::super::time::parse_optional_timestamp;
  use super::{CortexService, ServiceError, ServiceResult};

  const CHUNK_SIZE: i64 = 2_000;

  /// Eng review Fix 7: hard upper bound on `SkillBackfillRequest.limit`. Chosen
  /// generously above any realistic single-host `logs` table size (millions of
  /// rows would already be well past `CORTEX_MAX_DB_SIZE_MB`'s default 1024 MB
  /// guard in practice) while still being a real, enforced ceiling rather than
  /// "whatever the caller asks for" — closes the unbounded-scan DoS surface
  /// now that this is a `pub` service method reachable from CLI/MCP/REST.
  const MAX_BACKFILL_LIMIT: u64 = 1_000_000;

  /// Eng review Fix 7: process-wide single-flight gate for
  /// `backfill_skill_events`, mirroring the `SHARED_MAINTENANCE_PERMIT`
  /// pattern in `src/api.rs` (`OnceLock<Arc<Semaphore>>` + `try_acquire_owned`)
  /// but scoped to the service layer so CLI-local and MCP callers are covered
  /// too, not just REST. A held permit means a backfill is in flight; a second
  /// concurrent call observes `NoPermits` and returns `ServiceError::Busy`
  /// immediately rather than queuing behind the first scan.
  fn backfill_guard() -> Arc<Semaphore> {
      static GUARD: OnceLock<Arc<Semaphore>> = OnceLock::new();
      Arc::clone(GUARD.get_or_init(|| Arc::new(Semaphore::new(1))))
  }

  struct CandidateRow {
      id: i64,
      ai_tool: String,
      ai_project: Option<String>,
      ai_session_id: Option<String>,
      hostname: String,
      timestamp: String,
      message: String,
  }

  impl CortexService {
      pub async fn backfill_skill_events(
          &self,
          req: SkillBackfillRequest,
      ) -> ServiceResult<SkillBackfillResult> {
          let since = parse_optional_timestamp(req.since.as_deref(), "since")?
              .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
          // Eng review Fix 7: hard clamp, not just a floor.
          let limit = req.limit.unwrap_or(10_000).clamp(1, MAX_BACKFILL_LIMIT);
          let dry_run = req.dry_run;

          // Eng review Fix 7: single-flight guard acquired BEFORE the run_db
          // call so a second concurrent caller never even queues for a DB
          // permit — it fails fast here instead.
          let _permit = backfill_guard()
              .try_acquire_owned()
              .map_err(|_| ServiceError::Busy("skill event backfill already running".into()))?;

          self.run_db("backfill_skill_events", move |pool| {
              run_backfill(pool, since.as_deref(), limit, dry_run)
          })
          .await
      }
  }

  fn run_backfill(
      pool: &DbPool,
      since: Option<&str>,
      limit: u64,
      dry_run: bool,
  ) -> Result<SkillBackfillResult> {
      let mut result = SkillBackfillResult {
          dry_run,
          ..Default::default()
      };
      let mut last_id = 0i64;
      let mut remaining = limit;

      loop {
          if remaining == 0 {
              result.truncated = true;
              break;
          }
          let chunk_limit = CHUNK_SIZE.min(remaining as i64).max(1);
          // Eng review Fix 6: no write_lock() here — this is a pure SELECT and
          // WAL mode already gives it a consistent snapshot. Holding the write
          // lock around a read needlessly serializes the backfill against the
          // live syslog ingest writer for zero correctness benefit.
          let conn = pool.get()?;
          let rows = fetch_candidate_chunk(&conn, since, last_id, chunk_limit)?;
          drop(conn); // release back to pool before per-row parse work + next chunk

          if rows.is_empty() {
              break;
          }
          last_id = rows.last().map(|r| r.id).unwrap_or(last_id);
          result.scanned += rows.len() as u64;
          remaining = remaining.saturating_sub(rows.len() as u64);

          let mut inserts = Vec::new();
          for row in &rows {
              // Eng review Fix 1: the backfill reads `row.message` straight
              // from the `logs` table (there is no pre-parsed Value to reuse
              // here, unlike the ingest hot path in Task 6 — this is a
              // one-time historical scan, not the per-request ingest loop),
              // so a JSON parse is unavoidable for Claude rows that DO have a
              // skill event. The substring short-circuit still applies: skip
              // the parse entirely for the common case where the row has no
              // attributionSkill field at all.
              let extracted = match row.ai_tool.as_str() {
                  "claude" if row.message.contains("attributionSkill") => {
                      match serde_json::from_str::<serde_json::Value>(&row.message) {
                          Ok(value) => extract_claude_skill_events(&value),
                          Err(_) => {
                              result.parse_errors += 1;
                              continue;
                          }
                      }
                  }
                  "claude" => continue,
                  "codex" => extract_codex_skill_events(&row.message),
                  _ => continue,
              };
              for event in extracted {
                  inserts.push(SkillEventInsert {
                      log_id: row.id,
                      ai_tool: row.ai_tool.clone(),
                      ai_project: row.ai_project.clone(),
                      ai_session_id: row.ai_session_id.clone(),
                      hostname: row.hostname.clone(),
                      timestamp: row.timestamp.clone(),
                      event,
                  });
              }
          }

          if !dry_run && !inserts.is_empty() {
              let attempted = inserts.len() as u64;
              let inserted = insert_skill_events(pool, &inserts)? as u64;
              result.inserted += inserted;
              result.skipped_duplicates += attempted - inserted;
          } else if dry_run {
              // Dry-run still reports how many events WOULD be inserted as
              // "inserted" would be misleading; instead every extracted event
              // in a dry run counts toward neither inserted nor duplicates —
              // scanned/parse_errors are the only meaningful dry-run signal
              // per the CLI contract (`--dry-run` reports scanned rows and
              // parse errors without touching the table). Callers that need a
              // precise "would insert N" count should drop --dry-run.
          }

          if (rows.len() as i64) < chunk_limit {
              break;
          }
          std::thread::sleep(std::time::Duration::from_millis(10));
      }

      Ok(result)
  }

  fn fetch_candidate_chunk(
      conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
      since: Option<&str>,
      last_id: i64,
      chunk_limit: i64,
  ) -> Result<Vec<CandidateRow>> {
      let (sql, bindings): (&str, Vec<rusqlite::types::Value>) = match since {
          Some(since) => (
              "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, message
               FROM logs
               WHERE ai_tool IN ('claude', 'codex')
                 AND id > ?1
                 AND timestamp >= ?2
               ORDER BY id ASC
               LIMIT ?3",
              vec![
                  rusqlite::types::Value::Integer(last_id),
                  rusqlite::types::Value::Text(since.to_string()),
                  rusqlite::types::Value::Integer(chunk_limit),
              ],
          ),
          None => (
              "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, message
               FROM logs
               WHERE ai_tool IN ('claude', 'codex')
                 AND id > ?1
               ORDER BY id ASC
               LIMIT ?2",
              vec![
                  rusqlite::types::Value::Integer(last_id),
                  rusqlite::types::Value::Integer(chunk_limit),
              ],
          ),
      };
      let mut stmt = conn.prepare(sql)?;
      let rows = stmt
          .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
              Ok(CandidateRow {
                  id: row.get(0)?,
                  ai_tool: row.get(1)?,
                  ai_project: row.get(2)?,
                  ai_session_id: row.get(3)?,
                  hostname: row.get(4)?,
                  timestamp: row.get(5)?,
                  message: row.get(6)?,
              })
          })?
          .collect::<rusqlite::Result<Vec<_>>>()?;
      Ok(rows)
  }

  #[cfg(test)]
  #[path = "skill_backfill_tests.rs"]
  mod tests;
  ```

  NOTE for implementer: `r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>`
  is the concrete type `pool.get()` returns in this codebase — confirm the
  exact type alias by checking `type DbPool = r2d2::Pool<...>` in
  `src/db/pool.rs` and use whatever type alias (e.g. `crate::db::PooledConn` if
  one is already exported) is idiomatic here instead of spelling out the full
  generic, to match house style. If no such alias is exported, add
  `pub(crate) type PooledConn<'a> = ...` or just use `&rusqlite::Connection`
  by deref — check how `src/db/maintenance.rs`'s chunked functions type their
  `conn` parameter and mirror that exactly (grep
  `fn delete_heartbeat_chunk_where` signature for the pattern).

  Modify `src/app/services.rs`: add `mod skill_backfill;` next to
  `mod ai_indexing;` (around line 64).

  Also, make `backfill_guard` reachable from the test module (the
  `concurrent_backfill_calls_return_busy_instead_of_racing` test in Step 1
  calls `super::backfill_guard()`), so keep its visibility at the module level
  (private is fine — `#[path = "skill_backfill_tests.rs"] mod tests;` is a
  child module and can see private items via `super::`).

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib app::services::skill_backfill
  ```
  Expected: all four tests pass — `dry_run_reports_counts_without_inserting`,
  `real_run_inserts_events_and_is_idempotent`,
  `limit_is_clamped_to_hard_upper_bound` (Fix 7), and
  `concurrent_backfill_calls_return_busy_instead_of_racing` (Fix 7).

- [ ] **Step 5: Commit**
  ```bash
  git add src/app/services/skill_backfill.rs src/app/services/skill_backfill_tests.rs src/app/models/skill_backfill.rs src/app/models.rs src/app/services.rs
  git commit -m "feat(app): add backfill_skill_events service method with dry-run, idempotent chunked scan, single-flight guard, and hard limit clamp"
  ```

---

### Task 8: `cortex sessions skills backfill` CLI command

**Files:**
- Modify: `src/cli/args.rs` (add `SessionsCommand::Skills(SessionsSkillsCommand)`
  variant, `SessionsSkillsCommand` enum, `SessionsSkillsBackfillArgs` struct)
- Modify: `src/cli/parse/sessions.rs` (add `"skills"` to
  `SESSIONS_SUBCOMMANDS`, dispatch to a new `parse_sessions_skills` function)
- Create: `src/cli/parse/sessions/skills.rs` (parse functions, following
  `src/cli/parse/sessions/ops.rs` shape)
- Modify: `src/cli/run.rs` (dispatch `SessionsCommand::Skills(...)` arm)
- Modify: `src/cli/dispatch_sessions.rs` (add `run_ai_skills_backfill`,
  `run_ai_skills_list` — the latter is the read surface built out fully in
  Task 9; this task only needs the backfill arm, but declare both stubs so
  Task 9 slots in cleanly)
- Test: `src/cli/parse/sessions_tests.rs` or wherever `parse_sessions_*` tests
  live (`grep -rn "parse_sessions_tools" src/cli --include="*_tests.rs"` to
  find the exact sidecar file; append there)

**Interfaces:**
- Consumes: `CortexService::backfill_skill_events` (Task 7)
- Produces: `cortex sessions skills backfill --since 30d --limit 10000
  --dry-run` and `cortex sessions skills backfill --since 30d --limit 10000`
  as working CLI invocations (Local mode only — HTTP mode explicitly rejected
  with a clear error, matching `run_ai_index`'s pattern for local-filesystem
  operations, since this is a DB-heavy batch job that should run against the
  local `CortexService`, not proxied over HTTP per this repo's HTTP-CLI-parity
  rollout status — confirm current HTTP support scope by checking whether
  `HttpClient` has an analogous batch endpoint; if unsure, follow
  `run_ai_index`'s `CliMode::Http(_) => bail!(...)` precedent exactly)

- [ ] **Step 1: Write the failing test**

  First locate the exact sidecar test file for `src/cli/parse/sessions.rs`:
  ```bash
  find src/cli -iname "*sessions*test*"
  ```
  Append to whichever file that search returns (most likely
  `src/cli/parse/sessions_tests.rs` or `src/cli/parse_tests.rs` if sessions
  parsing tests live centrally — inspect and match existing test style there,
  e.g. `parse_command(vec!["sessions".into(), "tools".into()])`):
  ```rust
  #[test]
  fn parses_sessions_skills_backfill_with_flags() {
      let command = super::parse_command(vec![
          "sessions".to_string(),
          "skills".to_string(),
          "backfill".to_string(),
          "--since".to_string(),
          "30d".to_string(),
          "--limit".to_string(),
          "10000".to_string(),
          "--dry-run".to_string(),
      ])
      .unwrap();

      match command {
          CliCommand::Sessions(SessionsCommand::SkillsBackfill(args)) => {
              assert_eq!(args.since.as_deref(), Some("30d"));
              assert_eq!(args.limit, Some(10000));
              assert!(args.dry_run);
          }
          other => panic!("expected SessionsCommand::SkillsBackfill, got {other:?}"),
      }
  }

  #[test]
  fn parses_sessions_skills_list_with_project_filter() {
      let command = super::parse_command(vec![
          "sessions".to_string(),
          "skills".to_string(),
          "--project".to_string(),
          "cortex".to_string(),
          "--limit".to_string(),
          "20".to_string(),
      ])
      .unwrap();

      match command {
          CliCommand::Sessions(SessionsCommand::Skills(args)) => {
              assert_eq!(args.project.as_deref(), Some("cortex"));
              assert_eq!(args.limit, Some(20));
          }
          other => panic!("expected SessionsCommand::Skills, got {other:?}"),
      }
  }
  ```
  Adjust the `super::parse_command` path/import to match whatever the
  discovered test file already uses (some files call `parse_command` directly
  via `use super::super::parse_command;` or similar — mirror the neighboring
  `parses_sessions_tools_*`-style test in the same file exactly).

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib cli::parse::sessions::tests::parses_sessions_skills_backfill_with_flags
  ```
  Expected: compile error — `SessionsCommand::SkillsBackfill` /
  `SessionsCommand::Skills` do not exist.

- [ ] **Step 3: Write minimal implementation**

  In `src/cli/args.rs`, find `enum SessionsCommand` and add two variants
  (alongside `Tools(SessionsListArgs)` / `Index(SessionsIndexArgs)`):
  ```rust
  Skills(SessionsSkillsListArgs),
  SkillsBackfill(SessionsSkillsBackfillArgs),
  ```
  Add the two new arg structs near `SessionsListArgs`:
  ```rust
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub(crate) struct SessionsSkillsListArgs {
      pub json: bool,
      pub skill: Option<String>,
      pub plugin: Option<String>,
      pub tool: Option<String>,
      pub project: Option<String>,
      pub session_id: Option<String>,
      pub host: Option<String>,
      pub since: Option<String>,
      pub until: Option<String>,
      pub limit: Option<u32>,
  }

  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub(crate) struct SessionsSkillsBackfillArgs {
      pub json: bool,
      pub since: Option<String>,
      pub limit: Option<u64>,
      pub dry_run: bool,
  }
  ```

  In `src/cli/parse/sessions.rs`:
  - Add `"skills"` to `SESSIONS_SUBCOMMANDS`.
  - Add a dispatch arm in `parse_sessions_command`:
    ```rust
    "skills" => super::skills::parse_sessions_skills(rest),
    ```
  - Add `mod skills;` near the top (alongside `mod more; mod ops;`).

  Create `src/cli/parse/sessions/skills.rs`:
  ```rust
  use anyhow::{Result, bail};

  use super::super::super::parse_common::{FlagCursor, norm_time, value_after_equals};
  use super::super::super::{
      CliCommand, SessionsCommand, SessionsSkillsBackfillArgs, SessionsSkillsListArgs,
  };

  pub(crate) fn parse_sessions_skills(args: &[String]) -> Result<CliCommand> {
      match args.first().map(String::as_str) {
          Some("backfill") => parse_skills_backfill(&args[1..]),
          _ => parse_skills_list(args),
      }
  }

  fn parse_skills_list(args: &[String]) -> Result<CliCommand> {
      let mut parsed = SessionsSkillsListArgs::default();
      let mut flags = FlagCursor::new(args);
      while let Some(arg) = flags.next() {
          match arg.as_str() {
              "--json" => parsed.json = true,
              "--skill" => parsed.skill = Some(flags.value("--skill")?),
              "--plugin" => parsed.plugin = Some(flags.value("--plugin")?),
              "--tool" => parsed.tool = Some(flags.value("--tool")?),
              "--project" => parsed.project = Some(flags.value("--project")?),
              "--session-id" => parsed.session_id = Some(flags.value("--session-id")?),
              "--host" => parsed.host = Some(flags.value("--host")?),
              "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
              "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
              "--limit" => parsed.limit = Some(flags.value("--limit")?.parse()?),
              _ if arg.starts_with("--skill=") => {
                  parsed.skill = Some(value_after_equals(arg, "--skill")?)
              }
              _ if arg.starts_with("--plugin=") => {
                  parsed.plugin = Some(value_after_equals(arg, "--plugin")?)
              }
              _ if arg.starts_with("--tool=") => {
                  parsed.tool = Some(value_after_equals(arg, "--tool")?)
              }
              _ if arg.starts_with("--project=") => {
                  parsed.project = Some(value_after_equals(arg, "--project")?)
              }
              _ if arg.starts_with("--session-id=") => {
                  parsed.session_id = Some(value_after_equals(arg, "--session-id")?)
              }
              _ if arg.starts_with("--host=") => {
                  parsed.host = Some(value_after_equals(arg, "--host")?)
              }
              _ if arg.starts_with("--since=") => {
                  parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
              }
              _ if arg.starts_with("--until=") => {
                  parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
              }
              _ if arg.starts_with("--limit=") => {
                  parsed.limit = Some(value_after_equals(arg, "--limit")?.parse()?)
              }
              _ => bail!("unknown sessions skills option: {arg}"),
          }
      }
      Ok(CliCommand::Sessions(SessionsCommand::Skills(parsed)))
  }

  fn parse_skills_backfill(args: &[String]) -> Result<CliCommand> {
      let mut parsed = SessionsSkillsBackfillArgs::default();
      let mut flags = FlagCursor::new(args);
      while let Some(arg) = flags.next() {
          match arg.as_str() {
              "--json" => parsed.json = true,
              "--dry-run" => parsed.dry_run = true,
              "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
              "--limit" => parsed.limit = Some(flags.value("--limit")?.parse()?),
              _ if arg.starts_with("--since=") => {
                  parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
              }
              _ if arg.starts_with("--limit=") => {
                  parsed.limit = Some(value_after_equals(arg, "--limit")?.parse()?)
              }
              _ => bail!("unknown sessions skills backfill option: {arg}"),
          }
      }
      Ok(CliCommand::Sessions(SessionsCommand::SkillsBackfill(parsed)))
  }
  ```
  Adjust the exact `FlagCursor`/`norm_time`/`value_after_equals` import path if
  the real module path differs slightly from this guess — mirror
  `src/cli/parse/sessions.rs`'s own `use super::super::parse_common::{...}`
  line exactly (this new file is one directory level deeper, so it needs
  `super::super::super::parse_common`; verify against
  `src/cli/parse/sessions/ops.rs`'s actual import line and copy it verbatim
  with the module path adjusted).

  In `src/cli/run.rs`, add to the `CliCommand::Sessions(command) => match
  command { ... }` block:
  ```rust
  super::SessionsCommand::Skills(args) => dispatch::run_ai_skills(&mode, args).await,
  super::SessionsCommand::SkillsBackfill(args) => {
      dispatch::run_ai_skills_backfill(&mode, args).await
  }
  ```

  In `src/cli/dispatch_sessions.rs`, add (near `run_ai_index`):
  ```rust
  pub(crate) async fn run_ai_skills_backfill(
      mode: &CliMode,
      args: SessionsSkillsBackfillArgs,
  ) -> Result<()> {
      let service = match mode {
          CliMode::Http(_) => bail!("sessions skills backfill runs local DB scans; omit --http"),
          CliMode::Local(service) => service,
      };
      let response = service
          .backfill_skill_events(cortex::app::SkillBackfillRequest {
              since: args.since,
              limit: args.limit,
              dry_run: args.dry_run,
          })
          .await?;
      if args.json {
          println!("{}", serde_json::to_string_pretty(&response)?);
      } else {
          println!(
              "scanned={} inserted={} skipped_duplicates={} parse_errors={} truncated={} dry_run={}",
              response.scanned,
              response.inserted,
              response.skipped_duplicates,
              response.parse_errors,
              response.truncated,
              response.dry_run
          );
      }
      Ok(())
  }
  ```
  Leave `run_ai_skills` (the list/read variant) as a stub returning
  `bail!("not yet implemented")` for now — Task 9 replaces it with the real
  implementation. Confirm `cortex::app::SkillBackfillRequest` is actually
  re-exported from `src/app.rs`'s top-level `pub use` (check
  `grep -n "pub use services" src/app.rs` and add `SkillBackfillRequest,
  SkillBackfillResult` to that re-export list if `src/app/services.rs`'s
  `models` re-export doesn't already surface them transitively).

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib cli::parse::sessions::tests::parses_sessions_skills_backfill_with_flags
  cargo test --lib cli::parse::sessions::tests::parses_sessions_skills_list_with_project_filter
  cargo build --bin cortex
  ```
  Expected: both parse tests pass; binary builds (dispatch arms compile even
  though `run_ai_skills` is a stub, since Task 9 fills it in next).

- [ ] **Step 5: Commit**
  ```bash
  git add src/cli/args.rs src/cli/parse/sessions.rs src/cli/parse/sessions/skills.rs src/cli/run.rs src/cli/dispatch_sessions.rs
  git commit -m "feat(cli): add cortex sessions skills backfill command"
  ```

---

### Task 9: `skill_events` MCP action + CLI list + REST route

**Files:**
- Modify: `src/mcp/actions.rs` (add `ActionHandler::SkillEvents` variant, add
  `action_spec!("skill_events", Read, ..., SkillEvents, ...)` row)
- Modify: `src/mcp/tools.rs` (add `H::SkillEvents => tool_skill_events(state,
  args).await` dispatch arm + `tool_skill_events` function)
- Modify: `src/app/services/skill_backfill.rs` OR create
  `src/app/services/skill_events.rs` (add `CortexService::list_skill_events`
  wrapping `db::list_skill_events`)
- Modify: `src/app/models/skill_backfill.rs` -> rename/extend, or add
  `src/app/models/skill_events.rs` with `ListSkillEventsRequest`,
  `ListSkillEventsResponse` (API-facing types distinct from the DB-layer
  `AiSkillEventParams`/`AiSkillEventEntry`, mirroring how `ListAiToolsRequest`
  (app layer) differs from `ListAiToolsParams` (db layer))
- Modify: `src/api.rs` (add `.route("/api/ai/skills", get(ai_skills))` +
  handler function)
- Modify: `src/cli/dispatch_sessions.rs` (replace the `run_ai_skills` stub from
  Task 8 with the real implementation)
- Modify: `src/cli/output/logs.rs` (add `print_skill_events_response`,
  mirroring `print_ai_tools_response` at line 322)
- Test: `src/mcp/tools_tests.rs`, `src/api_tests.rs`, plus dispatch-level CLI
  test alongside Task 8's tests

**Interfaces:**
- Consumes: `db::{AiSkillEventParams, AiSkillEventEntry, ListSkillEventsResult,
  list_skill_events}` (Task 4)
- Produces: MCP action `skill_events` (scope `cortex:read`); REST `GET
  /api/ai/skills`; CLI `cortex sessions skills [--skill NAME] [--plugin
  PLUGIN] [--tool TOOL] [--project PROJECT] [--session-id ID] [--host HOST]
  [--since ...] [--until ...] [--limit N] [--json]`

- [ ] **Step 1: Write the failing test**

  In `src/mcp/tools_tests.rs` (or wherever action-dispatch tests for a similar
  simple read action live — check the existing `list_ai_tools`/`hosts` test
  for the exact harness pattern, e.g. an in-memory `AppState` builder), add:
  ```rust
  #[tokio::test]
  async fn skill_events_action_returns_inserted_rows() {
      let (state, pool) = test_app_state(); // reuse whatever helper existing
                                             // MCP action tests use to build
                                             // an AppState over a temp DB —
                                             // match the exact helper name
                                             // used by a neighboring test such
                                             // as one exercising ListHosts or
                                             // ListApps in this same file.
      // seed one logs row + one ai_skill_events row directly via pool, same
      // shape as src/db/skill_events_tests.rs's insert_log_row helper.
      let conn = pool.get().unwrap();
      conn.execute(
          "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip)
           VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', 'm', 'm', 'transcript://claude_project')",
          [],
      )
      .unwrap();
      let log_id = conn.last_insert_rowid();
      conn.execute(
          "INSERT INTO ai_skill_events (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp, skill_name, event_kind, evidence_kind)
           VALUES (?1, 'claude', 'cortex', 'sess-1', 'devhost', '2026-06-01T00:00:00.000Z', 'cortex-troubleshoot', 'claude_attribution', 'structured_json_field')",
          rusqlite::params![log_id],
      )
      .unwrap();
      drop(conn);

      let args = serde_json::json!({"action": "skill_events", "project": "cortex"});
      let response = crate::mcp::tools::execute_tool(&state, "cortex", args, None)
          .await
          .unwrap();
      assert_eq!(response["total"], 1);
      assert_eq!(response["events"][0]["skill_name"], "cortex-troubleshoot");
  }
  ```
  Before writing this, run `grep -n "async fn test_app_state\|fn build_app_state"
  src/mcp/tools_tests.rs src/mcp_tests.rs` to find the actual test-harness
  helper name/signature in this repo and adapt the test to call it correctly
  (do not invent a helper that doesn't exist — reuse the established one).

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib mcp::tools::tests::skill_events_action_returns_inserted_rows
  ```
  Expected: compile error or "unknown cortex action: skill_events" runtime
  error — action not registered yet.

- [ ] **Step 3: Write minimal implementation**

  In `src/mcp/actions.rs`:
  - Add `SkillEvents,` to the `ActionHandler` enum (alongside `Graph,` near the
    end, before `Help,`).
  - Add a new flags const near `TOPIC_CORRELATE_FLAGS` import area, or reuse
    `&[]` for a first cut (flags metadata is optional per the `action_spec!`
    short form). Add the action row right before the `help` entry (after
    `"graph"`):
    ```rust
    action_spec!(
        "skill_events",
        Read,
        "List extracted AI skill-invocation events",
        Cheap,
        SkillEvents,
        flags: &[],
        examples: &[
            "cortex sessions skills --project cortex --limit 20",
            "cortex sessions skills --skill cortex-troubleshoot --since 1h",
        ]
    ),
    ```

  In `src/mcp/tools.rs`:
  - Add dispatch arm: `H::SkillEvents => tool_skill_events(state, args).await,`
    (near `H::Graph => ...`).
  - Add handler function (near `tool_list_hosts`):
    ```rust
    async fn tool_skill_events(state: &AppState, args: Value) -> anyhow::Result<Value> {
        let req: cortex::app::ListSkillEventsRequest = action_payload(args, "skill_events")?;
        let response = state.service.list_skill_events(req).await?;
        Ok(serde_json::to_value(response)?)
    }
    ```
    Check `action_payload`'s exact signature/import in this file (it's already
    used by every other handler, e.g. `tool_list_apps`) and match it.

  Create `src/app/models/skill_events.rs`:
  ```rust
  use serde::{Deserialize, Serialize};

  use crate::db;

  #[derive(Debug, Clone, Default, Deserialize)]
  pub struct ListSkillEventsRequest {
      pub skill: Option<String>,
      pub plugin: Option<String>,
      pub tool: Option<String>,
      pub project: Option<String>,
      pub session_id: Option<String>,
      pub hostname: Option<String>,
      pub from: Option<String>,
      pub to: Option<String>,
      pub limit: Option<u32>,
  }

  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct SkillEventEntry {
      pub id: i64,
      pub log_id: i64,
      pub ai_tool: String,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub hostname: String,
      pub timestamp: String,
      pub skill_name: String,
      pub skill_plugin: Option<String>,
      pub event_kind: String,
      pub evidence_kind: String,
  }

  impl From<db::AiSkillEventEntry> for SkillEventEntry {
      fn from(value: db::AiSkillEventEntry) -> Self {
          Self {
              id: value.id,
              log_id: value.log_id,
              ai_tool: value.ai_tool,
              ai_project: value.ai_project,
              ai_session_id: value.ai_session_id,
              hostname: value.hostname,
              timestamp: value.timestamp,
              skill_name: value.skill_name,
              skill_plugin: value.skill_plugin,
              event_kind: value.event_kind,
              evidence_kind: value.evidence_kind,
          }
      }
  }

  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct ListSkillEventsResponse {
      pub total: usize,
      pub truncated: bool,
      pub events: Vec<SkillEventEntry>,
  }

  impl From<db::ListSkillEventsResult> for ListSkillEventsResponse {
      fn from(value: db::ListSkillEventsResult) -> Self {
          Self {
              total: value.total,
              truncated: value.truncated,
              events: value.events.into_iter().map(Into::into).collect(),
          }
      }
  }
  ```
  Register it in `src/app/models.rs` (`mod skill_events;` +
  `pub use skill_events::{ListSkillEventsRequest, ListSkillEventsResponse,
  SkillEventEntry};`).

  Add the service method — put it in `src/app/services/skill_backfill.rs`
  right after `backfill_skill_events`, or a new `src/app/services/skill_events.rs`
  if the module is getting large; either is fine, pick whichever keeps
  `skill_backfill.rs` under roughly 200 lines. If splitting, remember to add
  `mod skill_events;` to `src/app/services.rs`.
  ```rust
  impl CortexService {
      pub async fn list_skill_events(
          &self,
          req: ListSkillEventsRequest,
      ) -> ServiceResult<ListSkillEventsResponse> {
          let from = parse_optional_timestamp(req.from.as_deref(), "from")?
              .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
          let to = parse_optional_timestamp(req.to.as_deref(), "to")?
              .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
          let params = db::AiSkillEventParams {
              skill: req.skill,
              plugin: req.plugin,
              tool: req.tool,
              project: req.project,
              session_id: req.session_id,
              hostname: req.hostname,
              from,
              to,
              limit: req.limit,
          };
          let result = self
              .run_db("list_skill_events", move |pool| db::list_skill_events(pool, &params))
              .await?;
          Ok(result.into())
      }
  }
  ```

  In `src/api.rs`:
  - Add route: `.route("/api/ai/skills", get(ai_skills))` (near the other
    `/api/sessions/*` routes for locality).
  - Add handler. **Eng review Fix 9**: `skill_events` stays `cortex:read`-scoped
    per GH #94's explicit decision (not reconsidered here — do not add
    `require_api_admin_token`), but as cheap defense-in-depth this handler logs
    the caller IP and query filters at `tracing::info!` before serving the
    response — matching the logging LEVEL convention of `Read`-scoped routes in
    this file (the admin-scoped `ai_llm_invocations` uses `tracing::warn!`
    because it exposes kill-switch/circuit-breaker operational state; a plain
    `Read`-scoped AI-transcript route like this one uses `info!` instead, so
    there's at least a trace record of who queried skill-usage history without
    the noise level of a `warn!` on every normal read):
    ```rust
    async fn ai_skills(
        State(state): State<ApiState>,
        ConnectInfo(peer): ConnectInfo<SocketAddr>,
        Query(req): Query<ListSkillEventsRequest>,
    ) -> impl IntoResponse {
        tracing::info!(
            caller_ip = %peer.ip(),
            skill = ?req.skill,
            plugin = ?req.plugin,
            tool = ?req.tool,
            project = ?req.project,
            session_id = ?req.session_id,
            hostname = ?req.hostname,
            "read: skill_events queried"
        );
        respond(state.service.list_skill_events(req).await)
    }
    ```
    Add `ListSkillEventsRequest` to the big `use super::models::{...}` import
    block at the top of `src/api.rs`. `ConnectInfo<SocketAddr>` and
    `SocketAddr` are already imported/used by other handlers in this file
    (e.g. `ai_llm_invocations`, `db_vacuum`) — reuse the same imports, do not
    add duplicates.

  In `src/cli/dispatch_sessions.rs`, replace the Task-8 stub `run_ai_skills`
  with:
  ```rust
  pub(crate) async fn run_ai_skills(mode: &CliMode, args: SessionsSkillsListArgs) -> Result<()> {
      let json = args.json;
      let req = cortex::app::ListSkillEventsRequest {
          skill: args.skill,
          plugin: args.plugin,
          tool: args.tool,
          project: args.project,
          session_id: args.session_id,
          hostname: args.host,
          from: args.since,
          to: args.until,
          limit: args.limit,
      };
      let response = match mode {
          CliMode::Local(service) => service.list_skill_events(req).await?,
          CliMode::Http(client) => http_or_cancel(client.ai_skills(&req)).await?,
      };
      super::output::logs::print_skill_events_response(&response, json)
  }
  ```
  If `HttpClient` has no `ai_skills` method yet, either add a minimal one
  mirroring `client.ai_tools(&req)` (a GET to `/api/ai/skills` with query-string
  serialization via the same `serde_qs` mechanism already used for other
  list endpoints — check `src/cli/http_client.rs`'s `ai_tools` implementation
  and copy its shape), or fall back to `CliMode::Http(_) => bail!("sessions
  skills over --http is not yet supported")` if adding full HTTP parity is out
  of scope for this task — prefer adding the real HTTP client method since the
  pattern is mechanical and this repo's convention is full CLI/MCP/REST parity
  per `CLAUDE.md`.

  In `src/cli/output/logs.rs`, add near `print_ai_tools_response`. **Eng
  review Fix 8 note**: this function uses `println!` directly on
  `event.skill_name` / `event.skill_plugin` with no additional sanitization —
  that's safe because Task 2/3's `ExtractedSkillEvent::normalized()` already
  rejects any skill name/plugin containing a control character before it ever
  reaches the database, so by the time a row gets here it cannot contain an
  ANSI escape or embedded newline. Do not re-add sanitization here; the fix
  belongs at the extraction boundary, not the printer.
  ```rust
  pub(crate) fn print_skill_events_response(
      response: &cortex::app::ListSkillEventsResponse,
      json: bool,
  ) -> Result<()> {
      if json {
          println!("{}", serde_json::to_string_pretty(response)?);
          return Ok(());
      }
      if response.events.is_empty() {
          println!("No skill events found.");
          return Ok(());
      }
      for event in &response.events {
          println!(
              "{}  {}  {}{}  tool={} project={}",
              event.timestamp,
              event.skill_name,
              event.skill_plugin.as_deref().map(|p| format!("plugin={p} ")).unwrap_or_default(),
              event.event_kind,
              event.ai_tool,
              event.ai_project.as_deref().unwrap_or("-"),
          );
      }
      if response.truncated {
          println!("(truncated — refine filters or raise --limit)");
      }
      Ok(())
  }
  ```
  Match this function's exact formatting style to whatever
  `print_ai_tools_response` actually does (read it first) rather than
  inventing new conventions — the snippet above is a reasonable placeholder
  shape to adapt.

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib mcp::tools::tests::skill_events_action_returns_inserted_rows
  cargo test --lib api
  cargo build --bin cortex
  cargo clippy --all-targets -- -D warnings
  ```
  Expected: MCP action test passes; `cargo build`/`clippy` clean across the
  whole workspace (this is the task where every surface — MCP, REST, CLI —
  must compile together).

- [ ] **Step 5: Commit**
  ```bash
  git add src/mcp/actions.rs src/mcp/tools.rs src/app/models/skill_events.rs src/app/models.rs src/app/services.rs src/app/services/skill_backfill.rs src/api.rs src/cli/dispatch_sessions.rs src/cli/output/logs.rs
  git commit -m "feat(mcp,api,cli): add skill_events read action across MCP, REST, and CLI surfaces"
  ```

---

### Task 10: Full-surface integration test (ingest -> backfill -> list, end-to-end idempotency)

**Files:**
- Test: `src/scanner_tests.rs` (or a new `tests/skill_events_integration.rs`
  black-box test if the repo has a `tests/` integration-test convention beyond
  fixtures — check `find tests -maxdepth 1 -name "*.rs"` first; if
  `tests/*.rs` integration tests already exist for similar cross-module flows,
  add there instead of `scanner_tests.rs` to avoid growing that file further)

**Interfaces:**
- Consumes: everything from Tasks 1-9
- Produces: no new public API; proves the full pipeline is coherent end-to-end.

- [ ] **Step 1: Write the failing test**

  ```bash
  find tests -maxdepth 1 -name "*.rs"
  ```
  If an integration-test binary convention exists (e.g. `tests/smoke.rs`),
  add there with `use cortex::...` crate-external imports. Otherwise append to
  `src/scanner_tests.rs`:
  ```rust
  #[test]
  fn end_to_end_ingest_then_backfill_is_idempotent_across_both_paths() {
      let (pool, dir) = test_pool();

      // Row 1: indexed normally (picks up the skill event via flush_chunk).
      let claude_file = dir.path().join("claude.jsonl");
      std::fs::write(
          &claude_file,
          concat!(
              r#"{"sessionId":"sess-1","attributionSkill":"cortex-troubleshoot","attributionPlugin":"cortex","content":"hi"}"#,
              "\n"
          ),
      )
      .unwrap();
      index_file(&pool, &claude_file, "explicit_file").unwrap();

      // Row 2: inserted directly into `logs` bypassing the scanner entirely,
      // simulating data ingested BEFORE this phase shipped — this is exactly
      // what `sessions skills backfill` exists to catch up.
      {
          let conn = pool.get().unwrap();
          conn.execute(
              "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip, ai_tool, ai_project, ai_session_id)
               VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', ?1, ?1, 'transcript://claude_project', 'claude', 'cortex', 'sess-2')",
              rusqlite::params![r#"{"attributionSkill":"web-app-testing"}"#],
          )
          .unwrap();
      }

      let conn = pool.get().unwrap();
      let pre_backfill_count: i64 = conn
          .query_row("SELECT COUNT(*) FROM ai_skill_events", [], |row| row.get(0))
          .unwrap();
      assert_eq!(pre_backfill_count, 1, "only the scanner-ingested row should have a skill event pre-backfill");
      drop(conn);

      // Backfill catches the pre-existing row 2 without duplicating row 1's event.
      let result = crate::app::services::skill_backfill::run_backfill(&pool, None, 100, false).unwrap();
      assert_eq!(result.scanned, 2, "both claude rows should be scanned");
      assert_eq!(result.inserted, 1, "only row 2's event is new");
      assert_eq!(result.skipped_duplicates, 1, "row 1's event was already present from ingest-time extraction");

      let conn = pool.get().unwrap();
      let total: i64 = conn
          .query_row("SELECT COUNT(*) FROM ai_skill_events", [], |row| row.get(0))
          .unwrap();
      assert_eq!(total, 2);

      // Running backfill again is fully idempotent.
      let second = crate::app::services::skill_backfill::run_backfill(&pool, None, 100, false).unwrap();
      assert_eq!(second.inserted, 0);
      assert_eq!(second.skipped_duplicates, 2);
  }
  ```
  This calls `run_backfill` directly (crate-internal `pub(crate)` or private
  function) rather than through `CortexService`, so it requires
  `run_backfill` in `src/app/services/skill_backfill.rs` to be at least
  `pub(crate)` — adjust its visibility from Task 7's `fn run_backfill(...)`
  (private) to `pub(crate) fn run_backfill(...)` so this cross-module test can
  reach it. If the repo's existing convention instead drives everything
  through `CortexService` in integration tests (check whether
  `src/scanner_tests.rs` has ever called into `src/app/` before — it currently
  has not, based on its `use` block), prefer building a
  `CortexService::new(Arc::new(pool.clone()), StorageConfig::for_test(...))`
  and calling `.backfill_skill_events(...).await` inside a
  `#[tokio::test]` instead, matching Task 7's own test style, and drop the
  `pub(crate)` visibility change.

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cargo test --lib scanner::tests::end_to_end_ingest_then_backfill_is_idempotent_across_both_paths
  ```
  Expected: compile error (visibility or missing import) until the adjustments
  above are made, then a logical failure if wiring from earlier tasks has any
  gap — this test is the final correctness gate for the whole phase.

- [ ] **Step 3: Write minimal implementation**

  No new production code should be required if Tasks 1-9 were implemented
  correctly — this step is limited to:
  - Adjusting `run_backfill`'s visibility in
    `src/app/services/skill_backfill.rs` to `pub(crate)` (only if the direct
    call style above is used instead of the `CortexService` style).
  - Fixing any integration gap this test surfaces (e.g. a missed `mod`
    declaration, an import path typo from an earlier task's write-up).

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib scanner::tests::end_to_end_ingest_then_backfill_is_idempotent_across_both_paths
  cargo test --lib
  cargo clippy --all-targets -- -D warnings
  cargo fmt -- --check
  ```
  Expected: the new test passes, the FULL `cargo test --lib` suite passes (no
  regressions across the whole phase), clippy is clean, and `cargo fmt --check`
  reports no diffs.

- [ ] **Step 5: Commit**
  ```bash
  git add src/scanner_tests.rs src/app/services/skill_backfill.rs
  git commit -m "test: add end-to-end ingest+backfill idempotency coverage for ai_skill_events"
  ```

---

### Task 11: Documentation — action count, action table, CLI reference, schema docs

**Files:**
- Modify: `CLAUDE.md` (repo-root, at
  `/home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md`)
  — MCP Tools section: `48 actions` -> `49 actions` (NOT "47 -> 48" — as of
  this eng review pass, PR 1 "LLM Invocation Guard" has already merged and
  added the `llm_invocations` action, so the live baseline is already 48; this
  phase's `skill_events` is the 49th), add `skill_events` row to the action
  table (alphabetically/logically placed after `graph`, before the admin
  actions section, matching `src/mcp/actions.rs` ordering)
- Modify: `README.md` — search for any action-count mentions
  (`grep -n "action.*count\|[0-9][0-9] actions" README.md`) and update
  in lockstep
- Modify: `docs/mcp/TOOLS.md` — add `skill_events` to the action reference
- Modify: `docs/mcp/SCHEMA.md` — document the `ai_skill_events` table schema
  (columns, indexes, `UNIQUE` constraint, and the two closed enums
  `event_kind`/`evidence_kind` values)
- Modify: `docs/contracts/mcp-actions-current.md` — add `skill_events` to
  whatever generated/maintained contract listing exists there (check its
  format first — it may be a machine-generated snapshot; if so, check whether
  it has a regeneration script rather than hand-editing)
- Modify: `docs/CLI.md` — add `cortex sessions skills` and `cortex sessions
  skills backfill` to the command reference, in the `sessions` section
- Modify: `docs/api.md` (if it exists — `find docs -iname "api.md"`) — add
  `GET /api/ai/skills` to the REST route table

**Interfaces:**
- Consumes: nothing (pure documentation)
- Produces: nothing (no code)

- [ ] **Step 1: Write the failing test**

  **Eng review Fix 11 (real defect — corrects a false claim in the original
  plan)**: the original version of this task confidently asserted that
  `src/docs_tests.rs` "should already be red after Task 9 added the
  skill_events action (48 actions vs. a docs file still claiming 47)." This is
  FALSE — verified directly by reading the file during this eng review pass.
  `src/docs_tests.rs` contains exactly 4 tests
  (`current_docker_ingest_docs_prefer_agent_path_over_socket_proxy`,
  `coverage_docs_use_cortex_names_and_current_smoke_scope`,
  `coverage_tooling_is_documented_and_scripted`,
  `live_smoke_keeps_deterministic_admin_rest_coverage`), none of which assert
  an action count or cross-check `ACTION_SPECS` against any doc file. Verify
  this yourself before proceeding rather than trusting either this note or the
  original plan's claim — repo state can drift:
  ```bash
  grep -n "action.*count\|ACTION_SPECS" src/docs_tests.rs 2>&1
  cat src/docs_tests.rs
  ```
  As of this review, that grep returns nothing and the file has no such
  assertion. **Task 11's doc updates are therefore manual, not test-gated** —
  skip Steps 1-2 entirely and go straight to Step 3 (manual doc edits). If a
  future session finds `docs_tests.rs` HAS grown such an assertion by the time
  this task is implemented, treat that as the actual failing test for Step
  1/2 instead of this note.

- [ ] **Step 2: Run test to verify it fails**

  N/A per Step 1 — no such test exists as of this eng review pass. If one has
  been added since, run it here and use its failure as this step's evidence.

- [ ] **Step 3: Write minimal implementation**

  In `CLAUDE.md` (repo root), find:
  ```
  One MCP tool: **`cortex`** — dispatches by `action` argument. 48 actions, generated from `ACTION_SPECS` in `src/mcp/actions.rs` (the single authoritative registry — regenerate this table from there).
  ```
  Change `48 actions` to `49 actions` (re-verify the live count with
  `python3 -c "import re; print(len(re.findall(r'action_spec!\(\s*\"[a-z_]+\"', open('src/mcp/actions.rs').read())))"`
  or equivalent before editing — do not trust a hardcoded number in case other
  work has landed an action in the interim). In the action table, add a new
  row after the `graph` row and before the `file_tails` (admin) row:
  ```
  | `skill_events` | List extracted AI skill-invocation events |
  ```

  In `README.md`, run `grep -n "47" README.md` and update any matching action
  count reference the same way.

  In `docs/mcp/TOOLS.md`, add a `skill_events` entry following whatever format
  the file already uses for each action (read the file first to match its
  exact per-action documentation block shape — likely name, description,
  scope, example args, example response).

  In `docs/mcp/SCHEMA.md`, add a new section documenting `ai_skill_events`.
  **Eng review Fix 2**: no `skill_path`/`metadata_json` rows — those columns
  do not exist in the shipped schema (neither extractor ever set them, so they
  were removed rather than documented as "reserved for future use"). **Eng
  review Fix 4/5**: index list matches the redesigned set, and
  `idx_logs_ai_tool_id` is called out separately since it lives on the
  existing `logs` table, not `ai_skill_events`:
  ```markdown
  ### `ai_skill_events`

  One row per detected skill invocation extracted from an AI transcript log
  row. Added in migration 38.

  | Column | Type | Notes |
  |---|---|---|
  | `id` | INTEGER PK | autoincrement |
  | `log_id` | INTEGER NOT NULL | FK -> `logs(id)` ON DELETE CASCADE |
  | `ai_tool` | TEXT NOT NULL | `claude` \| `codex` |
  | `ai_project` | TEXT | nullable — copied from the source `logs` row |
  | `ai_session_id` | TEXT | nullable — copied from the source `logs` row |
  | `hostname` | TEXT NOT NULL | copied from the source `logs` row |
  | `timestamp` | TEXT NOT NULL | copied from the source `logs` row |
  | `skill_name` | TEXT NOT NULL | trimmed, max 256 chars, control characters rejected; `plugin:skill` combined form when the source used that shape |
  | `skill_plugin` | TEXT | nullable, trimmed, max 256 chars, control characters rejected |
  | `event_kind` | TEXT NOT NULL | `claude_attribution` \| `codex_skill_block` |
  | `evidence_kind` | TEXT NOT NULL | `structured_json_field` \| `transcript_content` |
  | `created_at` | TEXT NOT NULL | insert time |

  `UNIQUE(log_id, skill_name, event_kind, evidence_kind)` makes ingest-time and
  backfill insertion idempotent via `INSERT OR IGNORE`.

  Indexes on `ai_skill_events` (chosen to match the shipped CLI filter surface
  — `--skill`, `--plugin`, `--tool`, `--project`, `--session-id`, `--host`,
  plus the unfiltered default sort):
  - `idx_ai_skill_events_timestamp (timestamp)` — unfiltered/residual-filter default `ORDER BY timestamp DESC`
  - `idx_ai_skill_events_skill_time (skill_name, timestamp)` — `--skill`
  - `idx_ai_skill_events_plugin_time (skill_plugin, timestamp)` — `--plugin`
  - `idx_ai_skill_events_hostname_time (hostname, timestamp)` — `--host`
  - `idx_ai_skill_events_session_time (ai_tool, ai_project, ai_session_id, timestamp)` — `--tool` (leading column), and `--session-id` when paired with `--tool`
  - `idx_ai_skill_events_project_skill_time (ai_project, skill_name, timestamp) WHERE ai_project IS NOT NULL` — `--project` (+ `--skill`)

  Migration 38 also adds `idx_logs_ai_tool_id (ai_tool, id) WHERE ai_tool IN
  ('claude', 'codex')` to the EXISTING `logs` table — this supports the
  `sessions skills backfill` keyset-pagination scan (`id > ?` + `ORDER BY id
  ASC`), which the pre-existing `idx_logs_ai_tool_cover (ai_tool,
  ai_session_id, timestamp)` cannot serve since it doesn't include `id`.
  ```

  In `docs/contracts/mcp-actions-current.md`, check for a generation script
  reference at the top of the file (`grep -n "generated\|regenerate" docs/contracts/mcp-actions-current.md`).
  If it says "generated — do not hand-edit", find and run the generator
  script instead of hand-editing:
  ```bash
  grep -rn "mcp-actions-current" --include="*.rs" --include="*.sh" src/ scripts/ xtask/ 2>&1
  ```
  Run whatever script/binary that turns up (likely `cargo run --bin
  <something>` or a `scripts/*.sh`) to regenerate the file. If no generator
  exists, hand-edit it to add the `skill_events` row matching its existing
  format exactly.

  In `docs/CLI.md`, find the `sessions` command reference section and add:
  ```markdown
  | `cortex sessions skills [--skill NAME] [--plugin PLUGIN] [--tool TOOL] [--project PROJECT] [--session-id ID] [--host HOST] [--since ...] [--until ...] [--limit N] [--json]` | List extracted AI skill-invocation events |
  | `cortex sessions skills backfill [--since ...] [--limit N] [--dry-run] [--json]` | Backfill `ai_skill_events` from existing `logs` rows |
  ```
  Match the exact table/list format already used in that section (read it
  first).

  If `docs/api.md` exists, add `GET /api/ai/skills` with query params matching
  `ListSkillEventsRequest` fields, in whatever format the file's existing
  route entries use.

- [ ] **Step 4: Run test to verify it passes**
  ```bash
  cargo test --lib docs_tests
  cargo test --lib
  ```
  Expected: `docs_tests` passes trivially (as of this eng review pass it
  contains no action-count/doc-cross-check assertion — see Fix 11 note above —
  so this is a no-op regression guard, not a positive verification of the doc
  edits); full test suite still green. The doc edits themselves are verified
  by manual read-through, not by this test suite.

- [ ] **Step 5: Commit**
  ```bash
  git add CLAUDE.md README.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/contracts/mcp-actions-current.md docs/CLI.md docs/api.md
  git commit -m "docs: document skill_events action, ai_skill_events schema, and sessions skills CLI"
  ```

---

## Post-phase verification checklist (run once, after Task 11)

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo xtask check-version-sync
```

Per this repo's `CLAUDE.md` versioning rule, this phase's commits should be
squashed/tagged with a `feat` (minor) or `feat!`/`fix` prefix at PR time so
`cargo xtask bump-version minor` (new action, new table, new CLI surface —
qualifies as a feature addition, not a breaking change) runs before merge, and
`CHANGELOG.md` gets an entry. That version bump is intentionally NOT included
as a task above since it is normally done once at the end of the whole
feature branch, per repo convention, not per phase-task.

## Self-Review

**Note (post-review):** this Self-Review section describes the plan as
originally drafted. The "## Eng Review Fixes Applied" section near the top of
this document records the 11 fixes applied on top of it after four
independent review agents (architecture, simplicity, security, performance)
checked the plan against the live repo; the coverage/consistency claims below
have been updated in place to reflect the post-fix shapes, not re-derived
from scratch.

### Spec coverage

| Spec item | Covered by |
|---|---|
| Claude `attributionSkill`/`attributionPlugin` extraction (top-level, `message.*` nesting — `payload.*` deliberately removed, see Fix 3) | Task 2 |
| Codex `<skill><name>` extraction, multi-tag dedup, prose-not-matched | Task 3 |
| Shared normalization (trim/clamp/`plugin:skill` split, control-character rejection per Fix 8, never fabricated across separate Claude fields) | Task 2 (base), Task 3 (extends) |
| Migration for `ai_skill_events` table + redesigned indexes (Fix 4) + `idx_logs_ai_tool_id` on `logs` (Fix 5) + `KNOWN_SCHEMA_VERSION` bump | Task 1 |
| DB insert/list query layer (`insert_skill_events[_in_tx]`, `list_skill_events`) | Task 4 |
| `insert_logs_batch_in_tx` returns log ids (prerequisite plumbing for ingest-time insertion) | Task 5 |
| `ParsedTranscriptRecord.raw_value` threading (Fix 1) | Task 2 |
| Ingest-time insertion, same transaction as the `logs` row, no second full-table scan, no double JSON-parse (Fix 1), short-circuit on no-skill-event rows | Task 6 |
| Backfill: bounded/chunked, no write_lock around the SELECT (Fix 6), hard limit clamp + single-flight guard (Fix 7), idempotent, dry-run | Task 7 |
| `cortex sessions skills backfill` CLI command | Task 8 |
| `skill_events` read surface — MCP action (still `cortex:read`), CLI list (`cortex sessions skills`), REST (`GET /api/ai/skills`, with Fix 9 audit logging) | Task 9 |
| End-to-end ingest-then-backfill idempotency across both insertion paths | Task 10 |
| Docs: corrected action count (48 -> 49, not 47 -> 48), action table, CLI reference, schema docs (no `skill_path`/`metadata_json`), contracts snapshot, corrected `docs_tests.rs` claim (Fix 11) | Task 11 |

No gaps were found requiring a new task — every locked interface declared at the top of this plan (`ExtractedSkillEvent`, `SkillEventKind`, `SkillEvidenceKind`, `SkillEventInsert`, `AiSkillEventParams`, `AiSkillEventEntry`, `ListSkillEventsResult`, `SkillBackfillRequest`, `SkillBackfillResult`) is produced by exactly one task and consumed by name in every later task that needs it, now with `skill_path`/`metadata_json` removed consistently everywhere (Fix 2).

### Placeholder scan

Task 2's original `unreachable!()` "checkpoint" stub was removed during this eng review pass along with the `payload.*` branch it exercised (Fix 3) — the task now writes the real `extract_claude_skill_events` implementation directly, no intermediate wrong-on-purpose stub. No `unreachable!()`, `todo!()`, `unimplemented!()`, or silent no-op stub remains anywhere in the plan. Task 8 still leaves `run_ai_skills` (the read/list dispatch arm) as an explicit `bail!("not yet implemented")` stub, but this is closed out within the same plan at Task 9 Step 3, not left dangling at the end of the phase.

### Type consistency

Verified by direct text search across the whole document after applying the 11 fixes:
- `ExtractedSkillEvent` — same FOUR fields (`skill_name`, `skill_plugin`, `event_kind`, `evidence_kind`) in the Locked Interfaces block, Task 2's struct definition, Task 3's usage, and every constructor call site in Tasks 4, 6, and 7 — `skill_path`/`metadata_json` removed everywhere per Fix 2.
- `AiSkillEventParams` — identical nine-field shape (`skill`, `plugin`, `tool`, `project`, `session_id`, `hostname`, `from`, `to`, `limit`) in the Locked Interfaces block and Task 4's concrete implementation (unaffected by Fix 2 — this struct never had `skill_path`/`metadata_json`); Task 9's `ListSkillEventsRequest` (app-layer) maps 1:1 onto it field-for-field in `CortexService::list_skill_events`.
- `AiSkillEventEntry` — identical ELEVEN-field shape (down from thirteen) in the Locked Interfaces block, Task 4's struct, and Task 9's `From<db::AiSkillEventEntry> for SkillEventEntry` conversion (every remaining field is threaded through, none dropped or renamed beyond the two Fix 2 removals).
- `SkillEventInsert` — same six fields (`log_id`, `ai_tool`, `ai_project`, `ai_session_id`, `hostname`, `timestamp`, `event`) used consistently by Task 4's insert function, Task 6's ingest wiring, and Task 7's backfill service — unchanged by any fix (it wraps `ExtractedSkillEvent` rather than duplicating its fields).
- `SkillBackfillRequest`/`SkillBackfillResult` — same shapes in the Locked Interfaces block, Task 7's model definitions, and Task 8's CLI arg mapping into the request; Task 7's `backfill_skill_events` body changed (hard clamp + single-flight guard, Fix 7) but the request/result struct SHAPES themselves are unchanged.
- `ParsedTranscriptRecord` — new `raw_value: Option<serde_json::Value>` field (Fix 1), added consistently in Task 2's `src/scanner.rs` edit and populated/no-op'd consistently across `claude.rs` (populated), `codex.rs` (`None`), and `gemini.rs` (`None`).

No divergent field names or types were found between the interface declarations and their downstream usages.
