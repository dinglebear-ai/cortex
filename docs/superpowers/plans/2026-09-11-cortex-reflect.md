# `cortex reflect` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local-only `cortex reflect` command that indexes local AI transcripts, finds skill, MCP, and hook incidents, ranks them together, optionally LLM-assesses the top N, and prints one Markdown or JSON report.

**Architecture:** `reflect` is orchestration over existing, tested code. The only change to existing services is an optional `incident_id` on the three assess requests (Task 1). New code is a small model module (types, ranking, summary), one service pipeline, one pure LLM-failure classifier, one Markdown renderer, and CLI wiring. `main.rs` builds a query-only runtime pointed at the reflect database.

**Tech Stack:** Rust 2024 (MSRV 1.97.1), tokio, rusqlite/r2d2, serde/serde_json, chrono. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-11-cortex-reflect-design.md`
**Tracking:** beads `unraid-mcp-nh9s`

## Global Constraints

- Kinds are exactly `skill`, `mcp`, `hook`. No `abuse`, no built-in tools, no subagents.
- Defaults: window `--since 7d`, `--max-assess 5`, database `~/.cortex/reflect.db`.
- Database resolution order: `--db PATH`, then `CORTEX_DB_PATH`, then `~/.cortex/reflect.db`.
- `reflect` is local-only. `--http`, `--server`, `--token`, and `CORTEX_USE_HTTP=1` are rejected.
- `--max-assess N` sets how many top incidents get a detailed section. The LLM runs on those unless `--no-llm`. `0` means summary and table only.
- Exit code is 0 whenever a report is produced, including partial LLM failure.
- No new detectors, scoring, or prompts. All `LlmRunner` guards stay in force.
- Modules use sibling `foo.rs` next to `foo/`, never `foo/mod.rs`. Tests live in sidecar `*_tests.rs` files wired with `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`.
- Every Rust module stays under 500 lines (lefthook gate).
- Read environment variables through `crate::env::var_os` (library) or `cortex::env::var_os` (binary), never `std::env::var*`.
- `cargo fmt` and `cargo clippy` must pass before each commit.
- Conventional Commit messages. Every commit ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/app/models/{skill,mcp,hook}_assess.rs` | Modify | Optional `incident_id` on each assess request |
| `src/app/services/{skill,mcp,hook}_assessment.rs` | Modify | Forward `incident_id`; relax "target required" guard |
| `src/app/services/seed_test_support.rs` | Create | Test-only seeding helpers for skill, MCP, and hook incidents |
| `src/cli/args/assess.rs`, `src/cli/parse/assess.rs`, `src/cli/dispatch_sessions.rs` | Modify | `--incident-id` flag on `assess skill/mcp/hooks` |
| `src/app/models/reflect.rs` (+ `_tests.rs`) | Create | Reflect types, `From` conversions, ranking, summary |
| `src/app/services/reflect_llm.rs` (+ `_tests.rs`) | Create | Pure LLM-failure classifier and PATH lookup |
| `src/app/services/reflect.rs` (+ `_tests.rs`) | Create | `CortexService::run_reflect` pipeline |
| `src/app/reflect_report.rs` (+ `_tests.rs`) | Create | Markdown renderer |
| `src/runtime.rs` | Modify | `RuntimeCore::query_only_with_retry` |
| `src/cli/args/reflect.rs`, `src/cli/parse/reflect.rs` (+ `_tests.rs`) | Create | `ReflectArgs` and parser |
| `src/cli/dispatch_reflect.rs` (+ `_tests.rs`) | Create | Dispatch, output writing, DB path resolution |
| `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/run.rs`, `src/cli.rs`, `src/main.rs`, `src/surfaces.rs`, `src/cli/help.rs`, `src/cli/help_tests.rs` | Modify | Wire the new command |
| `tests/reflect_cli.rs` | Create | End-to-end binary test with transcript fixtures |
| `README.md`, `docs/runbooks/skill-reflection.md`, `CLAUDE.md`, `Justfile` | Modify | Docs and `just reflect` |

---

### Task 1: Target a single incident in `assess skill|mcp|hooks`

**Files:**
- Modify: `src/app/models/skill_assess.rs`, `src/app/models/mcp_assess.rs`, `src/app/models/hook_assess.rs`
- Modify: `src/app/services/skill_assessment.rs`, `src/app/services/mcp_assessment.rs`, `src/app/services/hook_assessment.rs`
- Create: `src/app/services/seed_test_support.rs`
- Modify: `src/app/services.rs` (module declaration)
- Modify: `src/cli/args/assess.rs`, `src/cli/parse/assess.rs`, `src/cli/dispatch_sessions.rs`, `src/cli/help.rs`
- Test: `src/app/services/skill_assessment_tests.rs`, `src/app/services/mcp_assessment_tests.rs`, `src/app/services/hook_assessment_tests.rs`, `src/cli/parse/assess_tests.rs`

**Interfaces:**
- Produces: `SkillAssessRequest.incident_id`, `McpAssessRequest.incident_id`, `HookAssessRequest.incident_id`, all `Option<String>`. When set, the service assesses exactly that incident.
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

In `src/app/services.rs`, add next to the other `mod` lines (after `mod rag;` keeps rough alphabetical order):

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

