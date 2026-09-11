# `cortex reflect` Implementation Plan (revised after engineering review)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local-only `cortex reflect` command that indexes local AI transcripts, finds skill, MCP, and hook incidents, ranks them together, optionally LLM-assesses the top N, and prints one Markdown or JSON report on stdout.

**Architecture:** `reflect` orchestrates existing code. It lists incidents with the existing list services, then investigates each top incident once (incident id plus its exact target), takes the deterministic findings from that evidence, and hands the evidence to the kind's existing per-evidence LLM helper. Existing-code changes are limited to an optional `incident_id` on the three assess requests (for the report's "assess this" commands), `pub(super)` on the three per-evidence LLM helpers, and a retrying query-only runtime constructor.

**Tech Stack:** Rust 2024 (MSRV 1.97.1), tokio, rusqlite/r2d2, serde/serde_json, chrono. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-11-cortex-reflect-design.md` (revised after review)
**Tracking:** beads `unraid-mcp-nh9s`. Deferred follow-ups: `unraid-mcp-360f` (fair share across kinds), `unraid-mcp-9b2h` (set-based anchor queries), `unraid-mcp-psvk` (index budget and live progress).

## Global Constraints

- Kinds are exactly `skill`, `mcp`, `hook`.
- Defaults: `--since 7d`, `--max-assess 5`, database `~/.cortex/reflect.db`.
- Database resolution: `--db PATH`, then non-empty `CORTEX_DB_PATH` (with a stderr warning), then `~/.cortex/reflect.db` (parent created 0700 when missing). The database file and its `-wal`/`-shm` siblings are set 0600 on Unix.
- No `--out` flag. Report on stdout, progress and warnings on stderr.
- `reflect` is local-only. `--http`, `--server`, `--token`, and `CORTEX_USE_HTTP=1` are rejected.
- `--max-assess N` sets how many top incidents get a detailed section; `0` means summary and table only.
- `until` is pinned to the run start when not given.
- Exit code is 0 whenever a report is produced, including partial LLM failure and incidents that change mid-run.
- No new detectors, scoring, or prompts. All `LlmRunner` guards stay in force.
- Sibling `foo.rs` modules, never `foo/mod.rs`. Sidecar `*_tests.rs` wired with `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`.
- Every Rust module under 500 lines (lefthook gate).
- Read env vars through `crate::env::var_os` (library) or `cortex::env::var_os` (binary and integration tests).
- `cargo fmt` and `cargo clippy --all-targets` pass before each commit.
- Conventional Commits. Every commit ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

## Review changes applied

| # | Review recommendation | Where |
|---|---|---|
| 1 | Investigate each top incident once with exact target filters; call the per-evidence LLM helper; keep findings when the LLM fails | Task 3 |
| 2 | Pin `until` at run start | Task 3 |
| 3 | An incident that no longer resolves is a per-incident failure, not an abort | Task 3 |
| 4 | List limit 100 (the real clamp); carry `total_incidents` and truncation into the summary | Tasks 2, 3, 4 |
| 5 | Index with `since` as the file-age filter; progress before and after | Task 3 |
| 6 | LLM "action disabled" and "circuit open" block only that kind | Task 3 |
| 7 | Mode is `report_only` when no LLM output was produced because of a fallback | Task 3 |
| 8 | Preflight checks the exact program string, no whitespace split | Task 3 |
| 9 | Collapse the three per-kind arms with local macros | Task 3 |
| 10 | Drop `--out` | Task 5, spec |
| 11 | Owner-only `~/.cortex` (when created) and database files | Task 5 |
| 12 | Warn when `CORTEX_DB_PATH` is used | Task 5 |
| 13 | Strip control characters from terminal-bound fields | Task 4 |
| 14 | Heading demotion skips code fences | Task 4 |
| 15 | Consistent table-cell escaping | Task 4 |
| 16 | State that scores are heuristic | Task 4 |
| 17 | End-to-end tests for a failing and a missing LLM program | Task 6 |

Skipped with reason: reusing `IndexResult` in the report (it carries internal scan counters; a 4-field summary keeps the JSON stable); consolidating the existing private seeding helpers in `src/db/*_incidents_tests.rs` (out of scope); moving the end-to-end tests into an existing test binary (small link cost; a dedicated file keeps them findable); the "raw transcript leak via findings" finding (verified false: findings are derived analysis, and ingest scrubs secrets).

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/app/models/{skill,mcp,hook}_assess.rs` | Modify | Optional `incident_id` |
| `src/app/services/{skill,mcp,hook}_assessment.rs` | Modify | Forward `incident_id`; relax target guard; `pub(super)` on `run_one_*` |
| `src/app/services/seed_test_support.rs` | Create | Test-only seeding helpers |
| `src/cli/args/assess.rs`, `src/cli/parse/assess.rs`, `src/cli/dispatch_sessions.rs`, `src/cli/help.rs` | Modify | `--incident-id` on `assess skill/mcp/hooks` |
| `src/app/models/reflect.rs` (+ `_tests.rs`) | Create | Types, conversions, ranking, summary |
| `src/app/services/reflect_llm.rs` (+ `_tests.rs`) | Create | LLM-failure classifier, program lookup |
| `src/app/services/reflect.rs` (+ `_tests.rs`) | Create | `CortexService::run_reflect` |
| `src/app/reflect_report.rs` (+ `_tests.rs`) | Create | Markdown renderer |
| `src/runtime.rs` | Modify | `RuntimeCore::query_only_with_retry` |
| `src/cli/args/reflect.rs`, `src/cli/parse/reflect.rs` (+ `_tests.rs`) | Create | Args and parser |
| `src/cli/dispatch_reflect.rs` (+ `_tests.rs`) | Create | Dispatch, DB path resolution, permissions |
| `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/run.rs`, `src/cli.rs`, `src/main.rs`, `src/surfaces.rs`, `src/cli/help_tests.rs` | Modify | Wiring |
| `tests/reflect_cli.rs` | Create | End-to-end binary tests |
| `README.md`, `docs/runbooks/skill-reflection.md`, `CLAUDE.md`, `Justfile` | Modify | Docs, `just reflect` |

---

### Task 1: Target a single incident in `assess skill|mcp|hooks`

The report prints `cortex assess <kind> --incident-id ID` for every unassessed incident. This task makes that command work.

**Files:**
- Modify: `src/app/models/skill_assess.rs`, `src/app/models/mcp_assess.rs`, `src/app/models/hook_assess.rs`
- Modify: `src/app/services/skill_assessment.rs`, `src/app/services/mcp_assessment.rs`, `src/app/services/hook_assessment.rs`
- Create: `src/app/services/seed_test_support.rs`; Modify: `src/app/services.rs`
- Modify: `src/cli/args/assess.rs`, `src/cli/parse/assess.rs`, `src/cli/dispatch_sessions.rs`, `src/cli/help.rs`
- Test: `src/app/services/{skill,mcp,hook}_assessment_tests.rs`, `src/cli/parse/assess_tests.rs`

**Interfaces:**
- Produces: `incident_id: Option<String>` on `SkillAssessRequest`, `McpAssessRequest`, `HookAssessRequest`.
- Produces (test-only): `crate::app::services::seed_test_support::{ts_minutes_ago, seed_skill_incident, seed_mcp_incident, seed_hook_incident}`.
- Produces: `--incident-id ID` on `cortex assess skill|mcp|hooks`.

- [ ] **Step 1: Create the shared seeding helpers**

Create `src/app/services/seed_test_support.rs`:

```rust
//! Test-only seeding helpers shared by the assessment and reflect service
//! tests. They mirror the private helpers in `src/db/*_incidents_tests.rs`
//! but use timestamps relative to now, so default time windows include them.

use crate::db::{DbPool, LogBatchEntry, insert_logs_batch};

pub(crate) const HOST: &str = "devhost";
pub(crate) const PROJECT: &str = "/tmp/reflect-project";

pub(crate) fn ts_minutes_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn ai_entry(ts: &str, tool: &str, session_id: &str, message: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: HOST.to_string(),
        facility: Some("local0".to_string()),
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: message.to_string(),
        source_ip: "127.0.0.1:514".to_string(),
        docker_checkpoint: None,
        ai_tool: Some(tool.to_string()),
        ai_project: Some(PROJECT.to_string()),
        ai_session_id: Some(session_id.to_string()),
        ai_transcript_path: Some(format!("{PROJECT}/{session_id}.jsonl")),
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

/// Inserts one log row and returns its id.
fn insert_one(pool: &DbPool, entry: LogBatchEntry) -> i64 {
    insert_logs_batch(pool, std::slice::from_ref(&entry)).unwrap();
    pool.get()
        .unwrap()
        .query_row("SELECT MAX(id) FROM logs", [], |row| row.get(0))
        .unwrap()
}

/// A skill load followed two minutes later by a user correction.
/// `minutes_ago` must be at least 3.
pub(crate) fn seed_skill_incident(pool: &DbPool, session_id: &str, skill: &str, minutes_ago: i64) {
    let skill_ts = ts_minutes_ago(minutes_ago);
    let log_id = insert_one(
        pool,
        ai_entry(&skill_ts, "codex", session_id, &format!("loaded skill {skill}")),
    );
    insert_one(
        pool,
        ai_entry(
            &ts_minutes_ago(minutes_ago - 2),
            "codex",
            session_id,
            "That's not what I asked for, please redo it.",
        ),
    );
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO ai_skill_events
                (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                 skill_name, skill_plugin, event_kind, evidence_kind, created_at)
             VALUES (?1, 'codex', ?2, ?3, ?4, ?5, ?6, NULL, 'skill_invoked', 'transcript', ?5)",
            rusqlite::params![log_id, PROJECT, session_id, HOST, skill_ts, skill],
        )
        .unwrap();
}

/// Two failing calls to the same MCP tool. `minutes_ago` must be at least 2.
pub(crate) fn seed_mcp_incident(
    pool: &DbPool,
    session_id: &str,
    server: &str,
    tool: &str,
    minutes_ago: i64,
) {
    for (offset, call_id) in [(0, "call-1"), (1, "call-2")] {
        let ts = ts_minutes_ago(minutes_ago - offset);
        let log_id = insert_one(
            pool,
            ai_entry(&ts, "claude", session_id, "Error: connection refused"),
        );
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO ai_mcp_events
                    (call_log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                     call_id, tool_name, mcp_server, mcp_tool, event_kind, is_error, created_at)
                 VALUES (?1, 'claude', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'call', 1, ?5)",
                rusqlite::params![
                    log_id,
                    PROJECT,
                    session_id,
                    HOST,
                    ts,
                    call_id,
                    format!("mcp__{server}__{tool}"),
                    server,
                    tool,
                ],
            )
            .unwrap();
    }
}

/// Two failed runs of the same hook. `minutes_ago` must be at least 1.
pub(crate) fn seed_hook_incident(pool: &DbPool, session_id: &str, hook_name: &str, minutes_ago: i64) {
    for offset in [0, 1] {
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO ai_hook_events
                    (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                     hook_event, hook_name, status, duration_ms, evidence_kind)
                 VALUES (NULL, 'claude', ?1, ?2, ?3, ?4, 'PostToolUse', ?5, 'failed', NULL,
                         'runtime_transcript')",
                rusqlite::params![PROJECT, session_id, HOST, ts_minutes_ago(minutes_ago - offset), hook_name],
            )
            .unwrap();
    }
}
```

In `src/app/services.rs`, add after `mod rag;`:

```rust
#[cfg(test)]
mod seed_test_support;
```

- [ ] **Step 2: Write the failing service tests**

Append to `src/app/services/skill_assessment_tests.rs`:

```rust
#[tokio::test]
async fn incident_id_targets_exactly_one_skill_incident() {
    use crate::app::models::AiSkillIncidentRequest;
    use crate::app::services::seed_test_support::seed_skill_incident;

    let (service, pool, _dir) = test_service();
    seed_skill_incident(&pool, "sess-a", "alpha-skill", 60);
    seed_skill_incident(&pool, "sess-b", "beta-skill", 30);
    let listed = service
        .list_ai_skill_incidents(AiSkillIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_skill_assessment_with_delta(
            SkillAssessRequest {
                incident_id: Some(target.clone()),
                ..Default::default()
            },
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].incident_id, target);
}
```

Append to `src/app/services/mcp_assessment_tests.rs`:

```rust
#[tokio::test]
async fn incident_id_targets_exactly_one_mcp_incident() {
    use crate::app::models::{AiMcpIncidentRequest, McpAssessRequest};
    use crate::app::services::seed_test_support::seed_mcp_incident;

    let (service, pool, _dir) = test_service();
    seed_mcp_incident(&pool, "sess-a", "labby", "search", 60);
    seed_mcp_incident(&pool, "sess-b", "unifi", "clients", 30);
    let listed = service
        .list_ai_mcp_incidents(AiMcpIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_mcp_assessment_with_delta(
            McpAssessRequest {
                incident_id: Some(target.clone()),
                ..Default::default()
            },
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].incident_id, target);
}
```

Append to `src/app/services/hook_assessment_tests.rs`:

```rust
#[tokio::test]
async fn incident_id_targets_exactly_one_hook_incident() {
    use crate::app::models::{AiHookIncidentRequest, HookAssessRequest};
    use crate::app::services::seed_test_support::seed_hook_incident;

    let (service, pool, _dir) = test_service();
    seed_hook_incident(&pool, "sess-a", "format-on-save", 60);
    seed_hook_incident(&pool, "sess-b", "lint-on-stop", 30);
    let listed = service
        .list_ai_hook_incidents(AiHookIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_hook_assessment_with_delta(
            HookAssessRequest {
                incident_id: Some(target.clone()),
                ..Default::default()
            },
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].incident_id, target);
}
```

If `mcp_assessment_tests.rs` or `hook_assessment_tests.rs` has no `test_service()` helper, add this at the top of that file:

```rust
fn test_service() -> (CortexService, std::sync::Arc<crate::db::DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(dir.path().join("assess-test.db"));
    let pool = std::sync::Arc::new(crate::db::init_pool(&storage).unwrap());
    (CortexService::new(std::sync::Arc::clone(&pool), storage), pool, dir)
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib incident_id_targets_exactly_one`
Expected: compile error, "struct `SkillAssessRequest` has no field named `incident_id`".

- [ ] **Step 4: Add `incident_id` to the three request models**

In `SkillAssessRequest`, `McpAssessRequest`, and `HookAssessRequest`, add as the first field:

```rust
    /// Assess exactly this incident (as returned by the matching
    /// `*_incidents` listing). When set, the target fields may be empty.
    /// Skipped when `None` for the same serde_qs reason as
    /// `AiSkillInvestigateRequest::incident_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
```

- [ ] **Step 5: Forward `incident_id` in the three services**

In `src/app/services/skill_assessment.rs`, `run_skill_assessment_with_delta`, replace the guard:

```rust
        if req.incident_id.is_none() && req.skill.is_none() && req.plugin.is_none() {
            return Err(ServiceError::InvalidInput(
                "assess skill requires a skill name, --plugin, or --incident-id".to_string(),
            ));
        }
```

In the `AiSkillInvestigateRequest` literal, replace `incident_id: None,` with `incident_id: req.incident_id.clone(),`. In the "no skill incident found" branch:

```rust
            let skill_desc = req
                .incident_id
                .clone()
                .or_else(|| req.skill.clone())
                .or_else(|| req.plugin.clone().map(|p| format!("plugin:{p}")))
                .unwrap_or_default();
```

In `src/app/services/mcp_assessment.rs`, `run_mcp_assessment_with_delta`, replace the guard:

```rust
        if req.incident_id.is_none()
            && req.mcp_server.is_none()
            && req.mcp_tool.is_none()
            && req.tool_name.is_none()
        {
            return Err(ServiceError::InvalidInput(
                "assess mcp requires an mcp_server, mcp_tool, tool_name, or incident_id".to_string(),
            ));
        }
```

Replace `incident_id: None,` in its `AiMcpInvestigateRequest` literal with `incident_id: req.incident_id.clone(),`, and make `target_desc` start with `req.incident_id.clone().or_else(|| req.mcp_server.clone())`.

In `src/app/services/hook_assessment.rs`, `run_hook_assessment_with_delta`: replace `incident_id: None,` in its `AiHookInvestigateRequest` literal with `incident_id: req.incident_id.clone(),`, and make `hook_desc` start with `req.incident_id.clone().or_else(|| req.hook_name.clone())`.

- [ ] **Step 6: Fix every existing struct literal**

Run: `grep -rn 'SkillAssessRequest {\|McpAssessRequest {\|HookAssessRequest {' src tests`
For every literal that lists all fields without `..Default::default()`, add `incident_id: None,`. Leave the three new tests alone.

- [ ] **Step 7: Run the service tests**

Run: `cargo test --lib assessment`
Expected: PASS, including the three new tests.

- [ ] **Step 8: Write the failing CLI parser tests**

Append to `src/cli/parse/assess_tests.rs` (add `use super::super::super::args::{AssessCommand, CliCommand};` at the top if the file does not already import them):

```rust
#[test]
fn assess_skill_accepts_incident_id_without_skill_name() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Skill(parsed)) = parse_assess_skill_from(&args).unwrap() else {
        panic!("expected assess skill");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
    assert_eq!(parsed.skill, None);
}

#[test]
fn assess_mcp_accepts_incident_id_without_target() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Mcp(parsed)) = parse_assess_mcp_from(&args).unwrap() else {
        panic!("expected assess mcp");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
}

#[test]
fn assess_hooks_accepts_incident_id() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Hooks(parsed)) = parse_assess_hooks(&args).unwrap() else {
        panic!("expected assess hooks");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
}
```

- [ ] **Step 9: Run the parser tests to verify they fail**

Run: `cargo test --bin cortex assess_`
Expected: compile error, no field `incident_id` on `AssessSkillArgs`.

- [ ] **Step 10: Add the flag**

In `src/cli/args/assess.rs`, add `pub incident_id: Option<String>,` as the first field of `AssessSkillArgs`, `AssessMcpArgs`, and `AssessHooksArgs`.

In `src/cli/parse/assess.rs`:
- In `parse_assess_skill_from`, `parse_assess_mcp_from`, and `parse_assess_hooks`, add the arm `"--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),` and add `"--incident-id"` to each `suggest::unknown_option` list.
- Skill guard: `if parsed.skill.is_none() && parsed.plugin.is_none() && parsed.incident_id.is_none() {`, message `"assess skill: skill name, --plugin, or --incident-id is required, e.g. ..."` keeping the existing examples.
- MCP guard: `if parsed.target.is_none() && parsed.server.is_none() && parsed.tool_name.is_none() && parsed.incident_id.is_none() {`.

In `src/cli/dispatch_sessions.rs`, add `incident_id: args.incident_id.clone(),` to the request literals in `run_assess_skill`, `run_assess_mcp`, and `run_assess_hooks`.

In `src/cli/help.rs`, in the `assess` `CommandDoc`, insert `[--incident-id ID] ` after `SKILL ` in the first skill line and after `hooks ` in the hooks line, and add after the two skill lines:

```rust
            "cortex assess skill --incident-id ID [--since TIME] [--until TIME] [--no-llm] [--json]",
```

- [ ] **Step 11: Run the CLI tests**

Run: `cargo test --bin cortex assess`
Expected: PASS.

- [ ] **Step 12: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/app src/cli
git commit -m "feat(assess): target a single incident with --incident-id

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Reflect model types, ranking, and summary

**Files:**
- Create: `src/app/models/reflect.rs`, `src/app/models/reflect_tests.rs`
- Modify: `src/app/models.rs`, `src/app.rs`

**Interfaces:**
- Consumes: `SkillIncident`, `McpIncident`, `HookIncident` (in scope via `use super::*`), `crate::scanner::IndexResult`.
- Produces (all `pub`, re-exported from `cortex::app`):
  - `enum ReflectKind { Skill, Mcp, Hook }`: `ALL`, `as_str`, `parse`, `assess_subcommand`. Lowercase serde. Derives `Ord` (used as a `BTreeMap` key).
  - `struct ReflectRequest { since, until, project, tool: Option<String>, kinds: Vec<ReflectKind>, run_llm: bool, max_assess: u32, index: bool }`
  - `struct ReflectIncident { kind, incident_id, target, target_key: String, target_detail: Option<String>, tool, project, session_id, last_seen: String, priority_score: f64, priority_label: String, signals_present: Vec<String> }` with `assess_command(&self, since: Option<&str>, until: Option<&str>) -> String` and `From<SkillIncident | McpIncident | HookIncident>`. `target_key`/`target_detail` are the exact grouping keys: skill name/plugin, MCP server/tool, hook event/name.
  - `struct ReflectKindListing { kind, total: usize, truncated: bool }`
  - `struct ReflectKindSummary { kind, listed, total: usize, truncated: bool, critical, high, medium, low: usize }`
  - `enum ReflectMode { ReportOnly, ReportAndLlm }` (snake_case)
  - `struct ReflectIndexSummary { discovered_files, ingested, skipped_dupes, parse_errors: usize }` with `From<&IndexResult>`
  - `struct ReflectAssessed { incident, assessment: Option<String>, findings: serde_json::Value, failure: Option<String> }`
  - `struct ReflectReport { since, until, project, tool: Option<String>, kinds, db_path: String, mode, llm_fallback_reason: Option<String>, index: Option<ReflectIndexSummary>, summary: Vec<ReflectKindSummary>, assessed: Vec<ReflectAssessed>, unassessed: Vec<ReflectIncident> }`
  - `fn rank_reflect_incidents(Vec<ReflectIncident>) -> Vec<ReflectIncident>`
  - `fn summarize_reflect_incidents(&[ReflectKindListing], &[ReflectIncident]) -> Vec<ReflectKindSummary>`

- [ ] **Step 1: Write the failing tests**

Create `src/app/models/reflect_tests.rs`:

```rust
use super::*;

fn incident(kind: ReflectKind, id: &str, score: f64, label: &str, last_seen: &str) -> ReflectIncident {
    ReflectIncident {
        kind,
        incident_id: id.to_string(),
        target: format!("target-{id}"),
        target_key: format!("key-{id}"),
        target_detail: None,
        tool: "claude".to_string(),
        project: "/p".to_string(),
        session_id: "s".to_string(),
        last_seen: last_seen.to_string(),
        priority_score: score,
        priority_label: label.to_string(),
        signals_present: vec![],
    }
}

#[test]
fn kind_parse_round_trips_and_rejects_others() {
    for kind in ReflectKind::ALL {
        assert_eq!(ReflectKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(ReflectKind::parse("abuse"), None);
    assert_eq!(ReflectKind::parse(""), None);
}

#[test]
fn hook_assess_subcommand_is_plural() {
    assert_eq!(ReflectKind::Skill.assess_subcommand(), "skill");
    assert_eq!(ReflectKind::Mcp.assess_subcommand(), "mcp");
    assert_eq!(ReflectKind::Hook.assess_subcommand(), "hooks");
}

#[test]
fn enums_serialize_as_documented() {
    assert_eq!(serde_json::to_string(&ReflectKind::Mcp).unwrap(), "\"mcp\"");
    assert_eq!(serde_json::to_string(&ReflectMode::ReportAndLlm).unwrap(), "\"report_and_llm\"");
}

#[test]
fn rank_orders_by_score_then_recency_then_id() {
    let ranked = rank_reflect_incidents(vec![
        incident(ReflectKind::Hook, "c", 10.0, "low", "2026-09-10T00:00:00Z"),
        incident(ReflectKind::Skill, "b", 40.0, "high", "2026-09-09T00:00:00Z"),
        incident(ReflectKind::Mcp, "a", 40.0, "high", "2026-09-09T00:00:00Z"),
        incident(ReflectKind::Mcp, "d", 40.0, "high", "2026-09-10T00:00:00Z"),
    ]);
    let ids: Vec<&str> = ranked.iter().map(|i| i.incident_id.as_str()).collect();
    assert_eq!(ids, ["d", "a", "b", "c"]);
}

#[test]
fn summary_counts_listed_totals_and_truncation_per_kind() {
    let incidents = vec![
        incident(ReflectKind::Skill, "a", 70.0, "critical", "t"),
        incident(ReflectKind::Skill, "b", 20.0, "medium", "t"),
        incident(ReflectKind::Hook, "c", 5.0, "low", "t"),
    ];
    let listings = [
        ReflectKindListing { kind: ReflectKind::Skill, total: 250, truncated: true },
        ReflectKindListing { kind: ReflectKind::Mcp, total: 0, truncated: false },
        ReflectKindListing { kind: ReflectKind::Hook, total: 1, truncated: false },
    ];
    let summary = summarize_reflect_incidents(&listings, &incidents);
    assert_eq!(summary.len(), 3);
    let skill = &summary[0];
    assert_eq!((skill.kind, skill.listed, skill.total, skill.truncated), (ReflectKind::Skill, 2, 250, true));
    assert_eq!((skill.critical, skill.medium), (1, 1));
    assert_eq!((summary[1].listed, summary[1].total), (0, 0));
    assert_eq!((summary[2].listed, summary[2].total, summary[2].low), (1, 1, 1));
}

#[test]
fn summary_total_is_never_below_listed() {
    let incidents = vec![incident(ReflectKind::Mcp, "a", 1.0, "low", "t")];
    let listings = [ReflectKindListing { kind: ReflectKind::Mcp, total: 0, truncated: false }];
    assert_eq!(summarize_reflect_incidents(&listings, &incidents)[0].total, 1);
}

#[test]
fn assess_command_includes_the_window() {
    let hook = incident(ReflectKind::Hook, "abc", 1.0, "low", "t");
    assert_eq!(hook.assess_command(None, None), "cortex assess hooks --incident-id abc");
    assert_eq!(
        hook.assess_command(Some("S"), Some("U")),
        "cortex assess hooks --incident-id abc --since S --until U"
    );
}
```

- [ ] **Step 2: Implement the module**

Create `src/app/models/reflect.rs`:

```rust
//! Types for `cortex reflect`: one merged, ranked view over skill, MCP, and
//! hook incidents, plus the report the CLI renders.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReflectKind {
    Skill,
    Mcp,
    Hook,
}

impl ReflectKind {
    pub const ALL: [ReflectKind; 3] = [ReflectKind::Skill, ReflectKind::Mcp, ReflectKind::Hook];

    pub fn as_str(self) -> &'static str {
        match self {
            ReflectKind::Skill => "skill",
            ReflectKind::Mcp => "mcp",
            ReflectKind::Hook => "hook",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "skill" => Some(ReflectKind::Skill),
            "mcp" => Some(ReflectKind::Mcp),
            "hook" => Some(ReflectKind::Hook),
            _ => None,
        }
    }

    /// The `cortex assess <subcommand>` that assesses this kind.
    pub fn assess_subcommand(self) -> &'static str {
        match self {
            ReflectKind::Skill => "skill",
            ReflectKind::Mcp => "mcp",
            ReflectKind::Hook => "hooks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectRequest {
    pub since: Option<String>,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub run_llm: bool,
    pub max_assess: u32,
    pub index: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectIncident {
    pub kind: ReflectKind,
    pub incident_id: String,
    /// Display form: `plugin:skill`, `server/tool`, or `event:hook`.
    pub target: String,
    /// Exact grouping key: skill name, MCP server, or hook event.
    pub target_key: String,
    /// Optional second key: skill plugin, MCP tool, or hook name.
    pub target_detail: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
    pub signals_present: Vec<String>,
}

impl ReflectIncident {
    /// The command that assesses this incident on its own.
    pub fn assess_command(&self, since: Option<&str>, until: Option<&str>) -> String {
        let mut command = format!(
            "cortex assess {} --incident-id {}",
            self.kind.assess_subcommand(),
            self.incident_id
        );
        for (flag, value) in [("--since", since), ("--until", until)] {
            if let Some(value) = value {
                command.push(' ');
                command.push_str(flag);
                command.push(' ');
                command.push_str(value);
            }
        }
        command
    }
}

fn joined(first: &str, separator: char, second: Option<&str>) -> String {
    match second {
        Some(second) => format!("{first}{separator}{second}"),
        None => first.to_string(),
    }
}

impl From<SkillIncident> for ReflectIncident {
    fn from(incident: SkillIncident) -> Self {
        // Display puts the plugin first: `plugin:skill`.
        let target = match &incident.skill_plugin {
            Some(plugin) => format!("{plugin}:{}", incident.skill_name),
            None => incident.skill_name.clone(),
        };
        Self {
            kind: ReflectKind::Skill,
            incident_id: incident.incident_id,
            target,
            target_key: incident.skill_name,
            target_detail: incident.skill_plugin,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

impl From<McpIncident> for ReflectIncident {
    fn from(incident: McpIncident) -> Self {
        let target = joined(&incident.mcp_server, '/', incident.mcp_tool.as_deref());
        Self {
            kind: ReflectKind::Mcp,
            incident_id: incident.incident_id,
            target,
            target_key: incident.mcp_server,
            target_detail: incident.mcp_tool,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

impl From<HookIncident> for ReflectIncident {
    fn from(incident: HookIncident) -> Self {
        let target = joined(&incident.hook_event, ':', incident.hook_name.as_deref());
        Self {
            kind: ReflectKind::Hook,
            incident_id: incident.incident_id,
            target,
            target_key: incident.hook_event,
            target_detail: incident.hook_name,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectKindListing {
    pub kind: ReflectKind,
    /// `total_incidents` from the list service (before its 100-row clamp).
    pub total: usize,
    /// True when the list or its candidate window was capped.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectKindSummary {
    pub kind: ReflectKind,
    pub listed: usize,
    pub total: usize,
    pub truncated: bool,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectMode {
    ReportOnly,
    ReportAndLlm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectIndexSummary {
    pub discovered_files: usize,
    pub ingested: usize,
    pub skipped_dupes: usize,
    pub parse_errors: usize,
}

impl From<&crate::scanner::IndexResult> for ReflectIndexSummary {
    fn from(result: &crate::scanner::IndexResult) -> Self {
        Self {
            discovered_files: result.discovered_files,
            ingested: result.ingested,
            skipped_dupes: result.skipped_dupes,
            parse_errors: result.parse_errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectAssessed {
    pub incident: ReflectIncident,
    /// LLM assessment Markdown. `None` in report-only mode or on failure.
    pub assessment: Option<String>,
    /// The kind's deterministic findings; `null` when the incident no
    /// longer resolved.
    pub findings: serde_json::Value,
    /// Why the LLM assessment or findings are missing.
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectReport {
    pub since: Option<String>,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub db_path: String,
    pub mode: ReflectMode,
    pub llm_fallback_reason: Option<String>,
    /// `None` when indexing was skipped with `--no-index`.
    pub index: Option<ReflectIndexSummary>,
    pub summary: Vec<ReflectKindSummary>,
    pub assessed: Vec<ReflectAssessed>,
    pub unassessed: Vec<ReflectIncident>,
}

/// Highest score first, then most recent, then incident id for a stable order.
pub fn rank_reflect_incidents(mut incidents: Vec<ReflectIncident>) -> Vec<ReflectIncident> {
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.incident_id.cmp(&b.incident_id))
    });
    incidents
}

/// One row per listing, in listing order, including empty kinds.
pub fn summarize_reflect_incidents(
    listings: &[ReflectKindListing],
    incidents: &[ReflectIncident],
) -> Vec<ReflectKindSummary> {
    listings
        .iter()
        .map(|listing| {
            let mut summary = ReflectKindSummary {
                kind: listing.kind,
                listed: 0,
                total: 0,
                truncated: listing.truncated,
                critical: 0,
                high: 0,
                medium: 0,
                low: 0,
            };
            for incident in incidents.iter().filter(|incident| incident.kind == listing.kind) {
                summary.listed += 1;
                match incident.priority_label.as_str() {
                    "critical" => summary.critical += 1,
                    "high" => summary.high += 1,
                    "medium" => summary.medium += 1,
                    _ => summary.low += 1,
                }
            }
            summary.total = listing.total.max(summary.listed);
            summary
        })
        .collect()
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
```

In `src/app/models.rs`, add `mod reflect;` next to `mod skill_assess;` and `pub use reflect::*;` next to the other `pub use ...::*;` lines.

In `src/app.rs`, add to the `pub use models::{ ... }` list:

```rust
    ReflectAssessed,
    ReflectIncident,
    ReflectIndexSummary,
    ReflectKind,
    ReflectKindListing,
    ReflectKindSummary,
    ReflectMode,
    ReflectReport,
    ReflectRequest,
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib models::reflect`
Expected: PASS (7 tests).

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/app/models.rs src/app/models/reflect.rs src/app/models/reflect_tests.rs src/app.rs
git commit -m "feat(reflect): add reflect report types and cross-kind ranking

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: LLM failure classifier and the `run_reflect` pipeline

**Files:**
- Create: `src/app/services/reflect_llm.rs`, `src/app/services/reflect_llm_tests.rs`
- Create: `src/app/services/reflect.rs`, `src/app/services/reflect_tests.rs`
- Modify: `src/app/services.rs`, `src/app/services/{skill,mcp,hook}_assessment.rs` (visibility only)

**Interfaces:**
- Consumes: Task 1 seeding helpers; Task 2 types and functions; existing `list_ai_*_incidents`, `investigate_ai_*_incidents`, `run_one_*_assessment`, `index_ai_roots`, `self.llm().backend(None)`, `LlmBackend::program()`.
- Produces: `CortexService::run_reflect<P: FnMut(&str) + Send>(&self, req: ReflectRequest, progress: P) -> ServiceResult<ReflectReport>`.
- Produces (crate-private): `reflect_llm::{ReflectLlmFailure, classify_llm_failure, program_on_path}`; `services::reflect::INCIDENT_CHANGED`.

Behavior recap (from the revised spec): pin `until`; index with `since` as the file-age filter; list with limit 100 and keep totals and truncation; investigate each top incident once with its incident id and exact target; findings come from that evidence; the LLM runs through the per-evidence helper; an LLM error keeps the findings; "globally disabled" stops the LLM for the run; "action disabled" and "circuit open" stop it for that kind; an incident that no longer resolves is a per-incident failure.

- [ ] **Step 1: Make the per-evidence LLM helpers visible to sibling modules**

In each of `src/app/services/skill_assessment.rs`, `mcp_assessment.rs`, and `hook_assessment.rs`, change `async fn run_one_skill_assessment<F>(` / `async fn run_one_mcp_assessment<F>(` / `async fn run_one_hook_assessment<F>(` to `pub(super) async fn ...`. No other change.

- [ ] **Step 2: Write the failing classifier tests**

Create `src/app/services/reflect_llm_tests.rs`:

```rust
use super::*;

fn internal(error: LlmRunnerError) -> ServiceError {
    ServiceError::Internal(anyhow::anyhow!(error))
}

#[test]
fn globally_disabled_llm_is_unavailable() {
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::Disabled)),
        ReflectLlmFailure::Unavailable(_)
    ));
}

#[test]
fn action_disabled_and_open_circuit_block_one_kind() {
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::ActionDisabled("skill_assess".into()))),
        ReflectLlmFailure::KindBlocked(_)
    ));
    let ReflectLlmFailure::KindBlocked(reason) = classify_llm_failure(&internal(LlmRunnerError::CircuitOpen {
        action: "mcp_assess".into(),
        retry_after: "2026-09-11T00:05:00Z".into(),
    })) else {
        panic!("expected KindBlocked");
    };
    assert!(reason.contains("circuit open"));
}

#[test]
fn timeouts_rate_limits_and_backend_errors_fail_one_incident() {
    for error in [
        LlmRunnerError::Timeout("id".into(), 120),
        LlmRunnerError::RateLimited { action: "a".into(), detail: "d".into() },
        LlmRunnerError::Internal(anyhow::anyhow!("spawn failed")),
    ] {
        assert!(matches!(classify_llm_failure(&internal(error)), ReflectLlmFailure::Failed(_)));
    }
}

#[test]
fn non_llm_errors_propagate() {
    assert_eq!(
        classify_llm_failure(&ServiceError::InvalidInput("bad".into())),
        ReflectLlmFailure::NotLlm
    );
}

#[test]
fn program_lookup_uses_the_exact_string() {
    assert!(program_on_path("sh"));
    assert!(program_on_path("/bin/sh"));
    assert!(!program_on_path("cortex-reflect-definitely-missing-binary"));
    assert!(!program_on_path(""));
    // A path containing a space must not be split into "/tmp/My".
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("My Codex");
    std::fs::create_dir_all(&spaced).unwrap();
    let program = spaced.join("codex");
    std::fs::write(&program, "").unwrap();
    assert!(program_on_path(program.to_str().unwrap()));
}
```

- [ ] **Step 3: Implement the classifier**

Create `src/app/services/reflect_llm.rs`:

```rust
//! Pure helpers that decide how `cortex reflect` reacts when an LLM
//! assessment cannot run.

use crate::app::ServiceError;
use crate::app::llm_runner::LlmRunnerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReflectLlmFailure {
    /// The LLM is globally disabled: no more LLM calls this run.
    Unavailable(String),
    /// This kind's action is disabled or its circuit is open: no more LLM
    /// calls for this kind.
    KindBlocked(String),
    /// Only this assessment failed.
    Failed(String),
    /// Not an LLM failure: propagate the error.
    NotLlm,
}

/// The per-evidence helpers wrap `LlmRunnerError` as
/// `ServiceError::Internal(anyhow!(error))` (see `run_llm_with_delta` in
/// `src/app/services.rs`), so `downcast_ref` recovers it.
pub(crate) fn classify_llm_failure(error: &ServiceError) -> ReflectLlmFailure {
    let ServiceError::Internal(inner) = error else {
        return ReflectLlmFailure::NotLlm;
    };
    match inner.downcast_ref::<LlmRunnerError>() {
        Some(runner @ LlmRunnerError::Disabled) => ReflectLlmFailure::Unavailable(runner.to_string()),
        Some(runner @ (LlmRunnerError::ActionDisabled(_) | LlmRunnerError::CircuitOpen { .. })) => {
            ReflectLlmFailure::KindBlocked(runner.to_string())
        }
        Some(runner) => ReflectLlmFailure::Failed(runner.to_string()),
        None => ReflectLlmFailure::Failed(format!("{inner:#}")),
    }
}

/// True when `program` is an existing file path, or a bare name found in a
/// `PATH` directory. The backend spawns this exact string, so it is never
/// split on whitespace.
pub(crate) fn program_on_path(program: &str) -> bool {
    if program.is_empty() {
        return false;
    }
    let candidate = std::path::Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    crate::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    })
}

#[cfg(test)]
#[path = "reflect_llm_tests.rs"]
mod tests;
```

In `src/app/services.rs`, add `mod reflect_llm;` after `mod rag;`.

Run: `cargo test --lib reflect_llm`
Expected: PASS (5 tests).

- [ ] **Step 4: Write the failing pipeline tests**

Create `src/app/services/reflect_tests.rs`:

```rust
use std::sync::Arc;

use super::*;
use crate::app::models::{AiSkillIncidentRequest, ReflectIncident, ReflectKind, ReflectMode, ReflectRequest};
use crate::app::services::seed_test_support::{seed_hook_incident, seed_mcp_incident, seed_skill_incident};
use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("reflect-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn request(kinds: Vec<ReflectKind>, max_assess: u32) -> ReflectRequest {
    ReflectRequest {
        since: None,
        until: None,
        project: None,
        tool: None,
        kinds,
        run_llm: false,
        max_assess,
        index: false,
    }
}

fn seed_all(pool: &DbPool) {
    seed_skill_incident(pool, "sess-skill", "alpha-skill", 60);
    seed_mcp_incident(pool, "sess-mcp", "labby", "search", 40);
    seed_hook_incident(pool, "sess-hook", "format-on-save", 20);
}

#[tokio::test]
async fn report_only_merges_kinds_ranks_and_caps_detail() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let mut progress = Vec::new();

    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 2), |line| progress.push(line.to_string()))
        .await
        .unwrap();

    assert_eq!(report.mode, ReflectMode::ReportOnly);
    assert_eq!(report.llm_fallback_reason, None);
    assert_eq!(report.index, None);
    assert_eq!(report.assessed.len(), 2);
    assert_eq!(report.unassessed.len(), 1);
    for summary in &report.summary {
        assert_eq!((summary.listed, summary.total, summary.truncated), (1, 1, false), "{:?}", summary.kind);
    }
    let scores: Vec<f64> = report
        .assessed
        .iter()
        .map(|a| a.incident.priority_score)
        .chain(report.unassessed.iter().map(|i| i.priority_score))
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "not ranked: {scores:?}");
    assert!(progress.iter().any(|line| line.starts_with("assessing 1/2")));

    let llm_rows: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM llm_invocations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(llm_rows, 0, "report-only must never call the LLM");
}

#[tokio::test]
async fn every_kind_resolves_by_incident_id_and_target() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service.run_reflect(request(ReflectKind::ALL.to_vec(), 3), |_| {}).await.unwrap();
    assert_eq!(report.assessed.len(), 3);
    for assessed in &report.assessed {
        assert_eq!(assessed.failure, None, "{:?} did not resolve", assessed.incident.kind);
        assert_eq!(assessed.assessment, None);
        assert!(assessed.findings.is_object());
    }
}

#[tokio::test]
async fn pins_until_when_absent() {
    let (service, _pool, _dir) = test_service();
    let report = service.run_reflect(request(ReflectKind::ALL.to_vec(), 5), |_| {}).await.unwrap();
    assert!(report.until.is_some());
}

#[tokio::test]
async fn kinds_filter_limits_detection() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service.run_reflect(request(vec![ReflectKind::Hook], 5), |_| {}).await.unwrap();
    assert_eq!(report.summary.len(), 1);
    assert_eq!(report.assessed.len(), 1);
    assert_eq!(report.assessed[0].incident.kind, ReflectKind::Hook);
}

#[tokio::test]
async fn max_assess_zero_reports_summary_only() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service.run_reflect(request(ReflectKind::ALL.to_vec(), 0), |_| {}).await.unwrap();
    assert!(report.assessed.is_empty());
    assert_eq!(report.unassessed.len(), 3);
}

#[tokio::test]
async fn empty_database_produces_an_empty_report() {
    let (service, _pool, _dir) = test_service();
    let report = service.run_reflect(request(ReflectKind::ALL.to_vec(), 5), |_| {}).await.unwrap();
    assert!(report.assessed.is_empty());
    assert!(report.unassessed.is_empty());
    assert!(report.summary.iter().all(|s| s.total == 0));
    assert!(report.db_path.ends_with("reflect-test.db"));
}

#[tokio::test]
async fn empty_kinds_is_invalid_input() {
    let (service, _pool, _dir) = test_service();
    let error = service.run_reflect(request(vec![], 5), |_| {}).await.unwrap_err();
    assert!(matches!(error, ServiceError::InvalidInput(_)));
}

#[tokio::test]
async fn an_incident_that_no_longer_resolves_is_missing_not_an_error() {
    let (service, pool, _dir) = test_service();
    seed_skill_incident(&pool, "sess-a", "alpha-skill", 60);
    let listed = service
        .list_ai_skill_incidents(AiSkillIncidentRequest::default())
        .await
        .unwrap();
    let mut incident = ReflectIncident::from(listed.incidents[0].clone());
    incident.incident_id = "no-such-incident".to_string();

    let outcome = service
        .assess_reflect_incident(&incident, &request(vec![ReflectKind::Skill], 1), None)
        .await
        .unwrap();
    assert!(matches!(outcome, AssessOutcome::Missing));
}
```

Run: `cargo test --lib services::reflect::`
Expected: compile error, no method `run_reflect`.

- [ ] **Step 5: Implement the pipeline**

Create `src/app/services/reflect.rs`:

```rust
//! `cortex reflect`: index local transcripts, list skill/MCP/hook incidents,
//! rank them together, and assess the top N. Each top incident is
//! investigated once (incident id plus its exact target); its findings come
//! from that evidence, and the evidence goes to the kind's existing
//! per-evidence LLM helper. Orchestration only: no new detection or prompts.

use std::collections::BTreeMap;

use super::reflect_llm::{ReflectLlmFailure, classify_llm_failure, program_on_path};
use super::*;
use crate::app::models::{
    AiHookIncidentRequest, AiHookInvestigateRequest, AiMcpIncidentRequest,
    AiMcpInvestigateRequest, AiSkillIncidentRequest, AiSkillInvestigateRequest, ReflectAssessed,
    ReflectIncident, ReflectIndexSummary, ReflectKind, ReflectKindListing, ReflectMode,
    ReflectReport, ReflectRequest, rank_reflect_incidents, summarize_reflect_incidents,
};
use crate::llm_backend::LlmBackend;

/// The incident list services clamp `limit` to 1..=100.
const REFLECT_LIST_LIMIT: u32 = 100;

pub(crate) const INCIDENT_CHANGED: &str =
    "incident changed during the run (the database was written while reflect ran); rerun reflect";

pub(super) enum AssessOutcome {
    Done { assessment: Option<String>, findings: serde_json::Value },
    LlmError { error: ServiceError, findings: serde_json::Value },
    Missing,
}

/// Lists one kind's incidents and returns (incidents, total, truncated).
macro_rules! list_kind {
    ($self:ident, $req:ident, $list:ident, $Req:ident) => {{
        let response = $self
            .$list($Req {
                tool: $req.tool.clone(),
                project: $req.project.clone(),
                since: $req.since.clone(),
                until: $req.until.clone(),
                limit: Some(REFLECT_LIST_LIMIT),
                ..Default::default()
            })
            .await?;
        let truncated = response.truncated || response.candidate_window_truncated;
        let total = response.total_incidents;
        let incidents: Vec<ReflectIncident> =
            response.incidents.into_iter().map(ReflectIncident::from).collect();
        (incidents, total, truncated)
    }};
}

/// Investigates one incident by id and exact target, then optionally runs
/// the per-evidence LLM helper. Evaluates to an `AssessOutcome`; returns
/// early from the enclosing fn on non-NotFound investigate errors.
macro_rules! assess_kind {
    ($self:ident, $incident:ident, $req:ident, $backend:ident, $investigate:ident, $run_one:ident,
     $Req:ident { $($field:ident : $value:expr),* $(,)? }) => {{
        let investigated = $self
            .$investigate($Req {
                incident_id: Some($incident.incident_id.clone()),
                tool: $req.tool.clone(),
                project: $req.project.clone(),
                since: $req.since.clone(),
                until: $req.until.clone(),
                limit: Some(1),
                $($field: $value,)*
                ..Default::default()
            })
            .await;
        let response = match investigated {
            Ok(response) => response,
            Err(ServiceError::NotFound(_)) => return Ok(AssessOutcome::Missing),
            Err(error) => return Err(error),
        };
        match response.evidence.into_iter().next() {
            None => AssessOutcome::Missing,
            Some(evidence) => {
                let findings = findings_json(&evidence.findings)?;
                match $backend {
                    None => AssessOutcome::Done { assessment: None, findings },
                    Some(backend) => {
                        let mut ignore = |_: &str| -> anyhow::Result<()> { Ok(()) };
                        match $self.$run_one(&evidence, backend, &mut ignore).await {
                            Ok(result) => AssessOutcome::Done { assessment: result.assessment, findings },
                            Err(error) => AssessOutcome::LlmError { error, findings },
                        }
                    }
                }
            }
        }
    }};
}

impl CortexService {
    pub async fn run_reflect<P>(
        &self,
        mut req: ReflectRequest,
        mut progress: P,
    ) -> ServiceResult<ReflectReport>
    where
        P: FnMut(&str) + Send,
    {
        if req.kinds.is_empty() {
            return Err(ServiceError::InvalidInput(
                "reflect requires at least one kind: skill, mcp, or hook".to_string(),
            ));
        }
        // Pin the window end so listing and assessment see the same incidents.
        if req.until.is_none() {
            req.until = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }

        let index = if req.index {
            progress(&format!(
                "indexing AI transcripts modified since {} (the first run can take a while)",
                req.since.as_deref().unwrap_or("the beginning")
            ));
            let result = self.index_ai_roots(None, false, req.since.clone()).await?;
            let summary = ReflectIndexSummary::from(&result);
            progress(&format!(
                "indexed {} new records from {} files ({} parse errors)",
                summary.ingested, summary.discovered_files, summary.parse_errors
            ));
            Some(summary)
        } else {
            None
        };

        let mut incidents = Vec::new();
        let mut listings = Vec::with_capacity(req.kinds.len());
        for &kind in &req.kinds {
            progress(&format!("detecting {} incidents", kind.as_str()));
            let (found, total, truncated) = match kind {
                ReflectKind::Skill => list_kind!(self, req, list_ai_skill_incidents, AiSkillIncidentRequest),
                ReflectKind::Mcp => list_kind!(self, req, list_ai_mcp_incidents, AiMcpIncidentRequest),
                ReflectKind::Hook => list_kind!(self, req, list_ai_hook_incidents, AiHookIncidentRequest),
            };
            listings.push(ReflectKindListing { kind, total, truncated });
            incidents.extend(found);
        }
        let mut ranked = rank_reflect_incidents(incidents);
        let summary = summarize_reflect_incidents(&listings, &ranked);
        let take = (req.max_assess as usize).min(ranked.len());
        let unassessed = ranked.split_off(take);

        let mut mode = if req.run_llm { ReflectMode::ReportAndLlm } else { ReflectMode::ReportOnly };
        let mut llm_fallback_reason = None;
        let mut backend = None;
        if mode == ReflectMode::ReportAndLlm && take > 0 {
            match self.reflect_llm_backend() {
                Ok(resolved) => backend = Some(resolved),
                Err(reason) => {
                    mode = ReflectMode::ReportOnly;
                    llm_fallback_reason = Some(reason);
                }
            }
        }

        let mut blocked: BTreeMap<ReflectKind, String> = BTreeMap::new();
        let mut assessed = Vec::with_capacity(take);
        for (position, incident) in ranked.into_iter().enumerate() {
            progress(&format!(
                "assessing {}/{take}: {} {}",
                position + 1,
                incident.kind.as_str(),
                incident.target
            ));
            let kind_blocked = blocked.get(&incident.kind).cloned();
            let use_backend = if kind_blocked.is_none() { backend.as_ref() } else { None };
            let entry = match self.assess_reflect_incident(&incident, &req, use_backend).await? {
                AssessOutcome::Done { assessment, findings } => ReflectAssessed {
                    incident,
                    assessment,
                    findings,
                    failure: kind_blocked,
                },
                AssessOutcome::Missing => ReflectAssessed {
                    incident,
                    assessment: None,
                    findings: serde_json::Value::Null,
                    failure: Some(INCIDENT_CHANGED.to_string()),
                },
                AssessOutcome::LlmError { error, findings } => {
                    let failure = match classify_llm_failure(&error) {
                        ReflectLlmFailure::NotLlm => return Err(error),
                        ReflectLlmFailure::Unavailable(reason) => {
                            backend = None;
                            llm_fallback_reason = Some(reason.clone());
                            reason
                        }
                        ReflectLlmFailure::KindBlocked(reason) => {
                            blocked.insert(incident.kind, reason.clone());
                            reason
                        }
                        ReflectLlmFailure::Failed(reason) => reason,
                    };
                    ReflectAssessed { incident, assessment: None, findings, failure: Some(failure) }
                }
            };
            assessed.push(entry);
        }
        if llm_fallback_reason.is_some() && assessed.iter().all(|entry| entry.assessment.is_none()) {
            mode = ReflectMode::ReportOnly;
        }

        Ok(ReflectReport {
            since: req.since.clone(),
            until: req.until.clone(),
            project: req.project.clone(),
            tool: req.tool.clone(),
            kinds: req.kinds.clone(),
            db_path: self.storage.db_path.display().to_string(),
            mode,
            llm_fallback_reason,
            index,
            summary,
            assessed,
            unassessed,
        })
    }

    /// Resolves the backend and checks its exact program string exists.
    fn reflect_llm_backend(&self) -> Result<LlmBackend, String> {
        let backend = self.llm().backend(None).map_err(|error| {
            format!("LLM backend could not be resolved ({error}); set CORTEX_LLM to codex or gemini, or pass --no-llm")
        })?;
        let program = backend.program();
        if program_on_path(&program) {
            Ok(backend)
        } else {
            Err(format!(
                "LLM backend program '{program}' was not found; install it, set CORTEX_CODEX_CMD or \
                 CORTEX_HEADLESS_GEMINI_CMD, or pass --no-llm"
            ))
        }
    }

    pub(super) async fn assess_reflect_incident(
        &self,
        incident: &ReflectIncident,
        req: &ReflectRequest,
        backend: Option<&LlmBackend>,
    ) -> ServiceResult<AssessOutcome> {
        // The target filters are exact matches on each kind's grouping key,
        // so they shrink the scan without changing the incident id.
        Ok(match incident.kind {
            ReflectKind::Skill => assess_kind!(
                self, incident, req, backend, investigate_ai_skill_incidents, run_one_skill_assessment,
                AiSkillInvestigateRequest {
                    skill: Some(incident.target_key.clone()),
                    plugin: incident.target_detail.clone(),
                }
            ),
            ReflectKind::Mcp => assess_kind!(
                self, incident, req, backend, investigate_ai_mcp_incidents, run_one_mcp_assessment,
                AiMcpInvestigateRequest {
                    mcp_server: Some(incident.target_key.clone()),
                    mcp_tool: incident.target_detail.clone(),
                }
            ),
            ReflectKind::Hook => assess_kind!(
                self, incident, req, backend, investigate_ai_hook_incidents, run_one_hook_assessment,
                AiHookInvestigateRequest {
                    hook_event: Some(incident.target_key.clone()),
                    hook_name: incident.target_detail.clone(),
                }
            ),
        })
    }
}

fn findings_json<T: serde::Serialize>(findings: &T) -> ServiceResult<serde_json::Value> {
    serde_json::to_value(findings)
        .map_err(|error| ServiceError::Internal(anyhow::anyhow!("serialize findings: {error}")))
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
```

In `src/app/services.rs`, add `mod reflect;` after `mod reflect_llm;`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib reflect`
Expected: PASS (`models::reflect`, `reflect_llm`, and `services::reflect`).

If `every_kind_resolves_by_incident_id_and_target` reports a kind with `failure: Some(INCIDENT_CHANGED)`, that kind's investigate did not match its own id under the target filter. Check that kind's `Task 1` test passes first, then compare the filter field names in `assess_reflect_incident` with that kind's `Ai*InvestigateRequest`. Do not remove the target filter.

If `an_incident_that_no_longer_resolves_is_missing_not_an_error` fails with `InvalidInput`, the skill investigate service reports an unknown id as invalid input. Read `investigate_ai_skill_incidents` in `src/app/services/skill_incidents.rs`, and add that exact error to the `Missing` arm in `assess_kind!` with a comment naming the service behavior.

- [ ] **Step 7: Check sizes and commit**

Run: `wc -l src/app/services/reflect.rs src/app/services/reflect_llm.rs`
Expected: each under 500.

```bash
cargo fmt && cargo clippy --all-targets
git add src/app/services.rs src/app/services/reflect.rs src/app/services/reflect_tests.rs src/app/services/reflect_llm.rs src/app/services/reflect_llm_tests.rs src/app/services/skill_assessment.rs src/app/services/mcp_assessment.rs src/app/services/hook_assessment.rs
git commit -m "feat(reflect): add run_reflect pipeline with per-kind LLM fallbacks

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Markdown report renderer

**Files:**
- Create: `src/app/reflect_report.rs`, `src/app/reflect_report_tests.rs`
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Task 2 types.
- Produces: `cortex::app::reflect_report::render_reflect_markdown(&ReflectReport) -> String`.

- [ ] **Step 1: Write the failing tests**

Create `src/app/reflect_report_tests.rs`:

```rust
use super::*;
use crate::app::models::{
    ReflectAssessed, ReflectIncident, ReflectIndexSummary, ReflectKind, ReflectKindSummary,
    ReflectMode, ReflectReport,
};

fn incident(kind: ReflectKind, id: &str, target: &str) -> ReflectIncident {
    ReflectIncident {
        kind,
        incident_id: id.to_string(),
        target: target.to_string(),
        target_key: target.to_string(),
        target_detail: None,
        tool: "claude".to_string(),
        project: "/p".to_string(),
        session_id: "sess-1".to_string(),
        last_seen: "2026-09-10T12:00:00Z".to_string(),
        priority_score: 42.0,
        priority_label: "high".to_string(),
        signals_present: vec!["user_correction_after_skill".to_string()],
    }
}

fn summary_row(kind: ReflectKind, listed: usize, total: usize, truncated: bool) -> ReflectKindSummary {
    ReflectKindSummary { kind, listed, total, truncated, critical: 0, high: listed, medium: 0, low: 0 }
}

fn report(assessed: Vec<ReflectAssessed>, unassessed: Vec<ReflectIncident>) -> ReflectReport {
    ReflectReport {
        since: Some("2026-09-04T00:00:00Z".to_string()),
        until: Some("2026-09-11T00:00:00Z".to_string()),
        project: None,
        tool: None,
        kinds: ReflectKind::ALL.to_vec(),
        db_path: "/home/u/.cortex/reflect.db".to_string(),
        mode: ReflectMode::ReportOnly,
        llm_fallback_reason: None,
        index: Some(ReflectIndexSummary { discovered_files: 3, ingested: 10, skipped_dupes: 0, parse_errors: 1 }),
        summary: vec![summary_row(ReflectKind::Skill, 1, 1, false)],
        assessed,
        unassessed,
    }
}

fn assessed(target: &str, assessment: Option<&str>, failure: Option<&str>) -> ReflectAssessed {
    ReflectAssessed {
        incident: incident(ReflectKind::Skill, "abc", target),
        assessment: assessment.map(str::to_string),
        findings: serde_json::json!({"likely_failure_modes": ["x"]}),
        failure: failure.map(str::to_string),
    }
}

#[test]
fn report_only_shows_summary_findings_and_other_incidents() {
    let md = render_reflect_markdown(&report(
        vec![assessed("lavra:lavra-plan", None, None)],
        vec![incident(ReflectKind::Mcp, "def", "labby/search")],
    ));
    assert!(md.starts_with("# Cortex reflect report\n"));
    assert!(md.contains("Mode: report only"));
    assert!(md.contains("Index: 3 files discovered, 10 new records, 0 duplicates skipped, 1 parse errors"));
    assert!(md.contains("| skill | 1 | 1 | 0 | 1 | 0 | 0 |"));
    assert!(md.contains("Scores are heuristic"));
    assert!(md.contains("## 1. skill `lavra:lavra-plan` (high, score 42.0)"));
    assert!(md.contains("```json\n"));
    assert!(md.contains("## Other incidents"));
    assert!(md.contains(
        "`cortex assess mcp --incident-id def --since 2026-09-04T00:00:00Z --until 2026-09-11T00:00:00Z`"
    ));
}

#[test]
fn truncated_kinds_get_a_warning() {
    let mut rep = report(vec![], vec![incident(ReflectKind::Skill, "a", "s")]);
    rep.summary = vec![summary_row(ReflectKind::Skill, 100, 250, true)];
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("> skill: showing the top 100 of 250 incidents"));
    assert!(md.contains("narrow --since"));
}

#[test]
fn llm_headings_are_demoted_outside_code_fences() {
    let mut rep = report(
        vec![assessed("s", Some("## Incident Summary\nText\n```sh\n# keep me\n```\n### Detail\n"), None)],
        vec![],
    );
    rep.mode = ReflectMode::ReportAndLlm;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("Mode: report + LLM"));
    assert!(md.contains("\n#### Incident Summary\n"));
    assert!(md.contains("\n# keep me\n"));
    assert!(md.contains("\n##### Detail\n"));
    assert!(!md.contains("```json"));
}

#[test]
fn failures_and_fallbacks_are_visible() {
    let mut rep = report(vec![assessed("s", None, Some("LLM invocation 'x' timed out after 120s"))], vec![]);
    rep.llm_fallback_reason = Some("LLM backend program 'codex' was not found".to_string());
    rep.index = None;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("> LLM skipped: LLM backend program 'codex' was not found"));
    assert!(md.contains("- Assessment failed: LLM invocation 'x' timed out after 120s"));
    assert!(md.contains("Index: skipped (--no-index)"));
}

#[test]
fn null_findings_are_not_rendered_as_json() {
    let mut entry = assessed("s", None, Some("incident changed during the run"));
    entry.findings = serde_json::Value::Null;
    let md = render_reflect_markdown(&report(vec![entry], vec![]));
    assert!(!md.contains("```json"));
}

#[test]
fn empty_report_says_so() {
    let mut rep = report(vec![], vec![]);
    rep.summary = vec![];
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("No skill, MCP, or hook incidents found in this window."));
    assert!(!md.contains("## Other incidents"));
}

#[test]
fn control_characters_and_table_breakers_are_neutralized() {
    let md = render_reflect_markdown(&report(
        vec![assessed("evil\u{1b}[31m`x`", Some("ok\u{1b}[2J\n"), None)],
        vec![incident(ReflectKind::Mcp, "p", "a|b`c")],
    ));
    assert!(!md.contains('\u{1b}'));
    assert!(md.contains("a\\|b'c"));
    assert!(md.contains("`evil [31m'x'`"));
}
```

- [ ] **Step 2: Implement the renderer**

Create `src/app/reflect_report.rs`:

```rust
//! Markdown rendering for `cortex reflect`. Pure: no I/O. Transcript-derived
//! text is treated as untrusted: control characters are removed before it
//! reaches a terminal.

use std::fmt::Write as _;

use crate::app::models::{ReflectAssessed, ReflectMode, ReflectReport};

pub fn render_reflect_markdown(report: &ReflectReport) -> String {
    let mut md = String::from("# Cortex reflect report\n\n");
    write_header(&mut md, report);
    write_summary(&mut md, report);
    for (position, assessed) in report.assessed.iter().enumerate() {
        write_assessed(&mut md, position + 1, assessed);
    }
    write_unassessed(&mut md, report);
    md
}

/// Single-line text: every control character becomes a space.
fn inline(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Text inside a code span: single line, no backticks.
fn code_span(text: &str) -> String {
    inline(text).replace('`', "'")
}

/// Text inside a table cell: single line, no pipes or backticks.
fn cell(text: &str) -> String {
    code_span(text).replace('|', "\\|")
}

/// Multi-line text: keep newlines and tabs, drop other control characters.
fn block(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// Pushes headings two levels down so they nest under the H2 incident
/// heading, leaving fenced code untouched.
fn demote_headings(text: &str) -> String {
    let mut in_fence = false;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                line.to_string()
            } else if !in_fence && line.starts_with('#') {
                format!("##{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_header(md: &mut String, report: &ReflectReport) {
    let since = report.since.as_deref().unwrap_or("beginning");
    let until = report.until.as_deref().unwrap_or("now");
    let kinds: Vec<&str> = report.kinds.iter().map(|kind| kind.as_str()).collect();
    let _ = writeln!(md, "Window: {} to {}  ", inline(since), inline(until));
    let _ = writeln!(
        md,
        "Filters: project={}, tool={}, kinds={}  ",
        inline(report.project.as_deref().unwrap_or("all")),
        inline(report.tool.as_deref().unwrap_or("all")),
        kinds.join(", ")
    );
    let _ = writeln!(md, "Database: `{}`  ", code_span(&report.db_path));
    let mode = match report.mode {
        ReflectMode::ReportOnly => "report only",
        ReflectMode::ReportAndLlm => "report + LLM",
    };
    let _ = writeln!(md, "Mode: {mode}  ");
    match &report.index {
        Some(index) => {
            let _ = writeln!(
                md,
                "Index: {} files discovered, {} new records, {} duplicates skipped, {} parse errors",
                index.discovered_files, index.ingested, index.skipped_dupes, index.parse_errors
            );
        }
        None => md.push_str("Index: skipped (--no-index)\n"),
    }
    if let Some(reason) = &report.llm_fallback_reason {
        let _ = writeln!(md, "\n> LLM skipped: {}", inline(reason));
    }
    md.push('\n');
}

fn write_summary(md: &mut String, report: &ReflectReport) {
    md.push_str("## Summary\n\n");
    if report.assessed.is_empty() && report.unassessed.is_empty() {
        md.push_str("No skill, MCP, or hook incidents found in this window.\n\n");
        return;
    }
    md.push_str("| Kind | Listed | Total | Critical | High | Medium | Low |\n");
    md.push_str("|------|--------|-------|----------|------|--------|-----|\n");
    for row in &report.summary {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} |",
            row.kind.as_str(),
            row.listed,
            row.total,
            row.critical,
            row.high,
            row.medium,
            row.low
        );
    }
    md.push('\n');
    for row in report.summary.iter().filter(|row| row.truncated) {
        let _ = writeln!(
            md,
            "> {}: showing the top {} of {} incidents; the detection window was capped. \
             Use a narrow --since for complete results.",
            row.kind.as_str(),
            row.listed,
            row.total
        );
    }
    md.push_str("Scores are heuristic. They share one formula shape across kinds but are not calibrated between them.\n\n");
}

fn write_assessed(md: &mut String, position: usize, assessed: &ReflectAssessed) {
    let incident = &assessed.incident;
    let _ = writeln!(
        md,
        "## {position}. {} `{}` ({}, score {:.1})\n",
        incident.kind.as_str(),
        code_span(&incident.target),
        inline(&incident.priority_label),
        incident.priority_score
    );
    let _ = writeln!(md, "- Incident: `{}`", code_span(&incident.incident_id));
    let _ = writeln!(
        md,
        "- Session: `{}` ({}, {})",
        code_span(&incident.session_id),
        inline(&incident.tool),
        inline(&incident.project)
    );
    let _ = writeln!(md, "- Last seen: {}", inline(&incident.last_seen));
    if !incident.signals_present.is_empty() {
        let signals: Vec<String> = incident.signals_present.iter().map(|s| inline(s)).collect();
        let _ = writeln!(md, "- Signals: {}", signals.join(", "));
    }
    if let Some(failure) = &assessed.failure {
        let _ = writeln!(md, "- Assessment failed: {}", inline(failure));
    }
    md.push('\n');
    match &assessed.assessment {
        Some(text) => {
            md.push_str(&demote_headings(&block(text)));
            if !md.ends_with('\n') {
                md.push('\n');
            }
        }
        None if assessed.findings.is_null() => {}
        None => {
            let findings = serde_json::to_string_pretty(&assessed.findings)
                .unwrap_or_else(|_| "{}".to_string());
            let _ = writeln!(md, "```json\n{}\n```", block(&findings));
        }
    }
    md.push('\n');
}

fn write_unassessed(md: &mut String, report: &ReflectReport) {
    if report.unassessed.is_empty() {
        return;
    }
    md.push_str("## Other incidents\n\n");
    md.push_str("| Kind | Target | Severity | Score | Last seen | Assess with |\n");
    md.push_str("|------|--------|----------|-------|-----------|-------------|\n");
    for incident in &report.unassessed {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {:.1} | {} | `{}` |",
            incident.kind.as_str(),
            cell(&incident.target),
            cell(&incident.priority_label),
            incident.priority_score,
            cell(&incident.last_seen),
            cell(&incident.assess_command(report.since.as_deref(), report.until.as_deref()))
        );
    }
    md.push('\n');
}

#[cfg(test)]
#[path = "reflect_report_tests.rs"]
mod tests;
```

In `src/app.rs`, add `pub mod reflect_report;` next to the other `pub mod` lines.

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib reflect_report`
Expected: PASS (7 tests).

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/app.rs src/app/reflect_report.rs src/app/reflect_report_tests.rs
git commit -m "feat(reflect): render reflect reports as markdown

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: `cortex reflect` CLI wiring

**Files:**
- Create: `src/cli/args/reflect.rs`, `src/cli/parse/reflect.rs`, `src/cli/parse/reflect_tests.rs`
- Create: `src/cli/dispatch_reflect.rs`, `src/cli/dispatch_reflect_tests.rs`
- Modify: `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/run.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/surfaces.rs`, `src/cli/help.rs`, `src/cli/help_tests.rs`

**Interfaces:**
- Consumes: `cortex::app::{ReflectKind, ReflectRequest}`, `CortexService::run_reflect`, `cortex::app::reflect_report::render_reflect_markdown`.
- Produces: `CliCommand::Reflect(ReflectArgs)`; in `cli`: `ReflectDbSource { Flag, Env, Default }`, `resolve_reflect_db_path(Option<&Path>, Option<OsString>, Option<OsString>) -> Result<(PathBuf, ReflectDbSource)>`, `prepare_reflect_db_dir(&Path, ReflectDbSource) -> Result<()>`, `restrict_reflect_db_file(&Path) -> Result<()>`; `RuntimeCore::query_only_with_retry(Config) -> Result<RuntimeCore>`.

- [ ] **Step 1: Write the failing parser tests**

Create `src/cli/parse/reflect_tests.rs`:

```rust
use super::*;

fn parse(args: &[&str]) -> anyhow::Result<ReflectArgs> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match parse_reflect(&args)? {
        CliCommand::Reflect(parsed) => Ok(parsed),
        other => panic!("expected reflect, got {other:?}"),
    }
}

#[test]
fn defaults_cover_all_kinds_llm_on_and_seven_days() {
    let parsed = parse(&[]).unwrap();
    assert_eq!(parsed.kinds, ReflectKind::ALL.to_vec());
    assert!(!parsed.no_llm && !parsed.no_index && !parsed.json);
    assert_eq!(parsed.max_assess, DEFAULT_REFLECT_MAX_ASSESS);
    assert_eq!(parsed.db, None);
    assert!(!parsed.since.is_empty(), "default --since 7d must be normalized");
}

#[test]
fn every_flag_is_parsed() {
    let parsed = parse(&[
        "--since", "2026-09-01T00:00:00Z", "--until", "2026-09-02T00:00:00Z",
        "--project", "/p", "--tool", "codex", "--kinds", "hook,skill",
        "--no-llm", "--max-assess", "0", "--no-index", "--db", "/tmp/r.db", "--json",
    ])
    .unwrap();
    assert!(parsed.since.starts_with("2026-09-01"));
    assert!(parsed.until.as_deref().unwrap().starts_with("2026-09-02"));
    assert_eq!(parsed.project.as_deref(), Some("/p"));
    assert_eq!(parsed.tool.as_deref(), Some("codex"));
    assert_eq!(parsed.kinds, vec![ReflectKind::Hook, ReflectKind::Skill]);
    assert!(parsed.no_llm && parsed.no_index && parsed.json);
    assert_eq!(parsed.max_assess, 0);
    assert_eq!(parsed.db, Some(std::path::PathBuf::from("/tmp/r.db")));
}

#[test]
fn kinds_are_deduplicated() {
    assert_eq!(parse(&["--kinds", "mcp,mcp, mcp"]).unwrap().kinds, vec![ReflectKind::Mcp]);
}

#[test]
fn unknown_kind_is_rejected() {
    let error = parse(&["--kinds", "skill,abuse"]).unwrap_err().to_string();
    assert!(error.contains("unknown kind 'abuse'"), "{error}");
}

#[test]
fn empty_kinds_is_rejected() {
    assert!(parse(&["--kinds", ","]).is_err());
}

#[test]
fn removed_and_unknown_options_are_rejected() {
    assert!(parse(&["--out", "x.md"]).is_err());
    assert!(parse(&["--all"]).is_err());
}
```

- [ ] **Step 2: Write the failing DB path tests**

Create `src/cli/dispatch_reflect_tests.rs`:

```rust
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

#[test]
fn flag_wins_over_env_and_home() {
    let (path, source) = resolve_reflect_db_path(
        Some(Path::new("/flag.db")),
        Some(OsString::from("/env.db")),
        Some(OsString::from("/home/u")),
    )
    .unwrap();
    assert_eq!((path, source), (PathBuf::from("/flag.db"), ReflectDbSource::Flag));
}

#[test]
fn env_wins_over_home() {
    let (path, source) =
        resolve_reflect_db_path(None, Some(OsString::from("/env.db")), Some(OsString::from("/home/u"))).unwrap();
    assert_eq!((path, source), (PathBuf::from("/env.db"), ReflectDbSource::Env));
}

#[test]
fn empty_env_falls_back_to_home_default() {
    let (path, source) =
        resolve_reflect_db_path(None, Some(OsString::new()), Some(OsString::from("/home/u"))).unwrap();
    assert_eq!((path, source), (PathBuf::from("/home/u/.cortex/reflect.db"), ReflectDbSource::Default));
}

#[test]
fn missing_home_is_an_error_that_mentions_db_flag() {
    let error = resolve_reflect_db_path(None, None, None).unwrap_err().to_string();
    assert!(error.contains("--db"), "{error}");
}

#[cfg(unix)]
#[test]
fn default_parent_is_created_owner_only_and_existing_dirs_are_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join(".cortex/reflect.db");
    prepare_reflect_db_dir(&db, ReflectDbSource::Default).unwrap();
    let mode = std::fs::metadata(home.path().join(".cortex")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);

    let existing = tempfile::tempdir().unwrap();
    std::fs::set_permissions(existing.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    prepare_reflect_db_dir(&existing.path().join("reflect.db"), ReflectDbSource::Default).unwrap();
    let mode = std::fs::metadata(existing.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[cfg(unix)]
#[test]
fn db_file_and_wal_siblings_become_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("reflect.db");
    for name in ["reflect.db", "reflect.db-wal"] {
        std::fs::write(dir.path().join(name), "").unwrap();
        std::fs::set_permissions(dir.path().join(name), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    restrict_reflect_db_file(&db).unwrap();
    for name in ["reflect.db", "reflect.db-wal"] {
        let mode = std::fs::metadata(dir.path().join(name)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
}
```

- [ ] **Step 3: Add `ReflectArgs` and the command variant**

Create `src/cli/args/reflect.rs`:

```rust
//! `cortex reflect`: one-shot local skill / MCP / hook reflection report.

use std::path::PathBuf;

use cortex::app::ReflectKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReflectArgs {
    /// Normalized absolute timestamp (default: 7 days ago).
    pub since: String,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub no_llm: bool,
    pub max_assess: u32,
    pub no_index: bool,
    pub db: Option<PathBuf>,
    pub json: bool,
}
```

In `src/cli/args.rs`: add `mod reflect;` next to `mod assess;`, `pub(crate) use reflect::ReflectArgs;` next to the `pub(crate) use assess::{...}` block, and `Reflect(ReflectArgs),` after `Assess(AssessCommand),` in `CliCommand`.

- [ ] **Step 4: Add the parser**

Create `src/cli/parse/reflect.rs`:

```rust
//! Parser for `cortex reflect`.

use std::path::PathBuf;

use anyhow::{Result, bail};
use cortex::app::ReflectKind;

use super::super::args::{CliCommand, ReflectArgs};
use super::super::parse_common::{FlagCursor, norm_time, parse_u32_flag};
use super::super::suggest;

pub(crate) const DEFAULT_REFLECT_SINCE: &str = "7d";
pub(crate) const DEFAULT_REFLECT_MAX_ASSESS: u32 = 5;

const REFLECT_FLAGS: &[&str] = &[
    "--since",
    "--until",
    "--project",
    "--tool",
    "--kinds",
    "--no-llm",
    "--max-assess",
    "--no-index",
    "--db",
    "--json",
];

pub(crate) fn parse_reflect(args: &[String]) -> Result<CliCommand> {
    let mut since: Option<String> = None;
    let mut parsed = ReflectArgs {
        since: String::new(),
        until: None,
        project: None,
        tool: None,
        kinds: ReflectKind::ALL.to_vec(),
        no_llm: false,
        max_assess: DEFAULT_REFLECT_MAX_ASSESS,
        no_index: false,
        db: None,
        json: false,
    };
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--no-llm" => parsed.no_llm = true,
            "--no-index" => parsed.no_index = true,
            "--since" => since = Some(flags.value("--since")?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--kinds" => parsed.kinds = parse_kinds(&flags.value("--kinds")?)?,
            "--max-assess" => {
                parsed.max_assess = parse_u32_flag("--max-assess", flags.value("--max-assess")?)?
            }
            "--db" => parsed.db = Some(PathBuf::from(flags.value("--db")?)),
            other => bail!("{}", suggest::unknown_option("reflect", other, REFLECT_FLAGS)),
        }
    }
    parsed.since = norm_time(since.unwrap_or_else(|| DEFAULT_REFLECT_SINCE.to_string()))?;
    Ok(CliCommand::Reflect(parsed))
}

fn parse_kinds(raw: &str) -> Result<Vec<ReflectKind>> {
    let mut kinds = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|part| !part.is_empty()) {
        let Some(kind) = ReflectKind::parse(part) else {
            bail!("reflect: unknown kind '{part}'; expected skill, mcp, or hook");
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        bail!("reflect: --kinds needs at least one of skill, mcp, hook");
    }
    Ok(kinds)
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
```

In `src/cli/parse.rs`: add `mod reflect;` next to `mod assess;`, `use self::reflect::parse_reflect;` next to `use self::assess::parse_assess;`, and `"reflect" => parse_reflect(rest),` after `"assess" => parse_assess(rest),`.

- [ ] **Step 5: Add dispatch, DB path resolution, and permissions**

Create `src/cli/dispatch_reflect.rs`:

```rust
//! `cortex reflect` dispatch, database path resolution, and database file
//! permissions.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cortex::app::ReflectRequest;
use cortex::app::reflect_report::render_reflect_markdown;

use super::CliMode;
use super::args::ReflectArgs;

pub(crate) async fn run_reflect(mode: &CliMode, args: ReflectArgs) -> Result<()> {
    // main.rs rejects HTTP mode before building a runtime; this guard is
    // defensive for any other caller.
    let CliMode::Local(service) = mode else {
        bail!("cortex reflect runs locally; remove --http / --server / --token and unset CORTEX_USE_HTTP");
    };
    let request = ReflectRequest {
        since: Some(args.since.clone()),
        until: args.until.clone(),
        project: args.project.clone(),
        tool: args.tool.clone(),
        kinds: args.kinds.clone(),
        run_llm: !args.no_llm,
        max_assess: args.max_assess,
        index: !args.no_index,
    };
    let report = service
        .run_reflect(request, |line| eprintln!("[reflect] {line}"))
        .await?;
    if let Some(reason) = &report.llm_fallback_reason {
        eprintln!("[reflect] warning: {reason}");
    }
    let body = if args.json {
        let mut json = serde_json::to_string_pretty(&report)?;
        json.push('\n');
        json
    } else {
        render_reflect_markdown(&report)
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReflectDbSource {
    Flag,
    Env,
    Default,
}

/// Reflect database: `--db`, then non-empty `CORTEX_DB_PATH`, then
/// `~/.cortex/reflect.db`. Environment values are passed in so the order is
/// unit-testable.
pub(crate) fn resolve_reflect_db_path(
    flag: Option<&Path>,
    env_db_path: Option<OsString>,
    home: Option<OsString>,
) -> Result<(PathBuf, ReflectDbSource)> {
    if let Some(path) = flag {
        return Ok((path.to_path_buf(), ReflectDbSource::Flag));
    }
    if let Some(path) = env_db_path.filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path), ReflectDbSource::Env));
    }
    let home = home.filter(|value| !value.is_empty()).ok_or_else(|| {
        anyhow!("cannot resolve ~/.cortex/reflect.db because HOME is not set; pass --db PATH")
    })?;
    Ok((PathBuf::from(home).join(".cortex").join("reflect.db"), ReflectDbSource::Default))
}

/// Creates the default database directory owner-only when it does not
/// exist. Existing directories and non-default paths are left alone.
pub(crate) fn prepare_reflect_db_dir(path: &Path, source: ReflectDbSource) -> Result<()> {
    if source != ReflectDbSource::Default {
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", parent.display()))?;
    }
    Ok(())
}

/// Restricts the SQLite file and its `-wal`/`-shm` siblings to the owner.
pub(crate) fn restrict_reflect_db_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let file = PathBuf::from(name);
            if file.is_file() {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("restricting {}", file.display()))?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_reflect_tests.rs"]
mod tests;
```

In `src/cli.rs`: add `mod dispatch_reflect;` next to `mod dispatch_sessions;`, and next to the file's other `pub(crate) use` lines:

```rust
pub(crate) use dispatch_reflect::{
    ReflectDbSource, prepare_reflect_db_dir, resolve_reflect_db_path, restrict_reflect_db_file,
};
```

In `src/cli/run.rs`, add after the `CliCommand::Assess(command) => match command { ... },` arm:

```rust
        CliCommand::Reflect(args) => super::dispatch_reflect::run_reflect(&mode, args).await,
```

- [ ] **Step 6: Add the retrying query-only constructor**

In `src/runtime.rs`, move the existing `for attempt in 0..3 { ... }` loop and everything after it in `load_query_only` (unchanged) into a new function, and make `load_query_only` call it:

```rust
    pub async fn load_query_only() -> Result<Self> {
        // Use load_for_stdio() to skip the non-loopback bind safety gate —
        // stdio mode never binds an HTTP port so the gate is irrelevant.
        Self::query_only_with_retry(Config::load_for_stdio()?).await
    }

    /// Query-only runtime for a caller-prepared config, retrying briefly
    /// when another writer holds the SQLite lock.
    pub async fn query_only_with_retry(config: Config) -> Result<Self> {
        // (the loop moved from load_query_only goes here, unchanged)
    }
```

The moved loop already calls `Self::query_only(config.clone())`, so it compiles unchanged. Run `cargo test --lib runtime` afterwards; expected PASS.

- [ ] **Step 7: Build the reflect runtime in `main.rs`**

In `src/main.rs`, insert immediately before the comment `// Build CliMode ONCE per invocation`:

```rust
    if let cli::CliCommand::Reflect(args) = &command {
        if let Some(trigger) = flags.http_trigger() {
            anyhow::bail!("cortex reflect runs locally; remove {trigger}");
        }
        let (db_path, source) = cli::resolve_reflect_db_path(
            args.db.as_deref(),
            cortex::env::var_os("CORTEX_DB_PATH"),
            cortex::env::var_os("HOME"),
        )?;
        if source == cli::ReflectDbSource::Env {
            eprintln!(
                "[reflect] warning: using CORTEX_DB_PATH={}; if this is a live cortex server \
                 database, reflect will write transcript records into it",
                db_path.display()
            );
        }
        cli::prepare_reflect_db_dir(&db_path, source)?;
        let mut config = cortex::config::Config::load_for_stdio()?;
        config.storage.db_path = db_path.clone();
        let runtime = RuntimeCore::query_only_with_retry(config).await?;
        cli::restrict_reflect_db_file(&db_path)?;
        return cli::run(cli::CliMode::Local(runtime.service()), command).await;
    }
```

- [ ] **Step 8: Register the surface and help**

In `src/surfaces.rs`, add after `local_cli!("assess", Sessions, Canonical),`:

```rust
    // One-shot local reflection report; may run the local LLM like `assess`.
    local_cli!("reflect", Sessions, Canonical),
```

Run: `grep -n 'CLI_ROOTS' src/surfaces.rs`. If `CLI_ROOTS` is a literal list, add `"reflect",` after `"assess",`; if it is derived from `SURFACE_SPECS`, leave it.

In `src/cli/help.rs`, change the group line to `("AI Transcripts", &["sessions", "assess", "reflect"]),` and add after the `assess` `CommandDoc`:

```rust
    CommandDoc {
        name: "reflect",
        summary: "One-shot local skill, MCP, and hook reflection report (local-only)",
        usage: &[
            "cortex reflect [--since TIME] [--until TIME] [--project PATH] [--tool TOOL] [--kinds skill,mcp,hook] [--no-llm] [--max-assess N] [--no-index] [--db PATH] [--json]",
        ],
    },
```

In `src/cli/help_tests.rs`, add `"reflect",` after `"assess",` in `PARSER_TOKENS`.

- [ ] **Step 9: Run the CLI tests**

Run: `cargo test --bin cortex`
Expected: PASS, including 6 parser and 6 dispatch tests. If a completion or catalog test fails because it enumerates root commands, add `reflect` next to `assess` in that list and rerun.

Run: `cargo test --lib surfaces`
Expected: PASS.

- [ ] **Step 10: Smoke test by hand**

```bash
cargo run --quiet -- reflect --no-llm --db "$(mktemp -d)/reflect.db" --max-assess 2
```

Expected: `[reflect] indexing ...`, `[reflect] indexed ...`, and `[reflect] detecting ...` on stderr; a report starting with `# Cortex reflect report` on stdout; exit 0.

```bash
cargo run --quiet -- --http reflect
```

Expected: non-zero exit with `cortex reflect runs locally; remove --http`.

- [ ] **Step 11: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/cli src/cli.rs src/main.rs src/runtime.rs src/surfaces.rs
git commit -m "feat(reflect): add cortex reflect command

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: End-to-end binary tests

**Files:**
- Create: `tests/reflect_cli.rs`

**Interfaces:**
- Consumes: the `cortex` binary via `env!("CARGO_BIN_EXE_cortex")`; `cortex::env::var_os`.

The fixture lines use the Claude fields the parser reads (`sessionId`, `timestamp`, `content`, `attributionSkill`, `attributionPlugin`; see `src/scanner/claude.rs` and the `attributionSkill` cases in `src/scanner_tests.rs`).

- [ ] **Step 1: Write the tests**

Create `tests/reflect_cli.rs`:

```rust
//! End-to-end: `cortex reflect` indexes Claude transcript lines from a
//! temporary HOME into the default reflect DB and reports a skill incident.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn now_minus(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn write_fixture(home: &Path) {
    let dir = home.join(".claude/projects/-tmp-reflect-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        serde_json::json!({
            "sessionId": "sess-e2e",
            "timestamp": now_minus(30),
            "attributionSkill": "cortex-troubleshoot",
            "attributionPlugin": "cortex",
            "content": "ran troubleshoot"
        }),
        serde_json::json!({
            "sessionId": "sess-e2e",
            "timestamp": now_minus(28),
            "content": "That's not what I asked for, please redo it."
        }),
    ];
    let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(dir.join("sess-e2e.jsonl"), body).unwrap();
}

fn cortex(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cortex"));
    command
        .args(args)
        .current_dir(home) // keep the repo's config.toml out of the run
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env_remove("CORTEX_DB_PATH")
        .env_remove("CORTEX_USE_HTTP")
        .env_remove("CORTEX_LLM")
        .env_remove("CORTEX_LLM_ENABLED")
        .env_remove("CORTEX_CODEX_CMD");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn reflect_json(home: &Path, extra: &[&str], env: &[(&str, &str)]) -> serde_json::Value {
    let mut args = vec!["reflect", "--json", "--kinds", "skill"];
    args.extend_from_slice(extra);
    let output = cortex(home, &args, env);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn false_program() -> PathBuf {
    let paths = cortex::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&paths)
        .map(|dir| dir.join("false"))
        .find(|candidate| candidate.is_file())
        .expect("a `false` program on PATH")
}

#[test]
fn reflect_indexes_detects_and_reports_incrementally() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());

    let report = reflect_json(home.path(), &["--no-llm"], &[]);
    assert_eq!(report["mode"], "report_only");
    assert!(report["db_path"].as_str().unwrap().ends_with(".cortex/reflect.db"));
    assert!(report["index"]["ingested"].as_u64().unwrap() >= 2);
    let top = &report["assessed"][0]["incident"];
    assert_eq!(top["kind"], "skill");
    assert_eq!(top["target"], "cortex:cortex-troubleshoot");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(home.path().join(".cortex")).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let db_mode = std::fs::metadata(home.path().join(".cortex/reflect.db")).unwrap().permissions().mode();
        assert_eq!(db_mode & 0o777, 0o600);
    }

    let again = reflect_json(home.path(), &["--no-llm"], &[]);
    assert_eq!(again["index"]["ingested"], 0, "second run must be incremental");
    assert_eq!(again["assessed"][0]["incident"]["target"], "cortex:cortex-troubleshoot");
}

#[test]
fn a_failing_llm_program_keeps_the_report() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let program = false_program();
    let report = reflect_json(
        home.path(),
        &["--max-assess", "1"],
        &[("CORTEX_LLM", "codex"), ("CORTEX_CODEX_CMD", program.to_str().unwrap())],
    );
    let entry = &report["assessed"][0];
    assert!(entry["assessment"].is_null());
    assert!(entry["failure"].is_string(), "expected a failure reason: {entry}");
    assert!(entry["findings"].is_object());
    assert!(report["llm_fallback_reason"].is_null());
}

#[test]
fn a_missing_llm_program_downgrades_the_run() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let report = reflect_json(
        home.path(),
        &["--max-assess", "1"],
        &[("CORTEX_LLM", "codex"), ("CORTEX_CODEX_CMD", "/nonexistent/cortex-reflect/codex")],
    );
    assert_eq!(report["mode"], "report_only");
    assert!(report["llm_fallback_reason"].as_str().unwrap().contains("was not found"));
    assert!(report["assessed"][0]["findings"].is_object());
}

#[test]
fn reflect_rejects_http_mode() {
    let home = tempfile::tempdir().unwrap();
    let output = cortex(home.path(), &["--http", "reflect"], &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runs locally"));
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test reflect_cli`
Expected: PASS (4 tests).

If the first test finds no skill incident, run the binary by hand against a temp HOME with the same fixture and check `cortex sessions skills --json` with `CORTEX_DB_PATH` set to the temp DB. Adjust only the fixture lines to the Claude shapes in `src/scanner_tests.rs`; never change product code to fit the fixture.

- [ ] **Step 3: Commit**

```bash
cargo fmt && cargo clippy --all-targets
git add tests/reflect_cli.rs
git commit -m "test(reflect): end-to-end cortex reflect incl. LLM failure paths

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Docs and `just reflect`

**Files:**
- Modify: `README.md`, `docs/runbooks/skill-reflection.md`, `CLAUDE.md`, `Justfile`

- [ ] **Step 1: README section**

In `README.md`, after the "Skill and abuse assessment" section, add:

````markdown
### One-shot reflection report

`cortex reflect` runs the whole skill, MCP, and hook reflection loop locally.
It needs no server and no tokens.

```bash
cortex reflect                          # last 7 days, LLM on the top 5 incidents
cortex reflect --no-llm                 # deterministic report only, no LLM program needed
cortex reflect --kinds skill,hook --since 30d --max-assess 10 > reflect.md
cortex reflect --json | jq '.unassessed[].incident_id'
```

It indexes transcripts modified in the window (Claude, Codex, Gemini,
Antigravity), ranks incidents from all selected kinds together, and assesses
the highest-scoring ones with the LLM selected by `CORTEX_LLM`. Results go to
`~/.cortex/reflect.db` (owner-only) unless you pass `--db` or set
`CORTEX_DB_PATH`. Each unassessed incident lists the
`cortex assess ... --incident-id` command that assesses it on its own.
````

- [ ] **Step 2: Runbook note**

Append to `docs/runbooks/skill-reflection.md`:

```markdown
## One-shot report

`cortex reflect` wraps indexing, skill/MCP/hook incident detection, and
assessment into one local command. The Codex app-server requirements above
apply to its LLM step. With `--no-llm` it needs no Codex or Gemini program.
If the LLM program is missing, `reflect` still produces a report and names
the reason at the top. If one kind's LLM action is disabled or its circuit
opens, only that kind falls back to deterministic findings.
```

- [ ] **Step 3: CLAUDE.md Commands entry**

In `CLAUDE.md`, in the first `## Commands` code block, add after the `cortex assess abuse` line:

```bash
cortex reflect [--since 7d] [--kinds skill,mcp,hook] [--no-llm] [--max-assess 5] [--db PATH] [--json]  # one-shot local reflection report
```

- [ ] **Step 4: Justfile recipe**

In `Justfile`, next to `dev`:

```just
# One-shot local skill/MCP/hook reflection report (pass flags through)
reflect *ARGS:
    cargo run --quiet -- reflect {{ARGS}}
```

- [ ] **Step 5: Verify and commit**

Run: `just --list | grep reflect`
Expected: `reflect *ARGS` listed.

Run: `cargo test --lib docs`
Expected: PASS.

```bash
git add README.md docs/runbooks/skill-reflection.md CLAUDE.md Justfile
git commit -m "docs(reflect): document cortex reflect and add just reflect

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Full gate

- [ ] **Step 1: Run the full suite and lints**

```bash
cargo fmt --check
cargo clippy --all-targets
just test
cargo xtask check-version-sync
```

Expected: all pass. No version bump; release-please derives it from the `feat` commits.

- [ ] **Step 2: Push and close**

```bash
git push
bd close unraid-mcp-nh9s --reason="cortex reflect implemented per docs/superpowers/specs/2026-09-11-cortex-reflect-design.md"
```