Append to `src/app/services/mcp_assessment_tests.rs` (reuse that file's existing `test_service()` helper; if the file names it differently, use that name):

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

If `mcp_assessment_tests.rs` or `hook_assessment_tests.rs` has no `test_service()` helper, add this one at the top of that file (identical to the one in `skill_assessment_tests.rs`):

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
Expected: compile error, "struct `SkillAssessRequest` has no field named `incident_id`" (and the same for MCP and hook).

- [ ] **Step 4: Add `incident_id` to the three request models**

In each of `SkillAssessRequest` (`src/app/models/skill_assess.rs`), `McpAssessRequest` (`src/app/models/mcp_assess.rs`), and `HookAssessRequest` (`src/app/models/hook_assess.rs`), add as the first field:

```rust
    /// Assess exactly this incident (as returned by the matching
    /// `*_incidents` listing). When set, the target fields may be empty.
    /// Skipped when `None` for the same serde_qs reason as
    /// `AiSkillInvestigateRequest::incident_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
```

- [ ] **Step 5: Forward `incident_id` in the three services**

In `src/app/services/skill_assessment.rs`, `run_skill_assessment_with_delta`:

```rust
        if req.incident_id.is_none() && req.skill.is_none() && req.plugin.is_none() {
            return Err(ServiceError::InvalidInput(
                "assess skill requires a skill name, --plugin, or --incident-id".to_string(),
            ));
        }
```

and in the `AiSkillInvestigateRequest` literal replace `incident_id: None,` with `incident_id: req.incident_id.clone(),`. In the "no skill incident found" branch, build the description with the incident id first:

```rust
            let skill_desc = req
                .incident_id
                .clone()
                .or_else(|| req.skill.clone())
                .or_else(|| req.plugin.clone().map(|p| format!("plugin:{p}")))
                .unwrap_or_default();
```

In `src/app/services/mcp_assessment.rs`, `run_mcp_assessment_with_delta`:

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

replace `incident_id: None,` in the `AiMcpInvestigateRequest` literal with `incident_id: req.incident_id.clone(),`, and start `target_desc` with `req.incident_id.clone().or_else(|| req.mcp_server.clone())`.

In `src/app/services/hook_assessment.rs`, `run_hook_assessment_with_delta`: replace `incident_id: None,` in the `AiHookInvestigateRequest` literal with `incident_id: req.incident_id.clone(),`, and start `hook_desc` with `req.incident_id.clone().or_else(|| req.hook_name.clone())`.

- [ ] **Step 6: Fix every existing struct literal**

Run: `grep -rn 'SkillAssessRequest {\|McpAssessRequest {\|HookAssessRequest {' src tests`
For every literal that lists all fields without `..Default::default()`, add `incident_id: None,`. Leave the three new tests alone.

- [ ] **Step 7: Run the service tests to verify they pass**

Run: `cargo test --lib assessment`
Expected: PASS, including the three new `incident_id_targets_exactly_one_*` tests.

- [ ] **Step 8: Write the failing CLI parser tests**

Append to `src/cli/parse/assess_tests.rs`:

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

If `assess_tests.rs` does not already import them, add `use super::super::super::args::{AssessCommand, CliCommand};` at the top (match the file's existing import style).

- [ ] **Step 9: Run the parser tests to verify they fail**

Run: `cargo test --bin cortex assess_`
Expected: compile error, "no field `incident_id`" on `AssessSkillArgs`.

- [ ] **Step 10: Add the flag**

In `src/cli/args/assess.rs`, add `pub incident_id: Option<String>,` as the first field of `AssessSkillArgs`, `AssessMcpArgs`, and `AssessHooksArgs`.

In `src/cli/parse/assess.rs`:
- In `parse_assess_skill_from`, `parse_assess_mcp_from`, and `parse_assess_hooks`, add the match arm `"--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),` and add `"--incident-id"` to each `suggest::unknown_option` list.
- Relax the skill guard to `if parsed.skill.is_none() && parsed.plugin.is_none() && parsed.incident_id.is_none() {` and change its message to `"assess skill: skill name, --plugin, or --incident-id is required, e.g. ..."` keeping the existing examples.
- Relax the MCP guard to `if parsed.target.is_none() && parsed.server.is_none() && parsed.tool_name.is_none() && parsed.incident_id.is_none() {`.

In `src/cli/dispatch_sessions.rs`, in the request literals built by `run_assess_skill`, `run_assess_mcp`, and `run_assess_hooks`, add `incident_id: args.incident_id.clone(),`.

In `src/cli/help.rs`, in the `assess` `CommandDoc`, insert `[--incident-id ID] ` after `SKILL ` in the first skill line and after `hooks ` in the hooks line, and add this usage line after the two skill lines:

```rust
            "cortex assess skill --incident-id ID [--no-llm] [--json]",
```

- [ ] **Step 11: Run the CLI tests to verify they pass**

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
- Create: `src/app/models/reflect.rs`
- Create: `src/app/models/reflect_tests.rs`
- Modify: `src/app/models.rs`, `src/app.rs`

**Interfaces:**
- Consumes: `SkillIncident`, `McpIncident`, `HookIncident` (in scope via `use super::*`), `crate::scanner::IndexResult`.
- Produces (all `pub`, re-exported from `cortex::app`):
  - `enum ReflectKind { Skill, Mcp, Hook }` with `ALL: [ReflectKind; 3]`, `as_str(self) -> &'static str`, `parse(&str) -> Option<Self>`, `assess_subcommand(self) -> &'static str`. Serializes lowercase.
  - `struct ReflectRequest { since, until, project, tool: Option<String>, kinds: Vec<ReflectKind>, run_llm: bool, max_assess: u32, index: bool }`
  - `struct ReflectIncident { kind, incident_id, target, tool, project, session_id, last_seen: String, priority_score: f64, priority_label: String, signals_present: Vec<String> }` with `assess_command(&self, since: Option<&str>) -> String` and `From<SkillIncident|McpIncident|HookIncident>`.
  - `enum ReflectMode { ReportOnly, ReportAndLlm }` (snake_case).
  - `struct ReflectIndexSummary { discovered_files, ingested, skipped_dupes, parse_errors: usize }` with `From<&IndexResult>`.
  - `struct ReflectKindSummary { kind, total, critical, high, medium, low }`
  - `struct ReflectAssessed { incident: ReflectIncident, assessment: Option<String>, findings: serde_json::Value, failure: Option<String> }`
  - `struct ReflectReport { since, until, project, tool: Option<String>, kinds: Vec<ReflectKind>, db_path: String, mode: ReflectMode, llm_fallback_reason: Option<String>, index: Option<ReflectIndexSummary>, summary: Vec<ReflectKindSummary>, assessed: Vec<ReflectAssessed>, unassessed: Vec<ReflectIncident> }`
  - `fn rank_reflect_incidents(Vec<ReflectIncident>) -> Vec<ReflectIncident>`
  - `fn summarize_reflect_incidents(&[ReflectKind], &[ReflectIncident]) -> Vec<ReflectKindSummary>`

- [ ] **Step 1: Write the failing tests**

Create `src/app/models/reflect_tests.rs`:

```rust
use super::*;

fn incident(kind: ReflectKind, id: &str, score: f64, label: &str, last_seen: &str) -> ReflectIncident {
    ReflectIncident {
        kind,
        incident_id: id.to_string(),
        target: format!("target-{id}"),
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
fn kind_serializes_lowercase() {
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
fn summary_counts_per_selected_kind_including_empty_kinds() {
    let incidents = vec![
        incident(ReflectKind::Skill, "a", 70.0, "critical", "t"),
        incident(ReflectKind::Skill, "b", 20.0, "medium", "t"),
        incident(ReflectKind::Hook, "c", 5.0, "low", "t"),
    ];
    let summary = summarize_reflect_incidents(&ReflectKind::ALL, &incidents);
    assert_eq!(summary.len(), 3);
    assert_eq!((summary[0].kind, summary[0].total, summary[0].critical, summary[0].medium), (ReflectKind::Skill, 2, 1, 1));
    assert_eq!((summary[1].kind, summary[1].total), (ReflectKind::Mcp, 0));
    assert_eq!((summary[2].kind, summary[2].total, summary[2].low), (ReflectKind::Hook, 1, 1));
}

#[test]
fn assess_command_includes_since_when_given() {
    let hook = incident(ReflectKind::Hook, "abc", 1.0, "low", "t");
    assert_eq!(hook.assess_command(None), "cortex assess hooks --incident-id abc");
    assert_eq!(
        hook.assess_command(Some("2026-09-04T00:00:00Z")),
        "cortex assess hooks --incident-id abc --since 2026-09-04T00:00:00Z"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib models::reflect`
Expected: compile error, the module `reflect` does not exist yet (the test file is not wired). Continue to Step 3.

- [ ] **Step 3: Implement the module**

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
    /// Human-readable target: `plugin:skill`, `server/tool`, or `event:hook`.
    pub target: String,
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
    pub fn assess_command(&self, since: Option<&str>) -> String {
        let mut command = format!(
            "cortex assess {} --incident-id {}",
            self.kind.assess_subcommand(),
            self.incident_id
        );
        if let Some(since) = since {
            command.push_str(" --since ");
            command.push_str(since);
        }
        command
    }
}

impl From<SkillIncident> for ReflectIncident {
    fn from(incident: SkillIncident) -> Self {
        let target = match &incident.skill_plugin {
            Some(plugin) => format!("{plugin}:{}", incident.skill_name),
            None => incident.skill_name.clone(),
        };
        Self {
            kind: ReflectKind::Skill,
            incident_id: incident.incident_id,
            target,
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
        let target = match &incident.mcp_tool {
            Some(tool) => format!("{}/{tool}", incident.mcp_server),
            None => incident.mcp_server.clone(),
        };
        Self {
            kind: ReflectKind::Mcp,
            incident_id: incident.incident_id,
            target,
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
        let target = match &incident.hook_name {
            Some(name) => format!("{}:{name}", incident.hook_event),
            None => incident.hook_event.clone(),
        };
        Self {
            kind: ReflectKind::Hook,
            incident_id: incident.incident_id,
            target,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectKindSummary {
    pub kind: ReflectKind,
    pub total: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectAssessed {
    pub incident: ReflectIncident,
    /// LLM assessment Markdown. `None` in report-only mode or on failure.
    pub assessment: Option<String>,
    /// The kind's deterministic findings, serialized.
    pub findings: serde_json::Value,
    /// Why the LLM assessment is missing, when it was attempted or skipped
    /// because the circuit breaker opened.
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

/// One row per selected kind, in the order given, including empty kinds.
pub fn summarize_reflect_incidents(
    kinds: &[ReflectKind],
    incidents: &[ReflectIncident],
) -> Vec<ReflectKindSummary> {
    kinds
        .iter()
        .map(|&kind| {
            let mut summary = ReflectKindSummary {
                kind,
                total: 0,
                critical: 0,
                high: 0,
                medium: 0,
                low: 0,
            };
            for incident in incidents.iter().filter(|incident| incident.kind == kind) {
                summary.total += 1;
                match incident.priority_label.as_str() {
                    "critical" => summary.critical += 1,
                    "high" => summary.high += 1,
                    "medium" => summary.medium += 1,
                    _ => summary.low += 1,
                }
            }
            summary
        })
        .collect()
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
```

In `src/app/models.rs`, add `mod reflect;` next to `mod skill_assess;`, and `pub use reflect::*;` next to the other `pub use ...::*;` lines.

In `src/app.rs`, add these names to the `pub use models::{ ... }` list:

```rust
    ReflectAssessed,
    ReflectIncident,
    ReflectIndexSummary,
    ReflectKind,
    ReflectKindSummary,
    ReflectMode,
    ReflectReport,
    ReflectRequest,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib models::reflect`
Expected: PASS (6 tests).

- [ ] **Step 5: Lint and commit**

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
- Modify: `src/app/services.rs`

**Interfaces:**
- Consumes: Task 1 `incident_id` on assess requests and `seed_test_support`; Task 2 types, `rank_reflect_incidents`, `summarize_reflect_incidents`.
- Produces: `CortexService::run_reflect<P: FnMut(&str) + Send>(&self, req: ReflectRequest, progress: P) -> ServiceResult<ReflectReport>`.
- Produces (crate-private): `reflect_llm::{ReflectLlmFailure, classify_llm_failure, program_on_path}`.

Note: the spec's "stub LLM runner" fallback tests are implemented as pure tests of the classifier and PATH lookup. The pipeline tests cover report-only behavior end to end. This avoids faking the LLM subprocess while still covering each fallback decision.

- [ ] **Step 1: Write the failing classifier tests**

Create `src/app/services/reflect_llm_tests.rs`:

```rust
use super::*;

fn internal(error: LlmRunnerError) -> ServiceError {
    ServiceError::Internal(anyhow::anyhow!(error))
}

#[test]
fn disabled_llm_makes_the_run_unavailable() {
    assert!(matches!(classify_llm_failure(&internal(LlmRunnerError::Disabled)), ReflectLlmFailure::Unavailable(_)));
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::ActionDisabled("skill_assess".into()))),
        ReflectLlmFailure::Unavailable(_)
    ));
}

#[test]
fn open_circuit_stops_further_calls() {
    let error = internal(LlmRunnerError::CircuitOpen {
        action: "skill_assess".into(),
        retry_after: "2026-09-11T00:05:00Z".into(),
    });
    let ReflectLlmFailure::CircuitOpen(reason) = classify_llm_failure(&error) else {
        panic!("expected CircuitOpen");
    };
    assert!(reason.contains("circuit open"));
}

#[test]
fn timeouts_and_rate_limits_fail_one_incident() {
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::Timeout("id".into(), 120))),
        ReflectLlmFailure::Failed(_)
    ));
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::RateLimited { action: "a".into(), detail: "d".into() })),
        ReflectLlmFailure::Failed(_)
    ));
}

#[test]
fn non_llm_errors_propagate() {
    assert_eq!(
        classify_llm_failure(&ServiceError::InvalidInput("bad".into())),
        ReflectLlmFailure::NotLlm
    );
}

#[test]
fn program_on_path_finds_real_and_rejects_missing_binaries() {
    assert!(program_on_path("sh"));
    assert!(program_on_path("/bin/sh"));
    assert!(!program_on_path("cortex-reflect-definitely-missing-binary"));
    assert!(!program_on_path(""));
}
```

- [ ] **Step 2: Implement the classifier**

Create `src/app/services/reflect_llm.rs`:

```rust
//! Pure helpers that decide how `cortex reflect` reacts when an LLM
//! assessment cannot run.

use crate::app::ServiceError;
use crate::app::llm_runner::LlmRunnerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReflectLlmFailure {
    /// The LLM cannot run at all: downgrade the whole run to report-only.
    Unavailable(String),
    /// The circuit breaker opened: no more LLM calls this run.
    CircuitOpen(String),
    /// Only this assessment failed: keep going.
    Failed(String),
    /// Not an LLM failure: propagate the error.
    NotLlm,
}

/// Assessment services wrap `LlmRunnerError` as `ServiceError::Internal`
/// (see `run_llm_with_delta` in `src/app/services.rs`).
pub(crate) fn classify_llm_failure(error: &ServiceError) -> ReflectLlmFailure {
    let ServiceError::Internal(inner) = error else {
        return ReflectLlmFailure::NotLlm;
    };
    match inner.downcast_ref::<LlmRunnerError>() {
        Some(runner @ (LlmRunnerError::Disabled | LlmRunnerError::ActionDisabled(_))) => {
            ReflectLlmFailure::Unavailable(runner.to_string())
        }
        Some(runner @ LlmRunnerError::CircuitOpen { .. }) => {
            ReflectLlmFailure::CircuitOpen(runner.to_string())
        }
        Some(runner) => ReflectLlmFailure::Failed(runner.to_string()),
        None => ReflectLlmFailure::Failed(format!("{inner:#}")),
    }
}

/// True when `program` is an existing file path, or a bare name found in a
/// `PATH` directory.
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

In `src/app/services.rs`, add `mod reflect;` and `mod reflect_llm;` after `mod rag;`.

- [ ] **Step 3: Run the classifier tests**

Run: `cargo test --lib reflect_llm`
Expected: PASS (5 tests). Step 4 adds `reflect.rs`; until then, temporarily comment out `mod reflect;` if it blocks compilation.

- [ ] **Step 4: Write the failing pipeline tests**

Create `src/app/services/reflect_tests.rs`:

```rust
use std::sync::Arc;

use super::*;
use crate::app::models::{ReflectKind, ReflectMode, ReflectRequest};
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
    assert_eq!(report.assessed.len() + report.unassessed.len(), 3);
    for summary in &report.summary {
        assert_eq!(summary.total, 1, "{:?} should have one incident", summary.kind);
    }
    let scores: Vec<f64> = report
        .assessed
        .iter()
        .map(|a| a.incident.priority_score)
        .chain(report.unassessed.iter().map(|i| i.priority_score))
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "not ranked: {scores:?}");
    for assessed in &report.assessed {
        assert_eq!(assessed.assessment, None);
        assert_eq!(assessed.failure, None);
        assert!(assessed.findings.is_object());
    }
    assert!(progress.iter().any(|line| line.starts_with("assessing 1/2")));

    let llm_rows: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM llm_invocations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(llm_rows, 0, "report-only must never call the LLM");
}

#[tokio::test]
async fn kinds_filter_limits_detection() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service.run_reflect(request(vec![ReflectKind::Hook], 5), |_| {}).await.unwrap();
    assert_eq!(report.summary.len(), 1);
    assert!(report.assessed.iter().all(|a| a.incident.kind == ReflectKind::Hook));
    assert_eq!(report.assessed.len(), 1);
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
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test --lib services::reflect::`
Expected: compile error, no method `run_reflect` on `CortexService`.

- [ ] **Step 6: Implement the pipeline**

Create `src/app/services/reflect.rs`:

```rust
//! `cortex reflect`: index local transcripts, list skill/MCP/hook incidents,
//! rank them together, and assess the top N through the existing guarded
//! assessment services. Orchestration only: no new detection or prompts.

use super::reflect_llm::{ReflectLlmFailure, classify_llm_failure, program_on_path};
use super::*;
use crate::app::models::{
    AiHookIncidentRequest, AiMcpIncidentRequest, AiSkillIncidentRequest, HookAssessRequest,
    McpAssessRequest, ReflectAssessed, ReflectIncident, ReflectIndexSummary, ReflectKind,
    ReflectMode, ReflectReport, ReflectRequest, SkillAssessRequest, rank_reflect_incidents,
    summarize_reflect_incidents,
};

/// Upper bound on incidents listed per kind before ranking.
const REFLECT_LIST_LIMIT: u32 = 500;

impl CortexService {
    pub async fn run_reflect<P>(
        &self,
        req: ReflectRequest,
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

        let index = if req.index {
            progress("indexing local AI transcripts");
            let result = self.index_ai_roots(None, false, None).await?;
            Some(ReflectIndexSummary::from(&result))
        } else {
            None
        };

        let mut incidents = Vec::new();
        for kind in &req.kinds {
            progress(&format!("detecting {} incidents", kind.as_str()));
            incidents.extend(self.list_reflect_incidents(*kind, &req).await?);
        }
        let mut ranked = rank_reflect_incidents(incidents);
        let summary = summarize_reflect_incidents(&req.kinds, &ranked);
        let take = (req.max_assess as usize).min(ranked.len());
        let unassessed = ranked.split_off(take);

        let mut mode = if req.run_llm {
            ReflectMode::ReportAndLlm
        } else {
            ReflectMode::ReportOnly
        };
        let mut llm_fallback_reason = None;
        if mode == ReflectMode::ReportAndLlm && take > 0 {
            if let Some(reason) = self.reflect_llm_preflight() {
                mode = ReflectMode::ReportOnly;
                llm_fallback_reason = Some(reason);
            }
        }

        let mut circuit_reason: Option<String> = None;
        let mut assessed = Vec::with_capacity(take);
        for (position, incident) in ranked.into_iter().enumerate() {
            let use_llm = mode == ReflectMode::ReportAndLlm && circuit_reason.is_none();
            progress(&format!(
                "assessing {}/{take}: {} {}",
                position + 1,
                incident.kind.as_str(),
                incident.target
            ));
            let entry = match self.assess_reflect_incident(&incident, &req, use_llm).await {
                Ok((assessment, findings)) => ReflectAssessed {
                    incident,
                    assessment,
                    findings,
                    failure: circuit_reason.clone(),
                },
                Err(error) => {
                    let failure = match classify_llm_failure(&error) {
                        ReflectLlmFailure::NotLlm => return Err(error),
                        ReflectLlmFailure::Unavailable(reason) => {
                            mode = ReflectMode::ReportOnly;
                            llm_fallback_reason = Some(reason);
                            None
                        }
                        ReflectLlmFailure::CircuitOpen(reason) => {
                            circuit_reason = Some(reason.clone());
                            Some(reason)
                        }
                        ReflectLlmFailure::Failed(reason) => Some(reason),
                    };
                    let (_, findings) = self.assess_reflect_incident(&incident, &req, false).await?;
                    ReflectAssessed {
                        incident,
                        assessment: None,
                        findings,
                        failure,
                    }
                }
            };
            assessed.push(entry);
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

    /// `Some(reason)` when the LLM backend cannot run on this host.
    fn reflect_llm_preflight(&self) -> Option<String> {
        match self.llm().backend(None) {
            Err(error) => Some(format!(
                "LLM backend could not be resolved ({error}); set CORTEX_LLM to codex or gemini"
            )),
            Ok(backend) => {
                let program = backend.program();
                let executable = program.split_whitespace().next().unwrap_or_default().to_string();
                (!program_on_path(&executable)).then(|| {
                    format!(
                        "LLM backend binary '{executable}' was not found on PATH; install it, \
                         set CORTEX_LLM, or pass --no-llm"
                    )
                })
            }
        }
    }

    async fn list_reflect_incidents(
        &self,
        kind: ReflectKind,
        req: &ReflectRequest,
    ) -> ServiceResult<Vec<ReflectIncident>> {
        let limit = Some(REFLECT_LIST_LIMIT);
        Ok(match kind {
            ReflectKind::Skill => self
                .list_ai_skill_incidents(AiSkillIncidentRequest {
                    tool: req.tool.clone(),
                    project: req.project.clone(),
                    since: req.since.clone(),
                    until: req.until.clone(),
                    limit,
                    ..Default::default()
                })
                .await?
                .incidents
                .into_iter()
                .map(ReflectIncident::from)
                .collect(),
            ReflectKind::Mcp => self
                .list_ai_mcp_incidents(AiMcpIncidentRequest {
                    tool: req.tool.clone(),
                    project: req.project.clone(),
                    since: req.since.clone(),
                    until: req.until.clone(),
                    limit,
                    ..Default::default()
                })
                .await?
                .incidents
                .into_iter()
                .map(ReflectIncident::from)
                .collect(),
            ReflectKind::Hook => self
                .list_ai_hook_incidents(AiHookIncidentRequest {
                    tool: req.tool.clone(),
                    project: req.project.clone(),
                    since: req.since.clone(),
                    until: req.until.clone(),
                    limit,
                    ..Default::default()
                })
                .await?
                .incidents
                .into_iter()
                .map(ReflectIncident::from)
                .collect(),
        })
    }

    /// Runs the kind's assessment service for exactly one incident. Uses the
    /// same window and filters as the listing so the incident id resolves.
    async fn assess_reflect_incident(
        &self,
        incident: &ReflectIncident,
        req: &ReflectRequest,
        run_llm: bool,
    ) -> ServiceResult<(Option<String>, serde_json::Value)> {
        let incident_id = Some(incident.incident_id.clone());
        match incident.kind {
            ReflectKind::Skill => {
                let response = self
                    .run_skill_assessment_with_delta(
                        SkillAssessRequest {
                            incident_id,
                            tool: req.tool.clone(),
                            project: req.project.clone(),
                            since: req.since.clone(),
                            until: req.until.clone(),
                            ..Default::default()
                        },
                        run_llm,
                        |_| Ok(()),
                    )
                    .await?;
                let result = first_result(response.results, &incident.incident_id)?;
                Ok((result.assessment, findings_json(&result.findings)?))
            }
            ReflectKind::Mcp => {
                let response = self
                    .run_mcp_assessment_with_delta(
                        McpAssessRequest {
                            incident_id,
                            tool: req.tool.clone(),
                            project: req.project.clone(),
                            since: req.since.clone(),
                            until: req.until.clone(),
                            ..Default::default()
                        },
                        run_llm,
                        |_| Ok(()),
                    )
                    .await?;
                let result = first_result(response.results, &incident.incident_id)?;
                Ok((result.assessment, findings_json(&result.findings)?))
            }
            ReflectKind::Hook => {
                let response = self
                    .run_hook_assessment_with_delta(
                        HookAssessRequest {
                            incident_id,
                            tool: req.tool.clone(),
                            project: req.project.clone(),
                            since: req.since.clone(),
                            until: req.until.clone(),
                            ..Default::default()
                        },
                        run_llm,
                        |_| Ok(()),
                    )
                    .await?;
                let result = first_result(response.results, &incident.incident_id)?;
                Ok((result.assessment, findings_json(&result.findings)?))
            }
        }
    }
}

fn first_result<T>(results: Vec<T>, incident_id: &str) -> ServiceResult<T> {
    results.into_iter().next().ok_or_else(|| {
        ServiceError::NotFound(format!(
            "incident {incident_id} disappeared between detection and assessment"
        ))
    })
}

fn findings_json<T: serde::Serialize>(findings: &T) -> ServiceResult<serde_json::Value> {
    serde_json::to_value(findings)
        .map_err(|error| ServiceError::Internal(anyhow::anyhow!("serialize findings: {error}")))
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
```

If you commented out `mod reflect;` in Step 3, restore it.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --lib reflect`
Expected: PASS (all `models::reflect`, `reflect_llm`, and `services::reflect` tests).

If `report_only_merges_kinds_ranks_and_caps_detail` finds zero incidents for a kind, check that kind's listing with `since: None` returns the seeded row: run `cargo test --lib incident_id_targets_exactly_one` from Task 1. Those tests seed the same way and must pass first.

- [ ] **Step 8: Check the module size gate and commit**

Run: `wc -l src/app/services/reflect.rs src/app/services/reflect_llm.rs`
Expected: each under 500.

```bash
cargo fmt && cargo clippy --all-targets
git add src/app/services.rs src/app/services/reflect.rs src/app/services/reflect_tests.rs src/app/services/reflect_llm.rs src/app/services/reflect_llm_tests.rs
git commit -m "feat(reflect): add run_reflect pipeline with LLM fallbacks

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Markdown report renderer

**Files:**
- Create: `src/app/reflect_report.rs`, `src/app/reflect_report_tests.rs`
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Task 2 `ReflectReport` and related types.
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
        tool: "claude".to_string(),
        project: "/p".to_string(),
        session_id: "sess-1".to_string(),
        last_seen: "2026-09-10T12:00:00Z".to_string(),
        priority_score: 42.0,
        priority_label: "high".to_string(),
        signals_present: vec!["user_correction_after_skill".to_string()],
    }
}

fn report(assessed: Vec<ReflectAssessed>, unassessed: Vec<ReflectIncident>) -> ReflectReport {
    ReflectReport {
        since: Some("2026-09-04T00:00:00Z".to_string()),
        until: None,
        project: None,
        tool: None,
        kinds: ReflectKind::ALL.to_vec(),
        db_path: "/home/u/.cortex/reflect.db".to_string(),
        mode: ReflectMode::ReportOnly,
        llm_fallback_reason: None,
        index: Some(ReflectIndexSummary { discovered_files: 3, ingested: 10, skipped_dupes: 0, parse_errors: 1 }),
        summary: vec![ReflectKindSummary { kind: ReflectKind::Skill, total: 1, critical: 0, high: 1, medium: 0, low: 0 }],
        assessed,
        unassessed,
    }
}

#[test]
fn report_only_shows_summary_findings_and_other_incidents() {
    let md = render_reflect_markdown(&report(
        vec![ReflectAssessed {
            incident: incident(ReflectKind::Skill, "abc", "lavra:lavra-plan"),
            assessment: None,
            findings: serde_json::json!({"likely_failure_modes": ["x"]}),
            failure: None,
        }],
        vec![incident(ReflectKind::Mcp, "def", "labby/search")],
    ));
    assert!(md.starts_with("# Cortex reflect report\n"));
    assert!(md.contains("Mode: report only"));
    assert!(md.contains("Index: 3 files discovered, 10 new records, 0 duplicates skipped, 1 parse errors"));
    assert!(md.contains("| skill | 1 | 0 | 1 | 0 | 0 |"));
    assert!(md.contains("## 1. skill `lavra:lavra-plan` (high, score 42.0)"));
    assert!(md.contains("```json\n"));
    assert!(md.contains("\"likely_failure_modes\""));
    assert!(md.contains("## Other incidents"));
    assert!(md.contains("`cortex assess mcp --incident-id def --since 2026-09-04T00:00:00Z`"));
}

#[test]
fn llm_assessment_headings_are_demoted_under_the_incident() {
    let mut rep = report(
        vec![ReflectAssessed {
            incident: incident(ReflectKind::Skill, "abc", "s"),
            assessment: Some("## Incident Summary\nText\n### Detail\n".to_string()),
            findings: serde_json::json!({}),
            failure: None,
        }],
        vec![],
    );
    rep.mode = ReflectMode::ReportAndLlm;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("Mode: report + LLM"));
    assert!(md.contains("\n#### Incident Summary\n"));
    assert!(md.contains("\n##### Detail\n"));
    assert!(!md.contains("```json"));
}

#[test]
fn failures_and_fallbacks_are_visible() {
    let mut rep = report(
        vec![ReflectAssessed {
            incident: incident(ReflectKind::Hook, "h1", "PostToolUse:fmt"),
            assessment: None,
            findings: serde_json::json!({}),
            failure: Some("LLM invocation 'x' timed out after 120s".to_string()),
        }],
        vec![],
    );
    rep.llm_fallback_reason = Some("LLM backend binary 'codex' was not found on PATH".to_string());
    rep.index = None;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("> LLM skipped: LLM backend binary 'codex' was not found on PATH"));
    assert!(md.contains("- Assessment failed: LLM invocation 'x' timed out after 120s"));
    assert!(md.contains("Index: skipped (--no-index)"));
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
fn pipes_in_targets_are_escaped_in_tables() {
    let md = render_reflect_markdown(&report(vec![], vec![incident(ReflectKind::Mcp, "p", "a|b")]));
    assert!(md.contains("`a\\|b`"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib reflect_report`
Expected: compile error, module `reflect_report` not found (after adding `pub mod reflect_report;` in Step 3 it becomes "cannot find function `render_reflect_markdown`").

- [ ] **Step 3: Implement the renderer**

Create `src/app/reflect_report.rs`:

```rust
//! Markdown rendering for `cortex reflect`. Pure: no I/O.

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

fn write_header(md: &mut String, report: &ReflectReport) {
    let since = report.since.as_deref().unwrap_or("beginning");
    let until = report.until.as_deref().unwrap_or("now");
    let kinds: Vec<&str> = report.kinds.iter().map(|kind| kind.as_str()).collect();
    let _ = writeln!(md, "Window: {since} to {until}  ");
    let _ = writeln!(
        md,
        "Filters: project={}, tool={}, kinds={}  ",
        report.project.as_deref().unwrap_or("all"),
        report.tool.as_deref().unwrap_or("all"),
        kinds.join(", ")
    );
    let _ = writeln!(md, "Database: `{}`  ", report.db_path);
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
        let _ = writeln!(md, "\n> LLM skipped: {reason}");
    }
    md.push('\n');
}

fn write_summary(md: &mut String, report: &ReflectReport) {
    md.push_str("## Summary\n\n");
    if report.assessed.is_empty() && report.unassessed.is_empty() {
        md.push_str("No skill, MCP, or hook incidents found in this window.\n\n");
        return;
    }
    md.push_str("| Kind | Total | Critical | High | Medium | Low |\n");
    md.push_str("|------|-------|----------|------|--------|-----|\n");
    for row in &report.summary {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} |",
            row.kind.as_str(),
            row.total,
            row.critical,
            row.high,
            row.medium,
            row.low
        );
    }
    md.push('\n');
}

fn write_assessed(md: &mut String, position: usize, assessed: &ReflectAssessed) {
    let incident = &assessed.incident;
    let _ = writeln!(
        md,
        "## {position}. {} `{}` ({}, score {:.1})\n",
        incident.kind.as_str(),
        incident.target,
        incident.priority_label,
        incident.priority_score
    );
    let _ = writeln!(md, "- Incident: `{}`", incident.incident_id);
    let _ = writeln!(
        md,
        "- Session: `{}` ({}, {})",
        incident.session_id, incident.tool, incident.project
    );
    let _ = writeln!(md, "- Last seen: {}", incident.last_seen);
    if !incident.signals_present.is_empty() {
        let _ = writeln!(md, "- Signals: {}", incident.signals_present.join(", "));
    }
    if let Some(failure) = &assessed.failure {
        let _ = writeln!(md, "- Assessment failed: {failure}");
    }
    md.push('\n');
    match &assessed.assessment {
        Some(text) => {
            md.push_str(&demote_headings(text));
            if !md.ends_with('\n') {
                md.push('\n');
            }
        }
        None => {
            let findings = serde_json::to_string_pretty(&assessed.findings)
                .unwrap_or_else(|_| "{}".to_string());
            let _ = writeln!(md, "```json\n{findings}\n```");
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
            "| {} | `{}` | {} | {:.1} | {} | `{}` |",
            incident.kind.as_str(),
            incident.target.replace('|', "\\|"),
            incident.priority_label,
            incident.priority_score,
            incident.last_seen,
            incident.assess_command(report.since.as_deref())
        );
    }
    md.push('\n');
}

/// Pushes LLM headings two levels down so they nest under the H2 incident
/// heading.
fn demote_headings(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with('#') {
                format!("##{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "reflect_report_tests.rs"]
mod tests;
```

In `src/app.rs`, add `pub mod reflect_report;` next to the other `pub mod` lines.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib reflect_report`
Expected: PASS (5 tests).

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/app.rs src/app/reflect_report.rs src/app/reflect_report_tests.rs
git commit -m "feat(reflect): render reflect reports as markdown

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: `cortex reflect` CLI wiring

**Files:**
- Create: `src/cli/args/reflect.rs`
- Create: `src/cli/parse/reflect.rs`, `src/cli/parse/reflect_tests.rs`
- Create: `src/cli/dispatch_reflect.rs`, `src/cli/dispatch_reflect_tests.rs`
- Modify: `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/run.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/surfaces.rs`, `src/cli/help.rs`, `src/cli/help_tests.rs`

**Interfaces:**
- Consumes: `cortex::app::{ReflectKind, ReflectRequest}`, `CortexService::run_reflect`, `cortex::app::reflect_report::render_reflect_markdown`.
- Produces: `CliCommand::Reflect(ReflectArgs)`; `cli::resolve_reflect_db_path(Option<&Path>, Option<OsString>, Option<OsString>) -> anyhow::Result<PathBuf>`; `RuntimeCore::query_only_with_retry(Config) -> Result<RuntimeCore>`.

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
    assert!(!parsed.no_llm);
    assert!(!parsed.no_index);
    assert!(!parsed.json);
    assert_eq!(parsed.max_assess, DEFAULT_REFLECT_MAX_ASSESS);
    assert_eq!(parsed.db, None);
    assert_eq!(parsed.out, None);
    assert!(!parsed.since.is_empty(), "default --since 7d must be normalized");
}

#[test]
fn every_flag_is_parsed() {
    let parsed = parse(&[
        "--since", "2026-09-01T00:00:00Z", "--until", "2026-09-02T00:00:00Z",
        "--project", "/p", "--tool", "codex", "--kinds", "hook,skill",
        "--no-llm", "--max-assess", "0", "--no-index", "--db", "/tmp/r.db",
        "--json", "--out", "/tmp/r.md",
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
    assert_eq!(parsed.out, Some(std::path::PathBuf::from("/tmp/r.md")));
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
fn unknown_option_is_rejected() {
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
    let path = resolve_reflect_db_path(
        Some(Path::new("/flag.db")),
        Some(OsString::from("/env.db")),
        Some(OsString::from("/home/u")),
    )
    .unwrap();
    assert_eq!(path, PathBuf::from("/flag.db"));
}

#[test]
fn env_wins_over_home() {
    let path = resolve_reflect_db_path(None, Some(OsString::from("/env.db")), Some(OsString::from("/home/u"))).unwrap();
    assert_eq!(path, PathBuf::from("/env.db"));
}

#[test]
fn empty_env_falls_back_to_home_default() {
    let path = resolve_reflect_db_path(None, Some(OsString::new()), Some(OsString::from("/home/u"))).unwrap();
    assert_eq!(path, PathBuf::from("/home/u/.cortex/reflect.db"));
}

#[test]
fn missing_home_is_an_error_that_mentions_db_flag() {
    let error = resolve_reflect_db_path(None, None, None).unwrap_err().to_string();
    assert!(error.contains("--db"), "{error}");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --bin cortex reflect`
Expected: compile errors, the new test files are not wired yet and `ReflectArgs` does not exist. Continue.

- [ ] **Step 4: Add `ReflectArgs` and the command variant**

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
    pub out: Option<PathBuf>,
}
```

In `src/cli/args.rs`: add `mod reflect;` next to `mod assess;`, add `pub(crate) use reflect::ReflectArgs;` next to the `pub(crate) use assess::{...}` block, and add the variant `Reflect(ReflectArgs),` after `Assess(AssessCommand),` in `CliCommand`.

- [ ] **Step 5: Add the parser**

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
    "--out",
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
        out: None,
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
            "--out" => parsed.out = Some(PathBuf::from(flags.value("--out")?)),
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

In `src/cli/parse.rs`: add `mod reflect;` next to `mod assess;`, `use self::reflect::parse_reflect;` next to `use self::assess::parse_assess;`, and the arm `"reflect" => parse_reflect(rest),` after `"assess" => parse_assess(rest),`.

- [ ] **Step 6: Add dispatch and DB path resolution**

Create `src/cli/dispatch_reflect.rs`:

```rust
//! `cortex reflect` dispatch: runs the local pipeline and writes the report.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cortex::app::ReflectRequest;
use cortex::app::reflect_report::render_reflect_markdown;

use super::CliMode;
use super::args::ReflectArgs;

pub(crate) async fn run_reflect(mode: &CliMode, args: ReflectArgs) -> Result<()> {
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
        eprintln!("[reflect] warning: {reason}; the report contains deterministic findings only");
    }
    let body = if args.json {
        let mut json = serde_json::to_string_pretty(&report)?;
        json.push('\n');
        json
    } else {
        render_reflect_markdown(&report)
    };
    match &args.out {
        Some(path) => std::fs::write(path, &body)
            .with_context(|| format!("writing reflect report to {}", path.display()))?,
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(body.as_bytes())?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Reflect database: `--db`, then `CORTEX_DB_PATH`, then `~/.cortex/reflect.db`.
/// Environment values are passed in so the order is unit-testable.
pub(crate) fn resolve_reflect_db_path(
    flag: Option<&Path>,
    env_db_path: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env_db_path.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = home
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("cannot resolve ~/.cortex/reflect.db because HOME is not set; pass --db PATH"))?;
    Ok(PathBuf::from(home).join(".cortex").join("reflect.db"))
}

#[cfg(test)]
#[path = "dispatch_reflect_tests.rs"]
mod tests;
```

In `src/cli.rs`: add `mod dispatch_reflect;` next to `mod dispatch_sessions;`, and `pub(crate) use dispatch_reflect::resolve_reflect_db_path;` next to the file's other `pub(crate) use` lines.

In `src/cli/run.rs`, add this arm after the `CliCommand::Assess(command) => match command { ... },` arm:

```rust
        CliCommand::Reflect(args) => super::dispatch_reflect::run_reflect(&mode, args).await,
```

- [ ] **Step 7: Add the retrying query-only constructor**

In `src/runtime.rs`, split `load_query_only` so the retry loop takes a prepared `Config`. Move the existing `for attempt in 0..3 { ... }` loop and everything after it in `load_query_only` unchanged into the new function:

```rust
    pub async fn load_query_only() -> Result<Self> {
        // Use load_for_stdio() to skip the non-loopback bind safety gate —
        // stdio mode never binds an HTTP port so the gate is irrelevant.
        Self::query_only_with_retry(Config::load_for_stdio()?).await
    }

    /// Query-only runtime for a caller-prepared config, retrying briefly
    /// when another writer holds the SQLite lock.
    pub async fn query_only_with_retry(config: Config) -> Result<Self> {
        // <the existing `for attempt in 0..3 { ... }` loop and tail, unchanged>
    }
```

The loop body already calls `Self::query_only(config.clone())`, so it compiles unchanged.

- [ ] **Step 8: Build the reflect runtime in `main.rs`**

In `src/main.rs`, insert this block immediately before the comment `// Build CliMode ONCE per invocation`:

```rust
    if let cli::CliCommand::Reflect(args) = &command {
        if let Some(trigger) = flags.http_trigger() {
            anyhow::bail!("cortex reflect runs locally; remove {trigger}");
        }
        let mut config = cortex::config::Config::load_for_stdio()?;
        config.storage.db_path = cli::resolve_reflect_db_path(
            args.db.as_deref(),
            cortex::env::var_os("CORTEX_DB_PATH"),
            cortex::env::var_os("HOME"),
        )?;
        let runtime = RuntimeCore::query_only_with_retry(config).await?;
        return cli::run(cli::CliMode::Local(runtime.service()), command).await;
    }
```

`init_pool` creates the parent directory, so `~/.cortex` does not need creating here.

- [ ] **Step 9: Register the surface and help**

In `src/surfaces.rs`, add after `local_cli!("assess", Sessions, Canonical),`:

```rust
    // One-shot local reflection report; may run the local LLM like `assess`.
    local_cli!("reflect", Sessions, Canonical),
```

Run: `grep -n 'CLI_ROOTS' src/surfaces.rs`. If `CLI_ROOTS` is a literal list, add `"reflect",` after `"assess",`. If it is derived from `SURFACE_SPECS`, do nothing.

In `src/cli/help.rs`, change the group line to `("AI Transcripts", &["sessions", "assess", "reflect"]),` and add after the `assess` `CommandDoc`:

```rust
    CommandDoc {
        name: "reflect",
        summary: "One-shot local skill, MCP, and hook reflection report (local-only)",
        usage: &[
            "cortex reflect [--since TIME] [--until TIME] [--project PATH] [--tool TOOL] [--kinds skill,mcp,hook] [--no-llm] [--max-assess N] [--no-index] [--db PATH] [--json] [--out FILE]",
        ],
    },
```

In `src/cli/help_tests.rs`, add `"reflect",` after `"assess",` in `PARSER_TOKENS`.

- [ ] **Step 10: Run the CLI tests**

Run: `cargo test --bin cortex`
Expected: PASS, including the 6 parser tests and 4 DB path tests. If a completion or surface-catalog test fails because it enumerates root commands, add `reflect` to that list next to `assess` and rerun.

Run: `cargo test --lib surfaces`
Expected: PASS.

- [ ] **Step 11: Smoke test by hand**

```bash
cargo run --quiet -- reflect --no-llm --db "$(mktemp -d)/reflect.db" --max-assess 2
```

Expected: `[reflect] indexing local AI transcripts` and `[reflect] detecting ...` on stderr, then a Markdown report starting with `# Cortex reflect report` on stdout. Exit code 0.

```bash
cargo run --quiet -- reflect --http
```

Expected: non-zero exit with `cortex reflect runs locally; remove --http`.

- [ ] **Step 12: Lint and commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/cli src/cli.rs src/main.rs src/runtime.rs src/surfaces.rs
git commit -m "feat(reflect): add cortex reflect command

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: End-to-end binary test with transcript fixtures

**Files:**
- Create: `tests/reflect_cli.rs`

**Interfaces:**
- Consumes: the `cortex` binary from Task 5 via `env!("CARGO_BIN_EXE_cortex")`.

- [ ] **Step 1: Write the test**

Create `tests/reflect_cli.rs`:

```rust
//! End-to-end: `cortex reflect` indexes real Claude transcript lines from a
//! temporary HOME into the default reflect DB and reports a skill incident.

use std::path::Path;
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

fn reflect(home: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cortex"))
        .arg("reflect")
        .args(extra)
        .current_dir(home) // keep the repo's config.toml out of the run
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env_remove("CORTEX_DB_PATH")
        .env_remove("CORTEX_USE_HTTP")
        .env_remove("CORTEX_LLM")
        .output()
        .unwrap()
}

#[test]
fn reflect_indexes_detects_and_reports_incrementally() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());

    let first = reflect(home.path(), &["--no-llm", "--json", "--kinds", "skill"]);
    assert!(first.status.success(), "stderr: {}", String::from_utf8_lossy(&first.stderr));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["mode"], "report_only");
    assert!(report["db_path"].as_str().unwrap().ends_with(".cortex/reflect.db"));
    assert!(home.path().join(".cortex/reflect.db").is_file());
    assert!(report["index"]["ingested"].as_u64().unwrap() >= 2);
    let top = &report["assessed"][0]["incident"];
    assert_eq!(top["kind"], "skill");
    assert_eq!(top["target"], "cortex:cortex-troubleshoot");

    let second = reflect(home.path(), &["--no-llm", "--json", "--kinds", "skill"]);
    assert!(second.status.success());
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["index"]["ingested"], 0, "second run must be incremental");
    assert_eq!(report["assessed"][0]["incident"]["target"], "cortex:cortex-troubleshoot");
}

#[test]
fn reflect_markdown_goes_to_out_file() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let out = home.path().join("report.md");
    let result = reflect(home.path(), &["--no-llm", "--out", out.to_str().unwrap()]);
    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert!(result.stdout.is_empty());
    let md = std::fs::read_to_string(out).unwrap();
    assert!(md.starts_with("# Cortex reflect report\n"));
    assert!(md.contains("cortex-troubleshoot"));
}

#[test]
fn reflect_rejects_http_mode() {
    let home = tempfile::tempdir().unwrap();
    let result = reflect(home.path(), &["--http"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("runs locally"));
}
```

`--http` is a global flag. If the parser only accepts it before the command name, change the last test's call to `Command::new(...).args(["--http", "reflect"])` by adding a variant helper, keeping the same assertions.

- [ ] **Step 2: Run the test**

Run: `cargo test --test reflect_cli`
Expected: PASS (3 tests).

If the first test finds no skill incident, run the same binary with `--no-index` removed and `--kinds skill` against the temp HOME, then inspect with `cortex sessions skills --json` using `CORTEX_DB_PATH` set to the temp DB. Check that the Claude parser read `timestamp` and `attributionSkill` from the fixture lines. Adjust only the fixture lines, never the product code, to match the Claude shapes in `src/scanner_tests.rs` (search for `attributionSkill`).

- [ ] **Step 3: Commit**

```bash
cargo fmt && cargo clippy --all-targets
git add tests/reflect_cli.rs
git commit -m "test(reflect): end-to-end cortex reflect with transcript fixtures

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Docs and `just reflect`

**Files:**
- Modify: `README.md`, `docs/runbooks/skill-reflection.md`, `CLAUDE.md`, `Justfile`

- [ ] **Step 1: README section**

In `README.md`, find the "Skill and abuse assessment" section and add a subsection after it:

````markdown
### One-shot reflection report

`cortex reflect` runs the whole skill, MCP, and hook reflection loop locally.
It needs no server and no tokens.

```bash
cortex reflect                     # last 7 days, LLM on the top 5 incidents
cortex reflect --no-llm            # deterministic report only, no LLM binary needed
cortex reflect --kinds skill,hook --since 30d --max-assess 10 --out reflect.md
cortex reflect --json | jq '.unassessed[].incident_id'
```

It indexes local transcript roots (Claude, Codex, Gemini, Antigravity), ranks
incidents from all selected kinds together, and assesses the highest-scoring
ones with the LLM selected by `CORTEX_LLM`. Results go to its own database,
`~/.cortex/reflect.db`, unless you pass `--db` or set `CORTEX_DB_PATH`.
Each unassessed incident lists the `cortex assess ... --incident-id` command
that assesses it on its own.
````

- [ ] **Step 2: Runbook note**

Append to `docs/runbooks/skill-reflection.md`:

```markdown
## One-shot report

`cortex reflect` wraps indexing, skill/MCP/hook incident detection, and
assessment into one local command. The Codex app-server requirements above
apply to its LLM step. With `--no-llm` it needs no Codex or Gemini binary.
If the LLM is disabled or its binary is missing, `reflect` still produces a
report and names the reason at the top.
```

- [ ] **Step 3: CLAUDE.md Commands entry**

In `CLAUDE.md`, in the first `## Commands` code block, add after the `cortex assess abuse` line:

```bash
cortex reflect [--since 7d] [--kinds skill,mcp,hook] [--no-llm] [--max-assess 5] [--db PATH] [--json] [--out FILE]  # one-shot local reflection report
```

- [ ] **Step 4: Justfile recipe**

In `Justfile`, add near the other run recipes (next to `dev`):

```just
# One-shot local skill/MCP/hook reflection report (pass flags through)
reflect *ARGS:
    cargo run --quiet -- reflect {{ARGS}}
```

- [ ] **Step 5: Verify and commit**

Run: `just --list | grep reflect`
Expected: `reflect *ARGS` is listed.

Run: `cargo test --test docs_tests 2>/dev/null; cargo test --lib docs`
Expected: PASS (docs consistency tests, if they cover README or CLAUDE.md).

```bash
git add README.md docs/runbooks/skill-reflection.md CLAUDE.md Justfile
git commit -m "docs(reflect): document cortex reflect and add just reflect

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Full gate

- [ ] **Step 1: Run the full hermetic suite and lints**

```bash
cargo fmt --check
cargo clippy --all-targets
just test
cargo xtask check-version-sync
```

Expected: all pass. No version bump is needed; release-please derives it from the `feat` commits.

- [ ] **Step 2: Push the branch and close the issue**

```bash
git push
bd close unraid-mcp-nh9s --reason="cortex reflect implemented per docs/superpowers/specs/2026-09-11-cortex-reflect-design.md"
```
