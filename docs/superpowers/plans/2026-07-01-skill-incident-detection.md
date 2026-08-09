# Skill Incident Detection, Investigation, and Deterministic Findings (GH #94 PR 3/4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect negative-signal patterns following a skill event, group them into scored incidents with a stable incident id, and build a bounded investigation evidence bundle with deterministic (non-LLM) findings.

**Architecture:** Five deterministic, phrase-boundary signal detectors (`src/app/skill_signal_detectors.rs`) classify transcript log messages as anchors for `user_correction_after_skill`, `tool_failure_after_skill`, `scope_or_source_confusion`, `ignored_skill_or_policy_instruction`, and `overlong_loop_after_skill`. A DB-layer query (`search_ai_skill_incidents`) groups `ai_skill_events` rows by `(skill_name, skill_plugin, tool, project, session_id, hostname, window_bucket)`, runs the detectors over the session's nearby transcript rows, and scores/sorts each group into a `SkillIncident` with a stable synthetic `incident_id`. A second DB-layer query (`investigate_ai_skill_incidents`) expands a `SkillIncident` into a bounded, truncation-flagged `SkillIncidentEvidence` bundle (skill events, signal anchors, transcript before/after, nearby tool failures/corrections/logs/errors), which the app layer enriches with `SkillIncidentFindings` computed by a pure rule-evaluation module (`src/app/skill_incident_findings.rs`) — no DB or LLM calls. All of this is exposed identically through MCP actions (`skill_incidents`, `skill_investigate`), REST routes, and a skill-first CLI (`cortex sessions skill-investigate <skill>`) that resolves the top-priority incident for a skill and summarizes the rest via `other_matching_incidents`.

**Tech Stack:** Rust 2024 edition, `rusqlite` (bundled SQLite, WAL mode), `serde`/`serde_json` for wire types.

## Global Constraints

- **This PR depends on PR 2 (Skill Event Extraction) being merged first** — it queries the `ai_skill_events` table and consumes `AiSkillEventParams`/`AiSkillEventEntry` from `docs/superpowers/plans/2026-07-01-skill-event-extraction.md`. Before starting Task 1, `grep -rn "AiSkillEventEntry\|ai_skill_events" src/` to confirm PR 2 has actually landed and its real field names match what this plan assumes (see the source file's own "verify against actual code" callout at the top of Task 1).
- Float score sorting (`priority_score: f64`) must use `f64::total_cmp()`, not `partial_cmp()` — `partial_cmp` returns `None` on NaN and silently corrupts sort order.
- Bounded evidence with explicit truncation flags on every collection — never silently claim complete evidence when caps were hit.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass before any task is considered done.
- Every new MCP action needs a row in `src/mcp/actions.rs` (`ACTION_SPECS`) + a dispatch arm in `src/mcp/tools.rs` + docs updates (`docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, `docs/contracts/mcp-actions-current.md`, `CLAUDE.md` action table + count).
- **PR sequencing note:** This is PR 3 of 4 for GH #94 Plan A. PR 4 (skill assessment + unified `cortex assess` CLI) depends on this PR (needs `SkillIncident`/`AiSkillInvestigateResponse` evidence types) and on PR 1 (needs `LlmRunner`) — do not implement PR 4 until this PR and PR 1 are both merged.

---


## Verify-against-actual-code callout (read before Task 1)

This phase assumes the prior "skill events" phase has landed:

- Table `ai_skill_events` with columns `id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp, skill_name, skill_plugin, event_kind, evidence_kind, created_at` (per PR 2's eng review pass, `skill_path` and `metadata_json` were dropped before implementation — neither extractor ever set them — so the shipped table does NOT have them; re-verify with the grep below since this note may itself be stale by the time PR 3 is implemented), created inline in `src/db/pool.rs` (this repo's migrations are `CREATE TABLE IF NOT EXISTS` blocks in `init_pool`/`run_migrations`, NOT a separate migrations directory — confirm by grepping `CREATE TABLE IF NOT EXISTS` in `src/db/pool.rs`).
- DB-layer params/entry types, assumed named `AiSkillEventParams` and `AiSkillEventEntry` in `src/db/models.rs`, with a query function (assumed `search_ai_skill_events` or similar) in `src/db/queries.rs`.
- App-layer mirrors in `src/app/models/` (likely `src/app/models/ai_skill_events.rs` or folded into an existing `ai_*` model file) and a service method on `src/app/services/ai.rs`.

**Before writing Task 1**, grep the actual repo for the real names:

```bash
grep -rn "ai_skill_events\|AiSkillEvent" src/db/models.rs src/db/queries.rs src/app/models/ src/app/services/ src/mcp/actions.rs src/mcp/tools.rs
```

If the real struct/field/function names differ from the assumptions above (e.g. `SkillEventParams` instead of `AiSkillEventParams`, or the table lives under a different column set), **update every reference in this phase's tasks to match the real names before implementing** — do not silently proceed with mismatched names. The column set assumed above (`id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp, skill_name, skill_plugin, event_kind, evidence_kind, created_at` — NOT `skill_path`/`metadata_json`, dropped by PR 2's eng review pass; see the callout above) is treated as ground truth for all SQL in this plan; adjust column names in every `SELECT`/`INSERT`/index statement below if the actual schema differs.

This plan also assumes indexes named (or equivalent to):
- `idx_ai_skill_events_skill_time` on `(skill_name, timestamp)`
- `idx_ai_skill_events_session_time` on `(ai_tool, ai_project, ai_session_id, timestamp)`
- `idx_ai_skill_events_project_skill_time` on `(ai_project, skill_name, timestamp)`

If those index names differ, the `EXPLAIN QUERY PLAN` assertions in Task 3's regression test must be updated to match the real index name (SQLite's `EXPLAIN QUERY PLAN` output embeds the literal index name — a mismatched literal will fail even if the plan is otherwise correct).

## Locked interfaces for other phases

These are the structs/functions this phase defines. A later "skill LLM assessment" phase will serialize `AiSkillInvestigateResult` (specifically each `SkillIncidentEvidence` inside it) into an LLM prompt — field names here are final.

```rust
// src/app/skill_incident_findings.rs — pure, no DB/LLM calls
pub const SKILL_SCOPE_MISMATCH: &str = "skill_scope_mismatch";
pub const MISSING_PREREQUISITE_CHECK: &str = "missing_prerequisite_check";
pub const WRONG_SOURCE_OF_TRUTH: &str = "wrong_source_of_truth";
pub const OVERLY_BROAD_RESEARCH_LOOP: &str = "overly_broad_research_loop";
pub const TOOL_POLICY_MISMATCH: &str = "tool_policy_mismatch";
pub const MISSING_VERIFICATION_STEP: &str = "missing_verification_step";
pub const AMBIGUOUS_SKILL_TRIGGER: &str = "ambiguous_skill_trigger";
pub const STALE_OR_CONFLICTING_SKILL_INSTRUCTION: &str = "stale_or_conflicting_skill_instruction";
pub const ASSISTANT_OVEREXPLAINED_SIMPLE_ANSWER: &str = "assistant_overexplained_simple_answer";
pub const UNKNOWN: &str = "unknown";

pub struct SkillFailureMode {
    pub category: String,       // one of the consts above
    pub confidence: String,     // "low" | "medium" | "high"
    pub evidence_ids: Vec<i64>, // log row ids (from logs table, via log_id or anchor ids)
}

pub struct SkillContributingFactor {
    pub factor: String,
    pub evidence_ids: Vec<i64>,
}

pub struct SkillPreventionHint {
    pub category: String,
    pub hint: String, // skill-doc-actionable text
}

pub struct SkillIncidentFindings {
    pub likely_failure_modes: Vec<SkillFailureMode>,
    pub contributing_factors: Vec<SkillContributingFactor>,
    pub prevention_hints: Vec<SkillPreventionHint>,
    pub open_questions: Vec<String>,
}

pub fn derive_skill_incident_findings(
    incident: &SkillIncident,
    skill_events: &[AiSkillEventEntry],
    signal_anchors: &[LogEntry],
    transcript_before: &[LogEntry],
    transcript_after: &[LogEntry],
    nearby_logs: &[LogEntry],
    nearby_errors: &[LogEntry],
) -> SkillIncidentFindings;
```

```rust
// src/db/models.rs — DB-layer grouping/scoring types
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillIncidentParams {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,          // default 20, clamp 1..=100
    pub window_minutes: Option<u32>, // default 10, clamp 1..=120
    pub signals: Vec<String>,        // filter to these anchor signal categories only; empty = all
    pub min_score: Option<f64>,      // filter incidents with priority_score >= min_score
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncident {
    pub incident_id: String,       // "skill-inc-{:016x}" stable hash, see Task 3
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub skill_event_count: usize,
    pub skill_event_ids: Vec<i64>,       // sorted ai_skill_events.id
    pub anchor_log_ids: Vec<i64>,        // sorted logs.id backing the anchor signals
    pub signal_counts: SkillSignalCounts,
    pub signals_present: Vec<String>,    // sorted distinct signal category names
    pub priority_score: f64,
    pub priority_label: String,          // "low" | "medium" | "high" | "critical"
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillSignalCounts {
    pub user_correction_after_skill: usize,
    pub tool_failure_after_skill: usize,
    pub scope_or_source_confusion: usize,
    pub ignored_skill_or_policy_instruction: usize,
    pub overlong_loop_after_skill: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillIncidentResult {
    pub incidents: Vec<SkillIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillInvestigateParams {
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,                     // max incidents to investigate, default 3, clamp 1..=10
    pub window_minutes: Option<u32>,             // default 10, clamp 1..=120
    pub correlation_window_minutes: Option<u32>, // default 5, clamp 1..=120
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentEvidence {
    pub incident: SkillIncident,
    pub skill_events: Vec<AiSkillEventEntry>,      // cap 25
    pub skill_events_truncated: bool,
    pub signal_anchors: Vec<LogEntry>,             // cap 50
    pub signal_anchors_truncated: bool,
    pub transcript_before: Vec<LogEntry>,          // cap 20
    pub transcript_before_truncated: bool,
    pub transcript_after: Vec<LogEntry>,           // cap 20
    pub transcript_after_truncated: bool,
    pub nearby_tool_failures: Vec<LogEntry>,       // subset of nearby_logs, cap 25
    pub nearby_tool_failures_truncated: bool,
    pub nearby_user_corrections: Vec<LogEntry>,    // subset of nearby_logs, cap 25
    pub nearby_user_corrections_truncated: bool,
    pub nearby_logs: Vec<LogEntry>,                // cap 50
    pub nearby_logs_truncated: bool,
    pub nearby_errors: Vec<LogEntry>,              // cap 25
    pub nearby_errors_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillInvestigateResult {
    pub evidence: Vec<SkillIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
    /// Populated only in the skill-first CLI/MCP/REST path when more than one
    /// incident matches the requested skill/plugin but only the top one (or
    /// `limit`) was returned in `evidence`.
    pub other_matching_incidents: Vec<SkillIncidentSummary>,
    /// True when skill events exist for the filter but no incident/negative
    /// signal was found — `evidence` then holds a single low-severity summary
    /// bundle instead of an error.
    pub no_incident_low_severity_summary: bool,
    /// True when no `ai_skill_events` rows at all matched the filter.
    pub no_data: bool,
    pub suggested_filters: Vec<String>, // populated only when no_data is true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentSummary {
    pub incident_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
}
```

App-layer (`src/app/models/ai_skill_incidents.rs`) mirrors these 1:1 with `From<db::X>` impls, exactly like `src/app/models/ai_incidents.rs` mirrors `db::AbuseIncident`/`db::IncidentEvidence` — same field names, plus `findings: skill_incident_findings::SkillIncidentFindings` attached inside the `From<db::SkillIncidentEvidence>` conversion (mirrors how `IncidentEvidence::from` in `src/app/models/ai_incidents.rs:104-137` attaches `incident_findings::derive_incident_findings(...)`).

MCP action names: `skill_incidents`, `skill_investigate` (both `Scope::Read`, added to `ACTION_SPECS` in `src/mcp/actions.rs` and `ActionHandler` enum).

CLI surface: `cortex sessions skill-incidents` and `cortex sessions skill-investigate <skill>` (this repo's CLI convention places all AI-transcript subcommands under `cortex sessions <subcommand>` — the top-level `cortex ai` alias was explicitly removed, see `removed_top_level_commands_fail_with_replacement_guidance` in `src/cli/parse_tests.rs`. The phase brief's `cortex ai skill-investigate ...` examples are illustrative; the real, wired command is `cortex sessions skill-investigate ...`. Task 8 documents this explicitly).

REST routes: `GET /api/sessions/skill-incidents`, `GET /api/sessions/skill-investigate`.

---

### Task 1: Signal-anchor detector — `user_correction_after_skill` and `tool_failure_after_skill`

**Files:**
- Create: `src/app/skill_signal_detectors.rs`
- Modify: `src/app/mod.rs` (add `pub mod skill_signal_detectors;` near existing `pub mod incident_findings;` — grep `mod incident_findings` in `src/app/mod.rs` to find the exact insertion point)
- Test: `src/app/skill_signal_detectors_tests.rs` (sidecar convention: `src/app/skill_signal_detectors.rs` ends with `#[cfg(test)] #[path = "skill_signal_detectors_tests.rs"] mod tests;`)

**Interfaces:**
- Consumes: `crate::app::models::LogEntry` (existing, from `src/app/models.rs` or wherever `LogEntry` is re-exported at the app layer — verify via `grep -n "pub struct LogEntry" src/app/models*.rs src/db/models.rs`; it is defined once in `src/db/models.rs` and re-exported through `src/app/models.rs`'s `pub use db::LogEntry;` pattern — confirm with `grep -n "LogEntry" src/app/models.rs`).
- Produces: `pub const SIGNAL_USER_CORRECTION_AFTER_SKILL: &str = "user_correction_after_skill";`, `pub const SIGNAL_TOOL_FAILURE_AFTER_SKILL: &str = "tool_failure_after_skill";`, `pub fn detect_user_correction(message: &str) -> bool`, `pub fn detect_tool_failure(message: &str) -> bool`. These are consumed by Task 3's grouping/scoring logic and Task 4's evidence-bundle nearby_tool_failures/nearby_user_corrections split.

- [ ] **Step 1: Write the failing test** (full real code)

Create `src/app/skill_signal_detectors_tests.rs`:

```rust
use super::*;

#[test]
fn detects_direct_correction_phrases() {
    let positives = [
        "That's not what I asked for, please redo it.",
        "You said you would run the tests but you didn't.",
        "This is just wrong, revert it.",
        "No, that's the wrong file entirely.",
        "We wasted twenty minutes on this dead end.",
        "All you had to say was 'I don't know'.",
        "Stop, you're going in circles.",
        "You didn't need to touch that config at all.",
    ];
    for msg in positives {
        assert!(detect_user_correction(msg), "expected correction hit for: {msg}");
    }
}

#[test]
fn does_not_flag_unrelated_negatives() {
    let negatives = [
        "No new errors were found in the log scan.",
        "The stop hook fired successfully.",
        "You said the deploy finished — confirming that now.",
        "wrongdoing was not detected in the audit",
    ];
    for msg in negatives {
        assert!(
            !detect_user_correction(msg),
            "unexpected correction hit for: {msg}"
        );
    }
}

#[test]
fn detects_tool_failure_phrases() {
    let positives = [
        "Command exited with exit code 1",
        "bash: permission denied",
        "error: file not found",
        "operation timed out after 30s",
        "failed to connect to database",
        "database is locked",
        "429 rate limit exceeded",
    ];
    for msg in positives {
        assert!(detect_tool_failure(msg), "expected tool-failure hit for: {msg}");
    }
}

#[test]
fn does_not_flag_successful_tool_output_as_failure() {
    let negatives = [
        "build completed successfully in 4.2s",
        "all 42 tests passed",
        "pushed to origin/main",
    ];
    for msg in negatives {
        assert!(
            !detect_tool_failure(msg),
            "unexpected tool-failure hit for: {msg}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib skill_signal_detectors -- --nocapture
```

Expected: compile error (`skill_signal_detectors` module and its functions do not exist yet) — `error[E0433]: failed to resolve: use of undeclared crate or module 'skill_signal_detectors'` or similar, since `src/app/skill_signal_detectors.rs` has not been created yet.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/skill_signal_detectors.rs`:

```rust
//! Deterministic, phrase-boundary keyword detectors for skill-incident anchor
//! signals. Pure functions over log message text — no DB, no LLM. Mirrors the
//! word/phrase-boundary matching style of `first_abuse_term`/`is_abuse_boundary`
//! in `src/db/queries.rs`, but operates on the app layer since these detectors
//! are shared by both the DB-layer grouping query (Task 3) and the evidence
//! bundle nearby-log classification (Task 4).

pub const SIGNAL_USER_CORRECTION_AFTER_SKILL: &str = "user_correction_after_skill";
pub const SIGNAL_TOOL_FAILURE_AFTER_SKILL: &str = "tool_failure_after_skill";
pub const SIGNAL_SCOPE_OR_SOURCE_CONFUSION: &str = "scope_or_source_confusion";
pub const SIGNAL_IGNORED_SKILL_OR_POLICY_INSTRUCTION: &str = "ignored_skill_or_policy_instruction";
pub const SIGNAL_OVERLONG_LOOP_AFTER_SKILL: &str = "overlong_loop_after_skill";

/// All five locked anchor signal categories, in a stable order used for
/// `signals_present` sorting and CLI `--signals` validation.
pub const ALL_SIGNALS: &[&str] = &[
    SIGNAL_USER_CORRECTION_AFTER_SKILL,
    SIGNAL_TOOL_FAILURE_AFTER_SKILL,
    SIGNAL_SCOPE_OR_SOURCE_CONFUSION,
    SIGNAL_IGNORED_SKILL_OR_POLICY_INSTRUCTION,
    SIGNAL_OVERLONG_LOOP_AFTER_SKILL,
];

/// Phrases indicating the user is correcting or pushing back on the assistant
/// immediately after a skill loaded. Deliberately phrase-level (not single
/// words like "no" or "wrong" in isolation) to avoid false positives on
/// unrelated negatives ("no new errors were found").
const USER_CORRECTION_PHRASES: &[&str] = &[
    "that's not what i asked",
    "that is not what i asked",
    "you said",
    "this is wrong",
    "that's wrong",
    "that is wrong",
    "no, that's",
    "no, that is",
    "we wasted",
    "all you had to say",
    "stop, you",
    "you didn't need to",
    "you did not need to",
];

/// Phrases indicating a tool/command failure surfaced in nearby transcript or
/// tool-output text.
const TOOL_FAILURE_PHRASES: &[&str] = &[
    "exit code",
    "permission denied",
    "not found",
    "timed out",
    "failed to",
    "database is locked",
    "rate limit",
];

/// Case-insensitive substring match on whole phrases (already multi-word, so
/// word-boundary checks are unnecessary for most entries — a phrase like
/// "you said" cannot ride inside an unrelated longer word the way a bare
/// single-word term like "hell" could).
fn contains_any_phrase(haystack_lower: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| haystack_lower.contains(p))
}

pub fn detect_user_correction(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, USER_CORRECTION_PHRASES)
}

pub fn detect_tool_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, TOOL_FAILURE_PHRASES)
}

#[cfg(test)]
#[path = "skill_signal_detectors_tests.rs"]
mod tests;
```

Add to `src/app/mod.rs` (find the line `pub mod incident_findings;` — or wherever `mod` declarations for app submodules live — and add immediately after it):

```rust
pub mod skill_signal_detectors;
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib skill_signal_detectors -- --nocapture
```

Expected: `test result: ok. 4 passed; 0 failed` (the four `#[test]` functions in `skill_signal_detectors_tests.rs`).

- [ ] **Step 5: Commit**

```bash
git add src/app/skill_signal_detectors.rs src/app/skill_signal_detectors_tests.rs src/app/mod.rs
git commit -m "feat: add user_correction and tool_failure skill-incident signal detectors"
```

---

### Task 2: Signal-anchor detectors — `scope_or_source_confusion`, `ignored_skill_or_policy_instruction`, `overlong_loop_after_skill`

**Files:**
- Modify: `src/app/skill_signal_detectors.rs` (append to the file created in Task 1)
- Test: `src/app/skill_signal_detectors_tests.rs` (append)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn detect_scope_or_source_confusion(message: &str) -> bool`, `pub fn detect_ignored_instruction(message: &str) -> bool`, `pub fn detect_overlong_loop(skill_event_count: usize, tool_call_count: usize, has_correction_or_frustration_signal: bool) -> bool`. All three consumed by Task 3 grouping and Task 6 findings module.

- [ ] **Step 1: Write the failing test** (full real code)

Append to `src/app/skill_signal_detectors_tests.rs`:

```rust
#[test]
fn detects_scope_or_source_confusion_phrases() {
    let positives = [
        "wait, this is the wrong repo entirely",
        "that data is stale, we're looking at memory not the live system",
        "you're using the wrong source of truth here",
        "this is memory-vs-live confusion, check the running container",
    ];
    for msg in positives {
        assert!(
            detect_scope_or_source_confusion(msg),
            "expected scope/source confusion hit for: {msg}"
        );
    }
}

#[test]
fn does_not_flag_unrelated_text_as_scope_confusion() {
    let negatives = [
        "the repo was cloned successfully",
        "source code review complete",
        "memory usage is within limits",
    ];
    for msg in negatives {
        assert!(
            !detect_scope_or_source_confusion(msg),
            "unexpected scope/source confusion hit for: {msg}"
        );
    }
}

#[test]
fn detects_ignored_instruction_phrases() {
    let positives = [
        "you claimed success without any verification",
        "you should have created a bead for this but didn't",
        "you used the wrong transport for this call",
        "you searched the raw web instead of using axon",
        "you stopped at the plan instead of implementing it",
    ];
    for msg in positives {
        assert!(
            detect_ignored_instruction(msg),
            "expected ignored-instruction hit for: {msg}"
        );
    }
}

#[test]
fn does_not_flag_compliant_text_as_ignored_instruction() {
    let negatives = [
        "verification passed, all tests green",
        "created bead cortex-142 for the follow-up",
        "used axon to research this before answering",
    ];
    for msg in negatives {
        assert!(
            !detect_ignored_instruction(msg),
            "unexpected ignored-instruction hit for: {msg}"
        );
    }
}

#[test]
fn overlong_loop_requires_both_volume_and_negative_signal() {
    // Long-but-successful: many tool calls, no correction/frustration — must NOT trigger.
    assert!(!detect_overlong_loop(3, 40, false));
    // Short loop with a correction — not "overlong", must NOT trigger.
    assert!(!detect_overlong_loop(1, 3, true));
    // Long loop WITH a correction/frustration signal — must trigger.
    assert!(detect_overlong_loop(2, 25, true));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib skill_signal_detectors -- --nocapture
```

Expected: compile error — `detect_scope_or_source_confusion`, `detect_ignored_instruction`, `detect_overlong_loop` are not defined.

- [ ] **Step 3: Write minimal implementation**

Append to `src/app/skill_signal_detectors.rs` (before the `#[cfg(test)]` block at the end — move the existing trailing `#[cfg(test)] #[path = ...] mod tests;` to the very end of the file after this new code):

```rust
/// Conservative phrases indicating the assistant is operating on the wrong
/// repo, stale data, wrong source of truth, or confusing an in-memory/cached
/// view with the live system. Starts from the same conservative style as
/// `UNCLEAR_INSTRUCTION_OR_SCOPE_DRIFT` in `src/app/incident_findings.rs`
/// plus skill-specific additions for source-of-truth confusion.
const SCOPE_OR_SOURCE_CONFUSION_PHRASES: &[&str] = &[
    "wrong repo",
    "wrong file",
    "stale data",
    "wrong source",
    "source of truth",
    "memory-vs-live",
    "memory vs live",
    "not the live",
    "going in circles",
];

/// Explicit-violation phrases for a fixed set of known instruction
/// categories: no verification after implementation, no issue/bead when
/// required, wrong transport/source, raw web when Axon/Labby required,
/// stopped at plan when asked to implement. Deliberately requires explicit
/// phrase evidence — no broad single-word matches.
const IGNORED_INSTRUCTION_PHRASES: &[&str] = &[
    "without any verification",
    "without verification",
    "claimed success without",
    "should have created a bead",
    "should have created an issue",
    "wrong transport",
    "wrong source for this call",
    "raw web instead of using axon",
    "instead of using axon",
    "stopped at the plan",
    "stopped at plan",
];

pub fn detect_scope_or_source_confusion(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, SCOPE_OR_SOURCE_CONFUSION_PHRASES)
}

pub fn detect_ignored_instruction(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, IGNORED_INSTRUCTION_PHRASES)
}

/// Minimum tool-call volume (rows between the skill event and resolution)
/// that counts as "many" for the overlong-loop signal.
const OVERLONG_LOOP_TOOL_CALL_THRESHOLD: usize = 15;

/// `overlong_loop_after_skill` requires BOTH a high tool-call volume after
/// the skill event AND a co-occurring negative signal (user correction or
/// frustration). Long-but-successful work alone must never trigger this —
/// callers pass `has_correction_or_frustration_signal` computed from the
/// other four detectors / the abuse-term matcher over the same window.
pub fn detect_overlong_loop(
    _skill_event_count: usize,
    tool_call_count: usize,
    has_correction_or_frustration_signal: bool,
) -> bool {
    tool_call_count >= OVERLONG_LOOP_TOOL_CALL_THRESHOLD && has_correction_or_frustration_signal
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib skill_signal_detectors -- --nocapture
```

Expected: `test result: ok. 10 passed; 0 failed` (4 from Task 1 + 6 new).

- [ ] **Step 5: Commit**

```bash
git add src/app/skill_signal_detectors.rs
git commit -m "feat: add scope-confusion, ignored-instruction, overlong-loop skill signal detectors"
```

---

### Task 3: Grouping, scoring, and stable incident id — `search_ai_skill_incidents` DB query

**Files:**
- Modify: `src/db/models.rs` (append the `AiSkillIncidentParams`, `SkillIncident`, `SkillSignalCounts`, `AiSkillIncidentResult` structs from "Locked interfaces" above, placed after the existing `AiIncidentResult` struct around line 521 — grep `pub struct AiIncidentResult` to find the exact line)
- Modify: `src/db/queries.rs` (add `search_ai_skill_incidents()` function; place it directly after `search_ai_incidents()` / `ai_incident_anchor_sql()`, i.e. after line ~2360 — grep `fn investigate_ai_incidents` to find the boundary and insert before it)
- Test: `src/db/queries_tests.rs` (this repo's sidecar convention: `src/db/queries.rs` has `#[cfg(test)] #[path = "queries_tests.rs"] mod tests;` at its end — add new `#[test]` functions there)

**Interfaces:**
- Consumes: `AiSkillEventEntry`/skill-events table from the prior phase (verify exact struct/column names per the Task-1-of-this-doc callout at top). Also consumes `crate::app::skill_signal_detectors::{detect_user_correction, detect_tool_failure, detect_scope_or_source_confusion, detect_ignored_instruction, detect_overlong_loop, ALL_SIGNALS}` from Tasks 1-2. Note: `src/db/queries.rs` is below `src/app/` in the dependency graph in this repo's module layout (db has no dependency on app) — confirm with `grep -n "^use crate::app" src/db/queries.rs`. If db cannot depend on app (likely, since app depends on db not vice versa), the detector functions must instead live in `src/db/` (e.g. a new `src/db/skill_signal_detectors.rs` reachable from `src/db/queries.rs`, with `src/app/skill_signal_detectors.rs` becoming a thin `pub use crate::db::skill_signal_detectors::*;` re-export so Task 6's findings module can still `use super::skill_signal_detectors` at the app layer). **Resolve this before Step 3 by running the grep above; if db cannot see app, move the Task 1/2 detector module to `src/db/skill_signal_detectors.rs` and adjust Tasks 1, 2, and 6 file paths accordingly.** For the remainder of this task, code below assumes detectors are reachable from `src/db/queries.rs` as `crate::app::skill_signal_detectors::*` — swap the `use` path if the module was relocated.
- Produces: `pub fn search_ai_skill_incidents(pool: &DbPool, params: &AiSkillIncidentParams) -> Result<AiSkillIncidentResult>` — consumed by Task 4's investigation bundle builder and the service/MCP/CLI/REST layers in Task 5.

- [ ] **Step 1: Write the failing test** (full real code)

Append to `src/db/queries_tests.rs` (near the existing `search_ai_incidents_anchor_plan_avoids_temp_order_sort` test, after `make_ai_entry`):

```rust
fn insert_skill_event(
    pool: &DbPool,
    log_id: i64,
    ai_tool: &str,
    ai_project: &str,
    ai_session_id: &str,
    hostname: &str,
    timestamp: &str,
    skill_name: &str,
    skill_plugin: Option<&str>,
) {
    // Note (PR 2 eng review sync): `skill_path`/`metadata_json` were dropped
    // from `ai_skill_events` before PR 2 shipped — neither extractor ever set
    // them. This INSERT reflects the shipped column set; re-verify against
    // the real schema before implementing if this note has gone stale.
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO ai_skill_events
            (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
             skill_name, skill_plugin, event_kind, evidence_kind, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'skill_invoked', 'transcript', ?6)",
        rusqlite::params![
            log_id,
            ai_tool,
            ai_project,
            ai_session_id,
            hostname,
            timestamp,
            skill_name,
            skill_plugin,
        ],
    )
    .unwrap();
}

#[test]
fn search_ai_skill_incidents_groups_by_skill_session_window_and_scores() {
    let (pool, _dir) = test_pool();

    // Skill event log row.
    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "loaded skill lavra:lavra-plan",
    );
    // Correction anchor shortly after, same session.
    let correction_log = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "That's not what I asked for, please redo it.",
    );
    insert_logs_batch(&pool, &[skill_log, correction_log]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(log_ids.len(), 2);

    insert_skill_event(
        &pool,
        log_ids[0],
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    let result = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.incidents.len(), 1, "expected one grouped incident");
    let incident = &result.incidents[0];
    assert_eq!(incident.skill_name, "lavra:lavra-plan");
    assert_eq!(incident.skill_plugin.as_deref(), Some("lavra"));
    assert_eq!(incident.tool, "codex");
    assert_eq!(incident.project, "/home/jmagar/workspace/cortex");
    assert_eq!(incident.session_id, "sess-skill-1");
    assert_eq!(incident.hostname, "devhost");
    assert_eq!(incident.skill_event_count, 1);
    assert_eq!(incident.signal_counts.user_correction_after_skill, 1);
    assert!(
        incident
            .signals_present
            .contains(&"user_correction_after_skill".to_string())
    );
    // score = skill_event_count*2 + user_correction_count*15 + signal_variety*5
    //       = 1*2 + 1*15 + 1*5 = 22 -> "medium" (>=15, <35)
    assert!((incident.priority_score - 22.0).abs() < f64::EPSILON);
    assert_eq!(incident.priority_label, "medium");
    assert!(!incident.incident_id.is_empty());
    assert!(incident.incident_id.starts_with("skill-inc-"));
}

#[test]
fn search_ai_skill_incidents_sorts_by_score_with_total_cmp() {
    let (pool, _dir) = test_pool();

    // Two independent sessions -> two incidents with different scores.
    // Session A: skill event only, no negative signal (low score).
    let a_skill = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-a",
        "sess-a",
        "loaded skill lavra:lavra-plan",
    );
    // Session B: skill event + correction + tool failure (higher score).
    let b_skill = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "loaded skill lavra:lavra-plan",
    );
    let b_correction = make_ai_entry(
        "2026-01-01T00:01:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "you said you would run the tests but you didn't",
    );
    let b_failure = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "command exited with exit code 1",
    );
    insert_logs_batch(&pool, &[a_skill, b_skill, b_correction, b_failure]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    insert_skill_event(
        &pool, log_ids[0], "codex", "/tmp/project-a", "sess-a", "devhost",
        "2026-01-01T00:00:00Z", "lavra:lavra-plan", Some("lavra"),
    );
    insert_skill_event(
        &pool, log_ids[1], "codex", "/tmp/project-b", "sess-b", "devhost",
        "2026-01-01T00:00:00Z", "lavra:lavra-plan", Some("lavra"),
    );

    let result = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(10),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.incidents.len(), 2);
    // Highest score first (session B).
    assert_eq!(result.incidents[0].session_id, "sess-b");
    assert_eq!(result.incidents[1].session_id, "sess-a");
    assert!(result.incidents[0].priority_score > result.incidents[1].priority_score);
    // Regression guard: scores must be a total order even in pathological
    // cases (NaN would break partial_cmp/unwrap_or(Equal) but not total_cmp).
    let mut scores = vec![f64::NAN, 3.0, 1.0, f64::NAN, 2.0];
    scores.sort_by(|a, b| b.total_cmp(a));
    assert_eq!(scores.len(), 5, "total_cmp sort must not panic or drop elements on NaN");
}

#[test]
fn search_ai_skill_incidents_min_score_and_signals_filters() {
    let (pool, _dir) = test_pool();
    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "claude",
        "/tmp/project-c",
        "sess-c",
        "loaded skill lavra:lavra-plan",
    );
    insert_logs_batch(&pool, &[skill_log]).unwrap();
    let log_id: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT id FROM logs LIMIT 1", [], |row| row.get(0))
            .unwrap()
    };
    insert_skill_event(
        &pool, log_id, "claude", "/tmp/project-c", "sess-c", "devhost",
        "2026-01-01T00:00:00Z", "lavra:lavra-plan", Some("lavra"),
    );

    // min_score above what a bare skill-event-only incident can reach (score=2) excludes it.
    let filtered = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            min_score: Some(10.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered.incidents.is_empty());

    // signals filter for a category with zero hits also excludes it.
    let filtered_by_signal = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            signals: vec!["tool_failure_after_skill".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered_by_signal.incidents.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib search_ai_skill_incidents -- --nocapture
```

Expected: compile error — `search_ai_skill_incidents`, `AiSkillIncidentParams`, `AiSkillIncidentResult` do not exist yet, and the `ai_skill_events` table may not exist if the skill-events phase has not landed (in which case `insert_skill_event` will fail at runtime with "no such table: ai_skill_events" once compilation is fixed — confirm the table exists first via `grep -n "ai_skill_events" src/db/pool.rs`; if absent, stop and complete the skill-events phase first, this phase is not implementable without it).

- [ ] **Step 3: Write minimal implementation**

Append to `src/db/models.rs` after the existing `AiIncidentResult` struct (around line 521, before the `// AI investigate` section comment):

```rust
// ---------------------------------------------------------------------------
// Skill incident grouping (kmib-skill.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillIncidentParams {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Max incidents to return. Default 20, clamp 1..=100.
    pub limit: Option<u32>,
    /// Grouping window in minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    /// Restrict to incidents containing at least one of these signal
    /// categories. Empty = no filter (all incidents).
    pub signals: Vec<String>,
    /// Minimum `priority_score` (inclusive). `None` = no filter.
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillSignalCounts {
    pub user_correction_after_skill: usize,
    pub tool_failure_after_skill: usize,
    pub scope_or_source_confusion: usize,
    pub ignored_skill_or_policy_instruction: usize,
    pub overlong_loop_after_skill: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncident {
    /// Stable synthetic ID: hash of skill name/plugin + tool/project/session/
    /// hostname + sorted anchor log ids + sorted skill event ids.
    pub incident_id: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub skill_event_count: usize,
    /// Sorted `ai_skill_events.id` values in this group.
    pub skill_event_ids: Vec<i64>,
    /// Sorted `logs.id` values backing the anchor signals in this group.
    pub anchor_log_ids: Vec<i64>,
    pub signal_counts: SkillSignalCounts,
    /// Sorted distinct signal category names present in this incident.
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    /// "low" | "medium" | "high" | "critical"
    pub priority_label: String,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillIncidentResult {
    pub incidents: Vec<SkillIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}
```

Add to `src/db/queries.rs`, directly before `pub fn investigate_ai_incidents(` (grep to confirm exact line, currently ~2362):

```rust
const SKILL_INCIDENT_CANDIDATE_CAP: usize = 10_000;

/// Grouping key for skill incidents: `(skill_name, skill_plugin, tool,
/// project, session_id, hostname, window_bucket)`.
/// `window_bucket = unix_secs / window_secs * window_secs` (floor to window
/// boundary), mirroring `search_ai_incidents`'s abuse-incident grouping.
pub fn search_ai_skill_incidents(
    pool: &DbPool,
    params: &AiSkillIncidentParams,
) -> Result<AiSkillIncidentResult> {
    use std::collections::HashMap;

    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let window_secs = i64::from(params.window_minutes.unwrap_or(10).clamp(1, 120)) * 60;

    // ── Fetch candidate skill events (bounded, same capped-window pattern as
    // search_ai_incidents' FTS candidate fetch) ─────────────────────────────
    struct SkillEventRow {
        id: i64,
        log_id: i64,
        timestamp: String,
        hostname: String,
        tool: String,
        project: String,
        session_id: String,
        skill_name: String,
        skill_plugin: Option<String>,
    }

    let mut sql = String::from(
        "SELECT id, log_id, timestamp, hostname, ai_tool, ai_project, ai_session_id,
                skill_name, skill_plugin
         FROM ai_skill_events
         WHERE 1 = 1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;
    if let Some(skill) = &params.skill {
        sql.push_str(&format!(" AND skill_name = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(skill.clone()));
        idx += 1;
    }
    if let Some(plugin) = &params.plugin {
        sql.push_str(&format!(" AND skill_plugin = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(plugin.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
        idx += 1;
    }
    if let Some(project) = &params.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(project.clone()));
        idx += 1;
    }
    if let Some(session_id) = &params.ai_session_id {
        sql.push_str(&format!(" AND ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(session_id.clone()));
        idx += 1;
    }
    if let Some(hostname) = &params.hostname {
        sql.push_str(&format!(" AND hostname = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(hostname.clone()));
        idx += 1;
    }
    if let Some(from) = &params.since {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(from.clone()));
        idx += 1;
    }
    if let Some(to) = &params.until {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(to.clone()));
    }
    let _ = idx;
    sql.push_str(&format!(
        " ORDER BY timestamp ASC LIMIT {}",
        SKILL_INCIDENT_CANDIDATE_CAP + 1
    ));

    let mut stmt = conn.prepare(&sql)?;
    let candidate_events: Vec<SkillEventRow> = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(SkillEventRow {
                id: row.get(0)?,
                log_id: row.get(1)?,
                timestamp: row.get(2)?,
                hostname: row.get(3)?,
                tool: row.get(4)?,
                project: row.get(5)?,
                session_id: row.get(6)?,
                skill_name: row.get(7)?,
                skill_plugin: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let candidate_window_truncated = candidate_events.len() > SKILL_INCIDENT_CANDIDATE_CAP;
    let raw_candidate_count = candidate_events.len();

    // ── Group by (skill_name, skill_plugin, tool, project, session_id,
    // hostname, window_bucket) ───────────────────────────────────────────────
    type GroupKey = (String, Option<String>, String, String, String, String, i64);
    let mut groups: HashMap<GroupKey, Vec<&SkillEventRow>> = HashMap::new();

    for row in candidate_events.iter().take(SKILL_INCIDENT_CANDIDATE_CAP) {
        let bucket = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
            .map(|dt| (dt.timestamp() / window_secs) * window_secs)
            .unwrap_or(0);
        let key = (
            row.skill_name.clone(),
            row.skill_plugin.clone(),
            row.tool.clone(),
            row.project.clone(),
            row.session_id.clone(),
            row.hostname.clone(),
            bucket,
        );
        groups.entry(key).or_default().push(row);
    }

    // ── For each group, fetch nearby transcript logs in the session/window to
    // detect anchor signals, then score ───────────────────────────────────────
    let mut incidents: Vec<SkillIncident> = Vec::with_capacity(groups.len());
    for ((skill_name, skill_plugin, tool, project, session_id, hostname, _bucket), events) in
        groups
    {
        let first_seen = events.first().map(|e| e.timestamp.clone()).unwrap_or_default();
        let last_seen = events.last().map(|e| e.timestamp.clone()).unwrap_or_default();
        let duration_secs = {
            let t0 = chrono::DateTime::parse_from_rfc3339(&first_seen)
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            let t1 = chrono::DateTime::parse_from_rfc3339(&last_seen)
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            (t1 - t0).max(0)
        };

        // Window bounds for anchor detection: from first skill event to
        // window_secs after the last one (anchors that follow the skill).
        let win_from = first_seen.clone();
        let win_to = chrono::DateTime::parse_from_rfc3339(&last_seen)
            .map(|dt| {
                (dt.with_timezone(&chrono::Utc) + chrono::Duration::seconds(window_secs))
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string()
            })
            .unwrap_or_else(|_| last_seen.clone());

        let mut anchor_stmt = conn.prepare(
            "SELECT id, message FROM logs
             WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
               AND timestamp >= ?4 AND timestamp <= ?5
             ORDER BY timestamp ASC
             LIMIT 500",
        )?;
        let anchor_rows: Vec<(i64, String)> = anchor_stmt
            .query_map(
                rusqlite::params![session_id, project, tool, win_from, win_to],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut counts = SkillSignalCounts::default();
        let mut anchor_log_ids: Vec<i64> = Vec::new();
        let tool_call_rows = anchor_rows.len();
        let mut has_correction_or_frustration = false;

        for (id, message) in &anchor_rows {
            let mut hit = false;
            if crate::app::skill_signal_detectors::detect_user_correction(message) {
                counts.user_correction_after_skill += 1;
                has_correction_or_frustration = true;
                hit = true;
            }
            if crate::app::skill_signal_detectors::detect_tool_failure(message) {
                counts.tool_failure_after_skill += 1;
                hit = true;
            }
            if crate::app::skill_signal_detectors::detect_scope_or_source_confusion(message) {
                counts.scope_or_source_confusion += 1;
                hit = true;
            }
            if crate::app::skill_signal_detectors::detect_ignored_instruction(message) {
                counts.ignored_skill_or_policy_instruction += 1;
                hit = true;
            }
            if hit {
                anchor_log_ids.push(*id);
            }
        }
        if crate::app::skill_signal_detectors::detect_overlong_loop(
            events.len(),
            tool_call_rows,
            has_correction_or_frustration,
        ) {
            counts.overlong_loop_after_skill += 1;
        }

        anchor_log_ids.sort_unstable();
        anchor_log_ids.dedup();

        let mut signals_present: Vec<String> = Vec::new();
        if counts.user_correction_after_skill > 0 {
            signals_present.push("user_correction_after_skill".to_string());
        }
        if counts.tool_failure_after_skill > 0 {
            signals_present.push("tool_failure_after_skill".to_string());
        }
        if counts.scope_or_source_confusion > 0 {
            signals_present.push("scope_or_source_confusion".to_string());
        }
        if counts.ignored_skill_or_policy_instruction > 0 {
            signals_present.push("ignored_skill_or_policy_instruction".to_string());
        }
        if counts.overlong_loop_after_skill > 0 {
            signals_present.push("overlong_loop_after_skill".to_string());
        }
        signals_present.sort();

        // ── Locked scoring formula ──────────────────────────────────────────
        let signal_variety = signals_present.len() as f64;
        let priority_score = events.len() as f64 * 2.0
            + counts.user_correction_after_skill as f64 * 15.0
            + counts.tool_failure_after_skill as f64 * 8.0
            + counts.scope_or_source_confusion as f64 * 12.0
            + counts.ignored_skill_or_policy_instruction as f64 * 12.0
            + counts.overlong_loop_after_skill as f64 * 10.0
            + signal_variety * 5.0;

        let priority_label = if priority_score < 15.0 {
            "low"
        } else if priority_score < 35.0 {
            "medium"
        } else if priority_score < 60.0 {
            "high"
        } else {
            "critical"
        }
        .to_string();

        let mut skill_event_ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        skill_event_ids.sort_unstable();

        // ── Stable incident ID: same DefaultHasher pattern as
        // search_ai_incidents (see queries.rs ~line 2255), extended with
        // skill name/plugin and sorted skill event ids.
        let incident_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            skill_name.hash(&mut h);
            skill_plugin.hash(&mut h);
            tool.hash(&mut h);
            project.hash(&mut h);
            session_id.hash(&mut h);
            hostname.hash(&mut h);
            for id in &anchor_log_ids {
                id.hash(&mut h);
            }
            for id in &skill_event_ids {
                id.hash(&mut h);
            }
            format!("skill-inc-{:016x}", h.finish())
        };

        incidents.push(SkillIncident {
            incident_id,
            skill_name,
            skill_plugin,
            tool,
            project,
            session_id,
            hostname,
            first_seen,
            last_seen,
            duration_secs,
            skill_event_count: events.len(),
            skill_event_ids,
            anchor_log_ids,
            signal_counts: counts,
            signals_present,
            priority_score,
            priority_label,
            window_minutes: (window_secs / 60) as u32,
        });
    }

    // ── Post-grouping filters: signals, min_score ───────────────────────────
    if !params.signals.is_empty() {
        incidents.retain(|inc| {
            inc.signals_present
                .iter()
                .any(|s| params.signals.contains(s))
        });
    }
    if let Some(min_score) = params.min_score {
        incidents.retain(|inc| inc.priority_score >= min_score);
    }

    // Sort by priority_score descending, then last_seen descending. Uses
    // total_cmp (not partial_cmp/unwrap_or(Equal)) — see search_ai_incidents
    // for why: total_cmp is a total order even if a NaN score ever appears.
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
    });

    let total_incidents = incidents.len();
    let truncated = total_incidents > limit || candidate_window_truncated;
    incidents.truncate(limit);

    Ok(AiSkillIncidentResult {
        incidents,
        total_incidents,
        candidate_event_rows: raw_candidate_count.min(SKILL_INCIDENT_CANDIDATE_CAP),
        candidate_cap: SKILL_INCIDENT_CANDIDATE_CAP,
        candidate_window_truncated,
        truncated,
    })
}
```

Note: if `src/db/queries.rs` genuinely cannot `use crate::app::...` (check for a `#![forbid]`/module-visibility issue via `cargo check` after adding this), move `skill_signal_detectors.rs` to `src/db/skill_signal_detectors.rs`, change all `crate::app::skill_signal_detectors::` references above to `crate::db::skill_signal_detectors::` (or a local `super::skill_signal_detectors::` if declared as a sibling module in `src/db/mod.rs`), and update Task 1/Task 2's `Modify: src/app/mod.rs` step to instead modify `src/db/mod.rs`, keeping a `pub use crate::db::skill_signal_detectors as skill_signal_detectors;` line in `src/app/mod.rs` so Task 6's `use super::skill_signal_detectors` in the app-layer findings module still resolves.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib search_ai_skill_incidents -- --nocapture
```

Expected: `test result: ok. 3 passed; 0 failed` (the three tests added in Step 1).

- [ ] **Step 5: Commit**

```bash
git add src/db/models.rs src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat: add search_ai_skill_incidents grouping/scoring query with stable incident ids"
```

---

### Task 4: Investigation bundle builder — `investigate_ai_skill_incidents` DB query with truncation flags

**Files:**
- Modify: `src/db/models.rs` (append `AiSkillInvestigateParams`, `SkillIncidentEvidence`, `AiSkillInvestigateResult`, `SkillIncidentSummary` structs from "Locked interfaces", after the `AiSkillIncidentResult` struct added in Task 3)
- Modify: `src/db/queries.rs` (add `investigate_ai_skill_incidents()`, placed directly after `search_ai_skill_incidents()` from Task 3)
- Test: `src/db/queries_tests.rs` (append)

**Interfaces:**
- Consumes: `search_ai_skill_incidents` (Task 3), `AiSkillEventEntry`/skill-events query function from the prior phase (verify real name; assumed `crate::db::queries::skill_events_for_incident` does NOT exist yet — this task must fetch the `ai_skill_events` rows for a group directly via `skill_event_ids` using an `IN (...)` query, mirroring how `investigate_ai_incidents` fetches anchors via `anchor_ids`).
- Produces: `pub fn investigate_ai_skill_incidents(pool: &DbPool, params: &AiSkillInvestigateParams) -> Result<AiSkillInvestigateResult>` — consumed by the service/MCP/CLI/REST layers in Task 5 and by Task 7's skill-first CLI resolution logic.

- [ ] **Step 1: Write the failing test** (full real code)

Append to `src/db/queries_tests.rs`:

```rust
#[test]
fn investigate_ai_skill_incidents_bundle_has_bounded_collections_and_truncation_flags() {
    let (pool, _dir) = test_pool();

    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "loaded skill lavra:lavra-plan",
    );
    let before_log = make_ai_entry(
        "2026-01-01T00:00:00.000Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "user asked to plan the feature",
    );
    let correction_log = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "that's not what I asked, wrong file",
    );
    let failure_log = make_ai_entry(
        "2026-01-01T00:03:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "command exited with exit code 1",
    );
    insert_logs_batch(
        &pool,
        &[before_log, skill_log, correction_log, failure_log],
    )
    .unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM logs ORDER BY timestamp ASC, id ASC")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    // log_ids: [before, skill, correction, failure] in timestamp order.
    insert_skill_event(
        &pool, log_ids[1], "codex", "/tmp/project-d", "sess-d", "devhost",
        "2026-01-01T00:00:00Z", "lavra:lavra-plan", Some("lavra"),
    );

    let result = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(3),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.evidence.len(), 1);
    let bundle = &result.evidence[0];
    assert_eq!(bundle.incident.skill_name, "lavra:lavra-plan");
    assert!(!bundle.skill_events.is_empty());
    assert!(!bundle.skill_events_truncated);
    assert!(!bundle.signal_anchors.is_empty());
    assert!(!bundle.signal_anchors_truncated);
    // transcript_before should include the pre-skill "user asked to plan" row.
    assert!(
        bundle
            .transcript_before
            .iter()
            .any(|e| e.message.contains("user asked to plan"))
    );
    assert!(!bundle.transcript_before_truncated);
    assert!(!bundle.transcript_after_truncated);
    // The correction log should land in nearby_user_corrections; the failure
    // log should land in nearby_tool_failures.
    assert!(
        bundle
            .nearby_user_corrections
            .iter()
            .any(|e| e.message.contains("wrong file"))
    );
    assert!(
        bundle
            .nearby_tool_failures
            .iter()
            .any(|e| e.message.contains("exit code 1"))
    );
    assert!(!bundle.nearby_logs_truncated);
    assert!(!bundle.nearby_errors_truncated);
}

#[test]
fn investigate_ai_skill_incidents_exact_incident_id_can_target_outside_top_page() {
    let (pool, _dir) = test_pool();
    let mut entries = Vec::new();
    for i in 0..12 {
        entries.push(make_ai_entry(
            &format!("2026-01-01T00:{i:02}:00Z"),
            "host-a",
            "codex",
            "/tmp/project-e",
            &format!("sess-e-{i:02}"),
            "loaded skill lavra:lavra-plan",
        ));
    }
    insert_logs_batch(&pool, &entries).unwrap();
    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    for (i, log_id) in log_ids.iter().enumerate() {
        insert_skill_event(
            &pool, *log_id, "codex", "/tmp/project-e", &format!("sess-e-{i:02}"), "host-a",
            &format!("2026-01-01T00:{i:02}:00Z"), "lavra:lavra-plan", Some("lavra"),
        );
    }

    let listed = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(listed.incidents.len(), 12);
    let target_id = listed.incidents.last().unwrap().incident_id.clone();

    let top_page = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !top_page
            .evidence
            .iter()
            .any(|b| b.incident.incident_id == target_id)
    );

    let exact = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            incident_id: Some(target_id.clone()),
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(exact.evidence.len(), 1);
    assert_eq!(exact.evidence[0].incident.incident_id, target_id);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib investigate_ai_skill_incidents -- --nocapture
```

Expected: compile error — `investigate_ai_skill_incidents`, `AiSkillInvestigateParams` do not exist yet.

- [ ] **Step 3: Write minimal implementation**

Append to `src/db/models.rs` after `AiSkillIncidentResult`:

```rust
// ---------------------------------------------------------------------------
// Skill investigate — evidence bundle layer (kmib-skill.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillInvestigateParams {
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Max incidents to investigate. Default 3, clamp 1..=10.
    pub limit: Option<u32>,
    /// Incident grouping window minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    /// Correlation window minutes around incident. Default 5, clamp 1..=120.
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentEvidence {
    pub incident: SkillIncident,
    /// The `ai_skill_events` rows in this group, capped at 25.
    pub skill_events: Vec<AiSkillEventEntry>,
    pub skill_events_truncated: bool,
    /// Transcript rows that triggered an anchor signal, capped at 50.
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    /// Same-session transcript entries before the first skill event, capped 20.
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    /// Same-session transcript entries after the last skill event, capped 20.
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    /// Subset of nearby_logs matching tool-failure phrases, capped 25.
    pub nearby_tool_failures: Vec<LogEntry>,
    pub nearby_tool_failures_truncated: bool,
    /// Subset of nearby_logs matching user-correction phrases, capped 25.
    pub nearby_user_corrections: Vec<LogEntry>,
    pub nearby_user_corrections_truncated: bool,
    /// Non-AI syslog/Docker logs in the correlation window, capped 50.
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    /// Subset of nearby_logs with severity warning or above, capped 25.
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillInvestigateResult {
    pub evidence: Vec<SkillIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
}
```

**Important:** `AiSkillEventEntry` here must be the exact type from the prior skill-events phase (per the Task-1-of-this-doc callout) — if that struct lives in `src/db/models.rs` already, do not redefine it; just reference it directly. If its real name differs (e.g. `SkillEventEntry`), replace every occurrence of `AiSkillEventEntry` in this task and in the "Locked interfaces" section at the top of this file with the real name.

Add to `src/db/queries.rs`, directly after `search_ai_skill_incidents`:

```rust
pub fn investigate_ai_skill_incidents(
    pool: &DbPool,
    params: &AiSkillInvestigateParams,
) -> Result<AiSkillInvestigateResult> {
    const SKILL_EVENTS_CAP: usize = 25;
    const SIGNAL_ANCHORS_CAP: usize = 50;
    const TRANSCRIPT_CAP: usize = 20;
    const NEARBY_CAP: usize = 50;
    const NEARBY_SUBSET_CAP: usize = 25;

    let limit = params.limit.unwrap_or(3).clamp(1, 10) as usize;
    let incident_lookup_limit = if params.incident_id.is_some() {
        100
    } else {
        limit as u32
    };
    let corr_mins = i64::from(params.correlation_window_minutes.unwrap_or(5).clamp(1, 120));

    let incident_result = search_ai_skill_incidents(
        pool,
        &AiSkillIncidentParams {
            skill: params.skill.clone(),
            plugin: params.plugin.clone(),
            ai_tool: params.ai_tool.clone(),
            ai_project: params.ai_project.clone(),
            ai_session_id: None,
            hostname: None,
            since: params.since.clone(),
            until: params.until.clone(),
            limit: Some(incident_lookup_limit),
            window_minutes: params.window_minutes,
            signals: Vec::new(),
            min_score: None,
        },
    )?;
    let total_incidents = incident_result.total_incidents;
    let truncated = incident_result.truncated;
    let mut incidents = if let Some(incident_id) = &params.incident_id {
        incident_result
            .incidents
            .into_iter()
            .filter(|inc| inc.incident_id == *incident_id)
            .collect::<Vec<_>>()
    } else {
        incident_result.incidents
    };
    incidents.truncate(limit);

    let conn = pool.get()?;
    let mut evidence = Vec::with_capacity(incidents.len());

    for incident in incidents {
        // ── Skill events for this group ─────────────────────────────────────
        let (skill_events, skill_events_truncated) = if incident.skill_event_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.skill_event_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            // Note (PR 2 eng review sync): no skill_path/metadata_json columns
            // — dropped from ai_skill_events before PR 2 shipped. Re-verify
            // against the real schema before implementing if stale.
            let sql = format!(
                "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                        skill_name, skill_plugin, event_kind, evidence_kind
                 FROM ai_skill_events WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows: Vec<AiSkillEventEntry> = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .skill_event_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_skill_event_row, // NOTE: use the actual row-mapper fn from the
                                          // skill-events phase (e.g. `map_ai_skill_event_row`);
                                          // grep queries.rs for the mapper used by that
                                          // phase's own query function and reuse it here
                                          // instead of redefining one.
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > SKILL_EVENTS_CAP;
            let mut out = rows;
            out.truncate(SKILL_EVENTS_CAP);
            (out, truncated)
        };

        // ── Signal anchor log rows ──────────────────────────────────────────
        let (signal_anchors, signal_anchors_truncated) = if incident.anchor_log_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.anchor_log_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            let sql = format!(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .anchor_log_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > SIGNAL_ANCHORS_CAP;
            let mut out = rows;
            out.truncate(SIGNAL_ANCHORS_CAP);
            (out, truncated)
        };

        // ── Transcript before/after (same pattern as investigate_ai_incidents) ──
        let (transcript_before, transcript_before_truncated) = {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
                   AND timestamp < ?4
                 ORDER BY timestamp DESC
                 LIMIT 21",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![
                        &incident.session_id,
                        &incident.project,
                        &incident.tool,
                        &incident.first_seen,
                    ],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > TRANSCRIPT_CAP;
            let mut out = rows;
            out.truncate(TRANSCRIPT_CAP);
            out.reverse();
            (out, truncated)
        };

        let (transcript_after, transcript_after_truncated) = {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
                   AND timestamp > ?4
                 ORDER BY timestamp ASC
                 LIMIT 21",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![
                        &incident.session_id,
                        &incident.project,
                        &incident.tool,
                        &incident.last_seen,
                    ],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > TRANSCRIPT_CAP;
            let mut out = rows;
            out.truncate(TRANSCRIPT_CAP);
            (out, truncated)
        };

        // ── Nearby non-AI logs in the correlation window ────────────────────
        let (nearby_logs, nearby_logs_truncated) = {
            let win_from = chrono::DateTime::parse_from_rfc3339(&incident.first_seen)
                .map(|dt| {
                    (dt.with_timezone(&chrono::Utc) - chrono::Duration::minutes(corr_mins))
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                })
                .unwrap_or_else(|_| incident.first_seen.clone());
            let win_to = chrono::DateTime::parse_from_rfc3339(&incident.last_seen)
                .map(|dt| {
                    (dt.with_timezone(&chrono::Utc) + chrono::Duration::minutes(corr_mins))
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                })
                .unwrap_or_else(|_| incident.last_seen.clone());

            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC
                 LIMIT 51",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![win_from, win_to], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > NEARBY_CAP;
            let mut out = rows;
            out.truncate(NEARBY_CAP);
            (out, truncated)
        };

        // ── Derived subsets: tool failures, user corrections, errors ────────
        let mut nearby_tool_failures: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| crate::app::skill_signal_detectors::detect_tool_failure(&e.message))
            .cloned()
            .collect();
        let nearby_tool_failures_truncated = nearby_tool_failures.len() > NEARBY_SUBSET_CAP;
        nearby_tool_failures.truncate(NEARBY_SUBSET_CAP);

        let mut nearby_user_corrections: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| crate::app::skill_signal_detectors::detect_user_correction(&e.message))
            .cloned()
            .collect();
        let nearby_user_corrections_truncated = nearby_user_corrections.len() > NEARBY_SUBSET_CAP;
        nearby_user_corrections.truncate(NEARBY_SUBSET_CAP);

        let error_sevs = ["emergency", "alert", "critical", "error", "warning"];
        let mut nearby_errors: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| error_sevs.contains(&e.severity.as_str()))
            .cloned()
            .collect();
        let nearby_errors_truncated = nearby_errors.len() > NEARBY_SUBSET_CAP;
        nearby_errors.truncate(NEARBY_SUBSET_CAP);

        evidence.push(SkillIncidentEvidence {
            incident,
            skill_events,
            skill_events_truncated,
            signal_anchors,
            signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated,
            transcript_after,
            transcript_after_truncated,
            nearby_tool_failures,
            nearby_tool_failures_truncated,
            nearby_user_corrections,
            nearby_user_corrections_truncated,
            nearby_logs,
            nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated,
        });
    }

    Ok(AiSkillInvestigateResult {
        evidence,
        total_incidents,
        truncated,
    })
}
```

**Callout:** `map_skill_event_row` is a placeholder name — the prior skill-events phase's query function already has a row-mapper closure/function for `ai_skill_events` rows (13 columns). Grep `src/db/queries.rs` for the existing skill-events query function (likely named `search_ai_skill_events` or similar per the callout at the top of this document) and reuse its exact row-mapping logic/closure instead of writing a new one, to avoid column-order drift between the two queries.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib investigate_ai_skill_incidents -- --nocapture
```

Expected: `test result: ok. 2 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add src/db/models.rs src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat: add investigate_ai_skill_incidents evidence bundle builder with truncation flags"
```

---

### Task 5: App-layer models, service methods, MCP actions, and REST routes for `skill_incidents`

**Files:**
- Create: `src/app/models/ai_skill_incidents.rs`
- Modify: `src/app/models.rs` (or `src/app/models/mod.rs` — grep `mod ai_incidents;` to find the exact declaration site and add `pub mod ai_skill_incidents;` beside it, plus re-export via `pub use ai_skill_incidents::*;` matching the existing `pub use ai_incidents::*;` pattern)
- Modify: `src/app/services/ai.rs` (add `list_ai_skill_incidents` and `investigate_ai_skill_incidents` service methods after the existing `investigate_ai_incidents` method, ~line 259)
- Modify: `src/mcp/actions.rs` (add `skill_incidents` and `skill_investigate` action specs + `ActionHandler` variants)
- Modify: `src/mcp/tools.rs` (add dispatch arms + handler functions)
- Modify: `src/api.rs` (add `GET /api/sessions/skill-incidents` and `GET /api/sessions/skill-investigate` routes + query structs + handlers)
- Test: `src/app/service_tests.rs` (append; this file already tests `investigate_ai_incidents` around line 117, follow that pattern)

**Interfaces:**
- Consumes: `db::search_ai_skill_incidents`, `db::investigate_ai_skill_incidents` (Task 3, 4), `db::AiSkillIncidentParams`, `db::AiSkillInvestigateParams`.
- Produces: `AiSkillIncidentRequest`, `AiSkillIncidentResponse`, `AiSkillInvestigateRequest`, `AiSkillInvestigateResponse` (app/wire layer, mirroring `AiIncidentRequest`/`AiIncidentResponse`/`AiInvestigateRequest`/`AiInvestigateResponse` in `src/app/models/ai_incidents.rs`), plus `AppService::list_ai_skill_incidents`, `AppService::investigate_ai_skill_incidents_bundle` (name it `investigate_ai_skill_incidents` unless that collides with the db-layer import — check via `cargo check`; if it collides, name the service method `investigate_skill_incidents` and note this rename in Task 7/8). These are consumed by Task 7 (CLI) and Task 8 (docs).

- [ ] **Step 1: Write the failing test** (full real code)

Append to `src/app/service_tests.rs` (mirror the existing `investigate_ai_incidents` test at line 117 — read it first via `sed -n '90,140p' src/app/service_tests.rs` to copy its exact `AppService`-construction boilerplate, then adapt):

```rust
#[tokio::test]
async fn list_ai_skill_incidents_empty_db_returns_no_data() {
    let service = test_app_service().await; // reuse this file's existing test-service constructor
    let response = service
        .list_ai_skill_incidents(AiSkillIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(response.incidents.len(), 0);
    assert_eq!(response.total_incidents, 0);
}

#[tokio::test]
async fn investigate_ai_skill_incidents_empty_db_returns_no_data_flag() {
    let service = test_app_service().await;
    let response = service
        .investigate_ai_skill_incidents(AiSkillInvestigateRequest::default())
        .await
        .unwrap();
    assert_eq!(response.evidence.len(), 0);
    assert!(response.no_data);
    assert!(!response.suggested_filters.is_empty());
}
```

(If this file's existing tests use a different constructor name than `test_app_service()`, grep `src/app/service_tests.rs` for the helper used by the `investigate_ai_incidents` test at line 117 and use that exact name instead.)

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib list_ai_skill_incidents_empty_db_returns_no_data investigate_ai_skill_incidents_empty_db_returns_no_data_flag -- --nocapture
```

Expected: compile error — `AiSkillIncidentRequest`, `AiSkillInvestigateRequest`, and the two service methods do not exist.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/models/ai_skill_incidents.rs` (mirrors `src/app/models/ai_incidents.rs` structure exactly):

```rust
use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSkillIncidentRequest {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    #[serde(default)]
    pub signals: Vec<String>,
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillSignalCounts {
    pub user_correction_after_skill: usize,
    pub tool_failure_after_skill: usize,
    pub scope_or_source_confusion: usize,
    pub ignored_skill_or_policy_instruction: usize,
    pub overlong_loop_after_skill: usize,
}

impl From<db::SkillSignalCounts> for SkillSignalCounts {
    fn from(v: db::SkillSignalCounts) -> Self {
        Self {
            user_correction_after_skill: v.user_correction_after_skill,
            tool_failure_after_skill: v.tool_failure_after_skill,
            scope_or_source_confusion: v.scope_or_source_confusion,
            ignored_skill_or_policy_instruction: v.ignored_skill_or_policy_instruction,
            overlong_loop_after_skill: v.overlong_loop_after_skill,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncident {
    pub incident_id: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub skill_event_count: usize,
    pub skill_event_ids: Vec<i64>,
    pub anchor_log_ids: Vec<i64>,
    pub signal_counts: SkillSignalCounts,
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    pub priority_label: String,
    pub window_minutes: u32,
}

impl From<db::SkillIncident> for SkillIncident {
    fn from(v: db::SkillIncident) -> Self {
        Self {
            incident_id: v.incident_id,
            skill_name: v.skill_name,
            skill_plugin: v.skill_plugin,
            tool: v.tool,
            project: v.project,
            session_id: v.session_id,
            hostname: v.hostname,
            first_seen: v.first_seen,
            last_seen: v.last_seen,
            duration_secs: v.duration_secs,
            skill_event_count: v.skill_event_count,
            skill_event_ids: v.skill_event_ids,
            anchor_log_ids: v.anchor_log_ids,
            signal_counts: v.signal_counts.into(),
            signals_present: v.signals_present,
            priority_score: v.priority_score,
            priority_label: v.priority_label,
            window_minutes: v.window_minutes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillIncidentResponse {
    pub incidents: Vec<SkillIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSkillInvestigateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentEvidence {
    pub incident: SkillIncident,
    pub skill_events: Vec<db::AiSkillEventEntry>, // rename to match real Task-1 type if different
    pub skill_events_truncated: bool,
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    pub nearby_tool_failures: Vec<LogEntry>,
    pub nearby_tool_failures_truncated: bool,
    pub nearby_user_corrections: Vec<LogEntry>,
    pub nearby_user_corrections_truncated: bool,
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
    /// Deterministic, rule-based findings (Task 6). Never an LLM summary.
    pub findings: skill_incident_findings::SkillIncidentFindings,
}

impl From<db::SkillIncidentEvidence> for SkillIncidentEvidence {
    fn from(v: db::SkillIncidentEvidence) -> Self {
        let incident: SkillIncident = v.incident.into();
        let signal_anchors: Vec<LogEntry> = v.signal_anchors.into_iter().map(Into::into).collect();
        let transcript_before: Vec<LogEntry> =
            v.transcript_before.into_iter().map(Into::into).collect();
        let transcript_after: Vec<LogEntry> =
            v.transcript_after.into_iter().map(Into::into).collect();
        let nearby_tool_failures: Vec<LogEntry> =
            v.nearby_tool_failures.into_iter().map(Into::into).collect();
        let nearby_user_corrections: Vec<LogEntry> = v
            .nearby_user_corrections
            .into_iter()
            .map(Into::into)
            .collect();
        let nearby_logs: Vec<LogEntry> = v.nearby_logs.into_iter().map(Into::into).collect();
        let nearby_errors: Vec<LogEntry> = v.nearby_errors.into_iter().map(Into::into).collect();

        let findings = skill_incident_findings::derive_skill_incident_findings(
            &incident,
            &v.skill_events,
            &signal_anchors,
            &transcript_before,
            &transcript_after,
            &nearby_logs,
            &nearby_errors,
        );

        Self {
            incident,
            skill_events: v.skill_events,
            skill_events_truncated: v.skill_events_truncated,
            signal_anchors,
            signal_anchors_truncated: v.signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated: v.transcript_before_truncated,
            transcript_after,
            transcript_after_truncated: v.transcript_after_truncated,
            nearby_tool_failures,
            nearby_tool_failures_truncated: v.nearby_tool_failures_truncated,
            nearby_user_corrections,
            nearby_user_corrections_truncated: v.nearby_user_corrections_truncated,
            nearby_logs,
            nearby_logs_truncated: v.nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated: v.nearby_errors_truncated,
            findings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentSummary {
    pub incident_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillInvestigateResponse {
    pub evidence: Vec<SkillIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
    #[serde(default)]
    pub other_matching_incidents: Vec<SkillIncidentSummary>,
    #[serde(default)]
    pub no_incident_low_severity_summary: bool,
    #[serde(default)]
    pub no_data: bool,
    #[serde(default)]
    pub suggested_filters: Vec<String>,
}
```

Add `pub mod ai_skill_incidents;` and re-export next to the existing `ai_incidents` declaration in `src/app/models.rs` (grep `mod ai_incidents` for the exact spot) — copy the exact visibility/re-export pattern already used there.

In `src/app/services/ai.rs`, append after `investigate_ai_incidents` (~line 259):

```rust
pub async fn list_ai_skill_incidents(
    &self,
    req: AiSkillIncidentRequest,
) -> ServiceResult<AiSkillIncidentResponse> {
    let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
    let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
    let result = self
        .run_db("list_ai_skill_incidents", move |pool| {
            db::search_ai_skill_incidents(
                pool,
                &db::AiSkillIncidentParams {
                    skill: req.skill,
                    plugin: req.plugin,
                    ai_tool: req.tool,
                    ai_project: req.project,
                    ai_session_id: req.session_id,
                    hostname: req.hostname,
                    since: from,
                    until: to,
                    limit: req.limit,
                    window_minutes: req.window_minutes,
                    signals: req.signals,
                    min_score: req.min_score,
                },
            )
        })
        .await?;
    Ok(AiSkillIncidentResponse {
        incidents: result.incidents.into_iter().map(Into::into).collect(),
        total_incidents: result.total_incidents,
        candidate_event_rows: result.candidate_event_rows,
        candidate_cap: result.candidate_cap,
        candidate_window_truncated: result.candidate_window_truncated,
        truncated: result.truncated,
    })
}

pub async fn investigate_ai_skill_incidents(
    &self,
    req: AiSkillInvestigateRequest,
) -> ServiceResult<AiSkillInvestigateResponse> {
    let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
    let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
    let has_skill_filter = req.skill.is_some() || req.plugin.is_some();
    let result = self
        .run_heavy_db("investigate_ai_skill_incidents", move |pool| {
            db::investigate_ai_skill_incidents(
                pool,
                &db::AiSkillInvestigateParams {
                    incident_id: req.incident_id,
                    skill: req.skill,
                    plugin: req.plugin,
                    ai_tool: req.tool,
                    ai_project: req.project,
                    since: from,
                    until: to,
                    limit: req.limit,
                    window_minutes: req.window_minutes,
                    correlation_window_minutes: req.correlation_window_minutes,
                },
            )
        })
        .await?;

    let no_data = result.evidence.is_empty() && result.total_incidents == 0;
    let suggested_filters = if no_data {
        vec![
            "widen --since (e.g. --since 30d)".to_string(),
            "drop --plugin and filter by --skill only".to_string(),
            "check `cortex sessions skill-incidents` with no filters to see what skills have events".to_string(),
        ]
    } else {
        Vec::new()
    };

    Ok(AiSkillInvestigateResponse {
        evidence: result.evidence.into_iter().map(Into::into).collect(),
        total_incidents: result.total_incidents,
        truncated: result.truncated,
        other_matching_incidents: Vec::new(), // populated by Task 7's skill-first CLI wrapper
        no_incident_low_severity_summary: false, // populated by Task 7
        no_data,
        suggested_filters,
    })
}
```

Note: `has_skill_filter` above is unused by this minimal version (it's plumbed for Task 7 to use when deciding whether to run the skill-first resolution wrapper vs. the plain incident-id path) — either remove it now with `#[allow(unused)]` or leave it for Task 7 to consume; Task 7 replaces this method's body with logic that calls this one internally, so keep the signature stable.

In `src/mcp/actions.rs`:
1. Add two variants to `ActionHandler` enum (after `IncidentContext,`): `SkillIncidents,` and `SkillInvestigate,`.
2. Add two `action_spec!` entries after the `"abuse_investigate"` entry (~line 330):

```rust
    action_spec!(
        "skill_incidents",
        Read,
        "List detected skill-usage incidents (negative signals after a skill loaded)",
        Moderate,
        SkillIncidents
    ),
    action_spec!(
        "skill_investigate",
        Read,
        "Deep-dive investigation of a skill-usage incident, skill-first",
        Expensive,
        SkillInvestigate
    ),
```

In `src/mcp/tools.rs`:
1. Import `AiSkillIncidentRequest, AiSkillInvestigateRequest` alongside the existing `AiIncidentRequest` import (~line 20).
2. Add dispatch arms after the `H::AbuseInvestigate => tool_abuse_investigate(state, args).await,` line (~line 94):

```rust
        H::SkillIncidents => tool_skill_incidents(state, args).await,
        H::SkillInvestigate => tool_skill_investigate(state, args).await,
```

3. Add handler functions after `tool_abuse_investigate` (~line 232):

```rust
async fn tool_skill_incidents(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let req: AiSkillIncidentRequest = action_payload(args, "skill_incidents")?;
    let response = state.service.list_ai_skill_incidents(req).await?;
    tracing::info!(
        incident_count = response.incidents.len(),
        total = response.total_incidents,
        "skill_incidents completed"
    );
    Ok(serde_json::to_value(response)?)
}

async fn tool_skill_investigate(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let req: AiSkillInvestigateRequest = action_payload(args, "skill_investigate")?;
    let response = state.service.investigate_ai_skill_incidents(req).await?;
    tracing::info!(
        total_incidents = response.total_incidents,
        no_data = response.no_data,
        "skill_investigate completed"
    );
    Ok(serde_json::to_value(response)?)
}
```

(Match the exact return shape of `tool_abuse_incidents`/`tool_abuse_investigate` at lines 215-233 — if those wrap with a different helper than `serde_json::to_value`, use that helper instead.)

In `src/api.rs`, add after the existing `ai_investigate` handler (~line 1121):

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AiSkillIncidentsQuery {
    skill: Option<String>,
    plugin: Option<String>,
    tool: Option<String>,
    project: Option<String>,
    session_id: Option<String>,
    hostname: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<u32>,
    window_minutes: Option<u32>,
    #[serde(default)]
    signals: Vec<String>,
    min_score: Option<f64>,
}

async fn ai_skill_incidents(
    State(state): State<ApiState>,
    serde_qs::axum::QsQuery(q): serde_qs::axum::QsQuery<AiSkillIncidentsQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .list_ai_skill_incidents(AiSkillIncidentRequest {
                skill: q.skill,
                plugin: q.plugin,
                tool: q.tool,
                project: q.project,
                session_id: q.session_id,
                hostname: q.hostname,
                since: q.since,
                until: q.until,
                limit: q.limit,
                window_minutes: q.window_minutes,
                signals: q.signals,
                min_score: q.min_score,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AiSkillInvestigateQuery {
    skill: Option<String>,
    plugin: Option<String>,
    tool: Option<String>,
    project: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<u32>,
    window_minutes: Option<u32>,
    correlation_window_minutes: Option<u32>,
    #[serde(default)]
    incident_id: Option<String>,
}

async fn ai_skill_investigate(
    State(state): State<ApiState>,
    serde_qs::axum::QsQuery(q): serde_qs::axum::QsQuery<AiSkillInvestigateQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .investigate_ai_skill_incidents(AiSkillInvestigateRequest {
                incident_id: q.incident_id,
                skill: q.skill,
                plugin: q.plugin,
                tool: q.tool,
                project: q.project,
                since: q.since,
                until: q.until,
                limit: q.limit,
                window_minutes: q.window_minutes,
                correlation_window_minutes: q.correlation_window_minutes,
            })
            .await,
    )
}
```

Register both routes in the router builder next to the existing `/api/sessions/incidents` and `/api/sessions/investigate` routes (~lines 265-266):

```rust
        .route("/api/sessions/skill-incidents", get(ai_skill_incidents))
        .route("/api/sessions/skill-investigate", get(ai_skill_investigate))
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib list_ai_skill_incidents_empty_db_returns_no_data investigate_ai_skill_incidents_empty_db_returns_no_data_flag -- --nocapture
cargo build
```

Expected: both tests `ok`, and `cargo build` succeeds with no errors (this is the first point all layers compile together — expect to fix small type mismatches from the "verify against actual code" callout at this step, e.g. the real `AiSkillEventEntry` name).

- [ ] **Step 5: Commit**

```bash
git add src/app/models/ai_skill_incidents.rs src/app/models.rs src/app/services/ai.rs src/mcp/actions.rs src/mcp/tools.rs src/api.rs src/app/service_tests.rs
git commit -m "feat: wire skill_incidents/skill_investigate through service, MCP, and REST layers"
```

---

### Task 6: Deterministic findings module — `src/app/skill_incident_findings.rs`

**Files:**
- Create: `src/app/skill_incident_findings.rs`
- Modify: `src/app/mod.rs` (add `pub mod skill_incident_findings;` beside `pub mod incident_findings;`)
- Test: `src/app/skill_incident_findings_tests.rs` (sidecar convention)

**Interfaces:**
- Consumes: `SkillIncident`, `LogEntry`, `AiSkillEventEntry` (app-layer types from Task 5), `crate::app::skill_signal_detectors::*` (Tasks 1-2).
- Produces: the `SkillIncidentFindings`/`SkillFailureMode`/`SkillContributingFactor`/`SkillPreventionHint` structs and `derive_skill_incident_findings` function specified in "Locked interfaces" at the top of this document — consumed by `src/app/models/ai_skill_incidents.rs`'s `From<db::SkillIncidentEvidence>` impl (Task 5) and, in a later phase, serialized directly into an LLM prompt.

- [ ] **Step 1: Write the failing test** (full real code)

Create `src/app/skill_incident_findings_tests.rs`:

```rust
use super::*;
use crate::app::models::{LogEntry, SkillIncident, SkillSignalCounts};

fn log(id: i64, message: &str) -> LogEntry {
    LogEntry {
        id,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hostname: "devhost".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        received_at: "2026-01-01T00:00:00Z".to_string(),
        source_ip: "127.0.0.1:0".to_string(),
        ai_tool: Some("codex".to_string()),
        ai_project: Some("/tmp/project".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        ai_transcript_path: None,
        metadata_json: None,
    }
}

fn incident(signals_present: Vec<&str>) -> SkillIncident {
    SkillIncident {
        incident_id: "skill-inc-test".to_string(),
        skill_name: "lavra:lavra-plan".to_string(),
        skill_plugin: Some("lavra".to_string()),
        tool: "codex".to_string(),
        project: "/tmp/project".to_string(),
        session_id: "sess-1".to_string(),
        hostname: "devhost".to_string(),
        first_seen: "2026-01-01T00:00:00Z".to_string(),
        last_seen: "2026-01-01T00:05:00Z".to_string(),
        duration_secs: 300,
        skill_event_count: 1,
        skill_event_ids: vec![1],
        anchor_log_ids: vec![2],
        signal_counts: SkillSignalCounts::default(),
        signals_present: signals_present.into_iter().map(String::from).collect(),
        priority_score: 22.0,
        priority_label: "medium".to_string(),
        window_minutes: 10,
    }
}

#[test]
fn detects_wrong_source_of_truth_category() {
    let inc = incident(vec!["scope_or_source_confusion"]);
    let anchors = vec![log(2, "you're using the wrong source of truth here, check the live container")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == WRONG_SOURCE_OF_TRUTH),
        "expected wrong_source_of_truth category, got {:?}",
        findings.likely_failure_modes
    );
    let mode = findings
        .likely_failure_modes
        .iter()
        .find(|f| f.category == WRONG_SOURCE_OF_TRUTH)
        .unwrap();
    assert_eq!(mode.evidence_ids, vec![2]);
    assert!(
        findings
            .prevention_hints
            .iter()
            .any(|h| h.category == WRONG_SOURCE_OF_TRUTH && h.hint.to_ascii_lowercase().contains("source of truth"))
    );
}

#[test]
fn detects_missing_verification_step_category() {
    let inc = incident(vec!["ignored_skill_or_policy_instruction"]);
    let anchors = vec![log(2, "you claimed success without any verification of the running app")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MISSING_VERIFICATION_STEP)
    );
    let hint = findings
        .prevention_hints
        .iter()
        .find(|h| h.category == MISSING_VERIFICATION_STEP)
        .unwrap();
    assert!(hint.hint.to_ascii_lowercase().contains("verification"));
}

#[test]
fn detects_overly_broad_research_loop_category() {
    let inc = incident(vec!["overlong_loop_after_skill"]);
    let mut counts = SkillSignalCounts::default();
    counts.overlong_loop_after_skill = 1;
    let mut inc2 = inc;
    inc2.signal_counts = counts;
    let anchors = vec![log(2, "that's not what I asked, we wasted twenty minutes going in circles")];
    let findings = derive_skill_incident_findings(&inc2, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == OVERLY_BROAD_RESEARCH_LOOP)
    );
}

#[test]
fn detects_ambiguous_skill_trigger_category() {
    let inc = incident(vec!["skill_scope_mismatch"]);
    let anchors = vec![log(2, "wrong skill triggered, this wasn't the right one for the task")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == AMBIGUOUS_SKILL_TRIGGER || f.category == SKILL_SCOPE_MISMATCH),
        "expected ambiguous_skill_trigger or skill_scope_mismatch, got {:?}",
        findings.likely_failure_modes
    );
}

#[test]
fn weak_evidence_falls_back_to_unknown_with_open_questions() {
    let inc = incident(vec![]);
    let findings = derive_skill_incident_findings(&inc, &[], &[], &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == UNKNOWN)
    );
    assert!(!findings.open_questions.is_empty());
}

#[test]
fn every_finding_cites_evidence_ids_when_not_unknown() {
    let inc = incident(vec!["tool_failure_after_skill"]);
    let anchors = vec![log(2, "command exited with exit code 1")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    for mode in &findings.likely_failure_modes {
        if mode.category != UNKNOWN {
            assert!(
                !mode.evidence_ids.is_empty(),
                "category {} has no evidence ids",
                mode.category
            );
        }
    }
}

#[test]
fn prevention_hints_are_skill_doc_actionable() {
    let inc = incident(vec!["scope_or_source_confusion"]);
    let anchors = vec![log(2, "wrong repo, this is stale data not the live system")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    for hint in &findings.prevention_hints {
        assert!(
            hint.hint.len() > 20,
            "hint should be a concrete actionable sentence: {}",
            hint.hint
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib skill_incident_findings -- --nocapture
```

Expected: compile error — `derive_skill_incident_findings` and the category constants do not exist yet.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/skill_incident_findings.rs` (mirrors `src/app/incident_findings.rs` structure):

```rust
//! Deterministic failure-hypothesis and prevention-hint generation over
//! skill-usage incident evidence bundles. Pure rule evaluation — never
//! queries the database and never calls an external LLM. Mirrors
//! `src/app/incident_findings.rs` (the abuse-incident findings module) but
//! targets skill-specific failure categories.

use serde::{Deserialize, Serialize};

use super::models::{LogEntry, SkillIncident};

// ── Stable failure-mode categories ──────────────────────────────────────────
pub const SKILL_SCOPE_MISMATCH: &str = "skill_scope_mismatch";
pub const MISSING_PREREQUISITE_CHECK: &str = "missing_prerequisite_check";
pub const WRONG_SOURCE_OF_TRUTH: &str = "wrong_source_of_truth";
pub const OVERLY_BROAD_RESEARCH_LOOP: &str = "overly_broad_research_loop";
pub const TOOL_POLICY_MISMATCH: &str = "tool_policy_mismatch";
pub const MISSING_VERIFICATION_STEP: &str = "missing_verification_step";
pub const AMBIGUOUS_SKILL_TRIGGER: &str = "ambiguous_skill_trigger";
pub const STALE_OR_CONFLICTING_SKILL_INSTRUCTION: &str = "stale_or_conflicting_skill_instruction";
pub const ASSISTANT_OVEREXPLAINED_SIMPLE_ANSWER: &str = "assistant_overexplained_simple_answer";
pub const UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillFailureMode {
    pub category: String,
    pub confidence: String,
    pub evidence_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillContributingFactor {
    pub factor: String,
    pub evidence_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillPreventionHint {
    pub category: String,
    pub hint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillIncidentFindings {
    pub likely_failure_modes: Vec<SkillFailureMode>,
    pub contributing_factors: Vec<SkillContributingFactor>,
    pub prevention_hints: Vec<SkillPreventionHint>,
    pub open_questions: Vec<String>,
}

/// `(category, keyword substrings, prevention hint)`. Kept specific — no
/// broad single tokens.
type Rule = (&'static str, &'static [&'static str], &'static str);

const RULES: &[Rule] = &[
    (
        WRONG_SOURCE_OF_TRUTH,
        &[
            "wrong source of truth",
            "wrong source",
            "stale data",
            "memory-vs-live",
            "memory vs live",
            "not the live",
        ],
        "Add a note to the skill doc naming the canonical source of truth for this data \
         (live system vs. memory/cache) and require the agent to confirm which one it used.",
    ),
    (
        WRONG_SOURCE_OF_TRUTH,
        &["wrong repo"],
        "Add a trigger-boundary note clarifying which repo this skill applies to, and require \
         the agent to confirm the working directory before acting.",
    ),
    (
        MISSING_VERIFICATION_STEP,
        &[
            "without any verification",
            "without verification",
            "claimed success without",
        ],
        "Add a verification checklist item requiring live repo/runtime evidence before claiming \
         success.",
    ),
    (
        MISSING_PREREQUISITE_CHECK,
        &[
            "should have created a bead",
            "should have created an issue",
        ],
        "Add a prerequisite-check step to the skill doc: confirm an issue/bead exists (or create \
         one) before starting non-trivial work.",
    ),
    (
        TOOL_POLICY_MISMATCH,
        &[
            "wrong transport",
            "wrong source for this call",
            "raw web instead of using axon",
            "instead of using axon",
        ],
        "Add an explicit tool-policy line to the skill doc naming the required transport/source \
         (e.g. Axon before raw web search) and why.",
    ),
    (
        OVERLY_BROAD_RESEARCH_LOOP,
        &[
            "going in circles",
            "we wasted",
            "all you had to say",
        ],
        "Add an anti-loop rule: after two failed searches, summarize current evidence and switch \
         strategy instead of repeating the same approach.",
    ),
    (
        AMBIGUOUS_SKILL_TRIGGER,
        &[
            "wrong skill",
            "not the right skill",
            "shouldn't have triggered",
            "should not have triggered",
        ],
        "Add a trigger-boundary note that this skill is for implementation planning only (or \
         narrow its stated trigger phrases) so it stops firing on out-of-scope requests.",
    ),
    (
        STALE_OR_CONFLICTING_SKILL_INSTRUCTION,
        &[
            "stale instruction",
            "conflicting instruction",
            "outdated skill",
            "skill doc is wrong",
            "skill doc is out of date",
        ],
        "Review and update the skill doc section that conflicts with current project conventions.",
    ),
    (
        ASSISTANT_OVEREXPLAINED_SIMPLE_ANSWER,
        &[
            "all you had to say was",
            "didn't need to touch",
            "did not need to touch",
            "you didn't need to",
        ],
        "Add a conciseness note to the skill doc: for simple factual questions, answer directly \
         before taking any action.",
    ),
    (
        SKILL_SCOPE_MISMATCH,
        &[
            "out of scope",
            "not what this skill is for",
        ],
        "Narrow the skill's stated scope in its description/trigger phrases to exclude this case.",
    ),
];

fn confidence_for(count: usize) -> &'static str {
    match count {
        0 | 1 => "low",
        2 => "medium",
        _ => "high",
    }
}

const SKILL_EVENT_FACTOR_THRESHOLD: usize = 3;
const ERROR_BURST_THRESHOLD: usize = 3;

fn scannable<'a>(
    signal_anchors: &'a [LogEntry],
    transcript_before: &'a [LogEntry],
    transcript_after: &'a [LogEntry],
    nearby_logs: &'a [LogEntry],
    nearby_errors: &'a [LogEntry],
) -> impl Iterator<Item = &'a LogEntry> {
    signal_anchors
        .iter()
        .chain(transcript_before)
        .chain(transcript_after)
        .chain(nearby_logs)
        .chain(nearby_errors)
}

/// Derive deterministic findings from a skill-incident evidence bundle. Pure
/// and total: identical input always yields identical output; every
/// non-`unknown` failure mode / contributing factor cites at least one
/// evidence id; weak evidence yields `unknown` + `open_questions` rather than
/// an unsupported claim.
pub fn derive_skill_incident_findings(
    incident: &SkillIncident,
    _skill_events: &[super::models::AiSkillEventEntry], // rename type per Task 5 callout if it differs
    signal_anchors: &[LogEntry],
    transcript_before: &[LogEntry],
    transcript_after: &[LogEntry],
    nearby_logs: &[LogEntry],
    nearby_errors: &[LogEntry],
) -> SkillIncidentFindings {
    let mut findings = SkillIncidentFindings::default();

    for (category, keywords, hint) in RULES {
        let mut ids: Vec<i64> = Vec::new();
        for entry in scannable(
            signal_anchors,
            transcript_before,
            transcript_after,
            nearby_logs,
            nearby_errors,
        ) {
            let haystack = entry.message.to_ascii_lowercase();
            if keywords.iter().any(|kw| haystack.contains(kw)) {
                ids.push(entry.id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        if !ids.is_empty() {
            let confidence = confidence_for(ids.len()).to_owned();
            findings.likely_failure_modes.push(SkillFailureMode {
                category: (*category).to_owned(),
                confidence,
                evidence_ids: ids,
            });
            findings.prevention_hints.push(SkillPreventionHint {
                category: (*category).to_owned(),
                hint: (*hint).to_owned(),
            });
        }
    }

    if incident.skill_event_count >= SKILL_EVENT_FACTOR_THRESHOLD && !signal_anchors.is_empty() {
        findings.contributing_factors.push(SkillContributingFactor {
            factor: format!(
                "Repeated skill invocation: {} skill events within the incident window.",
                incident.skill_event_count
            ),
            evidence_ids: signal_anchors.iter().map(|a| a.id).collect(),
        });
    }
    if nearby_errors.len() >= ERROR_BURST_THRESHOLD {
        findings.contributing_factors.push(SkillContributingFactor {
            factor: format!(
                "Error burst: {} error-level logs in the correlation window.",
                nearby_errors.len()
            ),
            evidence_ids: nearby_errors.iter().map(|e| e.id).collect(),
        });
    }

    if findings.likely_failure_modes.is_empty() {
        findings.likely_failure_modes.push(SkillFailureMode {
            category: UNKNOWN.to_owned(),
            confidence: "low".to_owned(),
            evidence_ids: Vec::new(),
        });
        findings.open_questions.push(
            "No deterministic failure signature matched the evidence window; manual transcript \
             review is recommended."
                .to_owned(),
        );
    }
    if signal_anchors.is_empty() {
        findings
            .open_questions
            .push("No negative signal anchors were captured for this incident.".to_owned());
    }
    if nearby_logs.is_empty() && nearby_errors.is_empty() {
        findings.open_questions.push(
            "No surrounding non-AI logs were available to corroborate the transcript signal."
                .to_owned(),
        );
    }

    findings
}

#[cfg(test)]
#[path = "skill_incident_findings_tests.rs"]
mod tests;
```

Add `pub mod skill_incident_findings;` to `src/app/mod.rs` beside `pub mod incident_findings;`.

**Note on the test file's imports:** the test file above references `WRONG_SOURCE_OF_TRUTH`, `MISSING_VERIFICATION_STEP`, etc. via `use super::*;` — since these are declared `pub const` in the module under test, this resolves automatically once the module compiles. The `_skill_events` parameter type `super::models::AiSkillEventEntry` must be swapped for the real type name per the Task-1 callout; if the real type instead lives directly under `crate::db` with no app-layer re-export, change the import to `crate::db::AiSkillEventEntry` (or whatever the real name is) both here and in the `From<db::SkillIncidentEvidence>` call site in Task 5's `src/app/models/ai_skill_incidents.rs`.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib skill_incident_findings -- --nocapture
```

Expected: `test result: ok. 7 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add src/app/skill_incident_findings.rs src/app/skill_incident_findings_tests.rs src/app/mod.rs
git commit -m "feat: add deterministic skill_incident_findings module with 9 failure categories"
```

---

### Task 7: Skill-first CLI UX — `cortex sessions skill-investigate <skill>` positional resolution, `other_matching_incidents`, no-data path

**Files:**
- Modify: `src/cli/args/sessions.rs` (add `SkillIncidents(SessionsSkillIncidentsArgs)` and `SkillInvestigate(SessionsSkillInvestigateArgs)` variants to `SessionsCommand`, plus the two arg structs)
- Modify: `src/cli/parse/sessions.rs` (add `"skill-incidents"` and `"skill-investigate"` to `SESSIONS_SUBCOMMANDS`, add parse functions with positional-arg handling for `skill-investigate`)
- Modify: `src/cli/dispatch_sessions.rs` (add `run_ai_skill_incidents`, `run_ai_skill_investigate` dispatch functions; `run_ai_skill_investigate` implements the skill-first resolution: fetch incidents matching the skill, pick top-priority, populate `other_matching_incidents`)
- Modify: `src/cli/run.rs` (wire the two new `SessionsCommand` variants to their dispatch functions)
- Modify: `src/app/services/ai.rs` (extend `investigate_ai_skill_incidents` from Task 5, or add a wrapping method `investigate_ai_skill_incidents_by_skill`, so the skill-first logic — top-incident selection + `other_matching_incidents` + no-signal low-severity summary — lives at the service layer and is shared by CLI, MCP, and REST, not duplicated in the CLI)
- Test: `src/cli/dispatch_sessions_tests.rs` (sidecar for `dispatch_sessions.rs`), `src/cli/parse/sessions_tests.rs` (sidecar for `parse/sessions.rs`)

**Interfaces:**
- Consumes: `AiSkillInvestigateRequest`/`AiSkillInvestigateResponse`/`AppService::investigate_ai_skill_incidents` (Task 5), `SkillIncidentSummary` (Task 5).
- Produces: `SessionsSkillIncidentsArgs`, `SessionsSkillInvestigateArgs` CLI arg structs; `run_ai_skill_incidents`, `run_ai_skill_investigate` dispatch functions; an extended/wrapping service method that fills `other_matching_incidents` and `no_incident_low_severity_summary` on `AiSkillInvestigateResponse`. This is the final layer other phases interact with only via the wire types already locked in Task 5 — no new types are introduced here beyond CLI arg structs.

- [ ] **Step 1: Write the failing test** (full real code)

First, the service-layer skill-first resolution logic needs a test. Append to `src/app/service_tests.rs`:

```rust
#[tokio::test]
async fn investigate_ai_skill_incidents_by_skill_picks_top_priority_and_lists_others() {
    let service = test_app_service().await; // same helper as Task 5's tests

    // Seed two sessions for the same skill: one low-signal, one high-signal.
    // (Use the service's local DB pool the same way other service_tests.rs
    // tests seed data — grep this file for how `investigate_ai_incidents`
    // tests seed logs via `service.pool()` or an equivalent accessor, and
    // reuse that exact pattern here instead of reinventing seeding.)
    // ... seed sess-low (skill event only) and sess-high (skill event +
    // correction + tool failure) for skill "lavra:lavra-plan" ...

    let response = service
        .investigate_ai_skill_incidents(AiSkillInvestigateRequest {
            skill: Some("lavra:lavra-plan".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(response.evidence.len(), 1, "skill-first default returns the single top incident");
    assert_eq!(
        response.evidence[0].incident.session_id, "sess-high",
        "top-priority incident must be the higher-signal session"
    );
    assert_eq!(
        response.other_matching_incidents.len(),
        1,
        "the lower-priority incident must be summarized in other_matching_incidents"
    );
    assert_eq!(response.other_matching_incidents[0].priority_label, "low");
}

#[tokio::test]
async fn investigate_ai_skill_incidents_low_signal_returns_summary_not_error() {
    let service = test_app_service().await;
    // Seed exactly one skill event for "lavra:lavra-plan" with no negative signal.
    // ...

    let response = service
        .investigate_ai_skill_incidents(AiSkillInvestigateRequest {
            skill: Some("lavra:lavra-plan".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(!response.no_data);
    assert_eq!(response.evidence.len(), 1);
    assert!(response.no_incident_low_severity_summary);
}
```

(This test's seeding comments are intentionally left as guidance rather than fully inlined SQL, because `service_tests.rs`'s existing helper for constructing a populated `AppService` in-process must be reused verbatim — copy the exact seeding helper the file already uses for `investigate_ai_incidents` tests, adapted to insert `ai_skill_events` rows the same way Task 3/4's `insert_skill_event` helper does in `queries_tests.rs`.)

Then the CLI parse test. Create/append to `src/cli/parse/sessions_tests.rs` (check if this sidecar exists yet; if `src/cli/parse/sessions.rs` doesn't already have `#[cfg(test)] #[path = "sessions_tests.rs"] mod tests;` at its end, add it):

```rust
#[test]
fn skill_investigate_binds_bare_positional_to_skill() {
    let cmd = parse_sessions_command(&[
        "skill-investigate".to_string(),
        "lavra:lavra-plan".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Sessions(SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.skill.as_deref(), Some("lavra:lavra-plan"));
            assert!(args.incident_id.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_accepts_since_and_tool_flags_with_positional() {
    let cmd = parse_sessions_command(&[
        "skill-investigate".to_string(),
        "lavra:lavra-plan".to_string(),
        "--since".to_string(),
        "7d".to_string(),
        "--tool".to_string(),
        "codex".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Sessions(SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.skill.as_deref(), Some("lavra:lavra-plan"));
            assert_eq!(args.tool.as_deref(), Some("codex"));
            assert!(args.since.is_some());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_incident_id_flag_overrides_but_does_not_require_positional() {
    let cmd = parse_sessions_command(&[
        "skill-investigate".to_string(),
        "--incident-id".to_string(),
        "skill-inc-deadbeef".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Sessions(SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.incident_id.as_deref(), Some("skill-inc-deadbeef"));
            assert!(args.skill.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_plugin_flag_for_plugin_level_investigation() {
    let cmd = parse_sessions_command(&[
        "skill-investigate".to_string(),
        "--plugin".to_string(),
        "lavra".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Sessions(SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.plugin.as_deref(), Some("lavra"));
            assert!(args.skill.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_all_and_limit_flags() {
    let cmd = parse_sessions_command(&[
        "skill-investigate".to_string(),
        "lavra:lavra-plan".to_string(),
        "--all".to_string(),
        "--limit".to_string(),
        "5".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Sessions(SessionsCommand::SkillInvestigate(args)) => {
            assert!(args.all);
            assert_eq!(args.limit, Some(5));
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib skill_investigate -- --nocapture
```

Expected: compile error — `SessionsCommand::SkillInvestigate`, `parse_sessions_command` handling of `"skill-investigate"`, and the associated arg struct fields do not exist yet.

- [ ] **Step 3: Write minimal implementation**

Add to `src/cli/args/sessions.rs`, extend `SessionsCommand` enum:

```rust
    SkillIncidents(SessionsSkillIncidentsArgs),
    SkillInvestigate(SessionsSkillInvestigateArgs),
```

Add new arg structs at the end of the file:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionsSkillIncidentsArgs {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub signals: Vec<String>,
    pub min_score: Option<String>, // parsed to f64 at into_request() time
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionsSkillInvestigateArgs {
    /// Bare positional argument — the skill name, e.g. `lavra:lavra-plan`.
    /// `None` when the caller used `--incident-id` or `--plugin` instead.
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub incident_id: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    /// Investigate multiple matching incidents instead of just the top one.
    pub all: bool,
    pub json: bool,
}
```

(`min_score` is kept as `Option<String>` in the CLI arg struct and parsed with `str::parse::<f64>()` inside `into_request()`, matching this repo's general pattern of parsing typed values at the request-conversion boundary rather than during flag scanning — see how `--limit` is parsed via `parse_u32_flag` at scan time instead for u32 fields; f64 has no existing shared parser helper in `FlagCursor`/`parse_common.rs`, so parse it inline in `into_request()` with a clear error message.)

Add to `src/cli/parse/sessions.rs`:

1. Add `"skill-incidents"` and `"skill-investigate"` to `SESSIONS_SUBCOMMANDS`.
2. Add dispatch arms in `parse_sessions_command`:

```rust
        "skill-incidents" => parse_sessions_skill_incidents(rest),
        "skill-investigate" => parse_sessions_skill_investigate(rest),
```

3. Add the two parse functions:

```rust
pub(crate) fn parse_sessions_skill_incidents(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SessionsSkillIncidentsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--skill" => parsed.skill = Some(flags.value("--skill")?),
            "--plugin" => parsed.plugin = Some(flags.value("--plugin")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--session-id" => parsed.session_id = Some(flags.value("--session-id")?),
            "--hostname" => parsed.hostname = Some(flags.value("--hostname")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--signal" => parsed.signals.push(flags.value("--signal")?),
            "--min-score" => parsed.min_score = Some(flags.value("--min-score")?),
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
            _ if arg.starts_with("--hostname=") => {
                parsed.hostname = Some(value_after_equals(arg, "--hostname")?)
            }
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with("--window-minutes=") => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    value_after_equals(arg, "--window-minutes")?,
                )?)
            }
            _ if arg.starts_with("--signal=") => {
                parsed.signals.push(value_after_equals(arg, "--signal")?)
            }
            _ if arg.starts_with("--min-score=") => {
                parsed.min_score = Some(value_after_equals(arg, "--min-score")?)
            }
            _ if arg.starts_with('-') => bail!("unknown sessions skill-incidents option: {arg}"),
            _ if parsed.skill.is_none() => parsed.skill = Some(arg),
            _ => bail!("unexpected sessions skill-incidents argument: {arg}"),
        }
    }
    Ok(CliCommand::Sessions(SessionsCommand::SkillIncidents(parsed)))
}

pub(crate) fn parse_sessions_skill_investigate(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SessionsSkillInvestigateArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--all" => parsed.all = true,
            "--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),
            "--plugin" => parsed.plugin = Some(flags.value("--plugin")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--correlation-window-minutes" => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    flags.value("--correlation-window-minutes")?,
                )?)
            }
            _ if arg.starts_with("--incident-id=") => {
                parsed.incident_id = Some(value_after_equals(arg, "--incident-id")?)
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
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with("--window-minutes=") => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    value_after_equals(arg, "--window-minutes")?,
                )?)
            }
            _ if arg.starts_with("--correlation-window-minutes=") => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    value_after_equals(arg, "--correlation-window-minutes")?,
                )?)
            }
            _ if arg.starts_with('-') => bail!("unknown sessions skill-investigate option: {arg}"),
            // Bare positional binds to --skill, exactly like `topic_correlate`
            // binds its positional to --topic (see action_flags.rs / actions.rs
            // TOPIC_CORRELATE_FLAGS positional: Some("--topic") pattern).
            _ if parsed.skill.is_none() => parsed.skill = Some(arg),
            _ => bail!("unexpected sessions skill-investigate argument: {arg}"),
        }
    }
    Ok(CliCommand::Sessions(SessionsCommand::SkillInvestigate(
        parsed,
    )))
}
```

Add `use super::super::{..., SessionsSkillIncidentsArgs, SessionsSkillInvestigateArgs};` to the existing import block at the top of `src/cli/parse/sessions.rs`.

In `src/app/services/ai.rs`, replace the Task 5 `investigate_ai_skill_incidents` body with the skill-first-aware version (keep the same public signature `pub async fn investigate_ai_skill_incidents(&self, req: AiSkillInvestigateRequest) -> ServiceResult<AiSkillInvestigateResponse>`):

```rust
pub async fn investigate_ai_skill_incidents(
    &self,
    req: AiSkillInvestigateRequest,
) -> ServiceResult<AiSkillInvestigateResponse> {
    let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
    let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
    let skill_first = req.incident_id.is_none() && (req.skill.is_some() || req.plugin.is_some());
    let requested_limit = req.limit;

    // Skill-first path: look up ALL matching incidents first (uncapped up to
    // the incident-list cap) so we can rank by priority and report the ones
    // we are not returning as `other_matching_incidents`.
    let lookup_limit = if skill_first { Some(100) } else { requested_limit };

    let result = self
        .run_heavy_db("investigate_ai_skill_incidents", {
            let req_incident_id = req.incident_id.clone();
            let req_skill = req.skill.clone();
            let req_plugin = req.plugin.clone();
            let req_tool = req.tool.clone();
            let req_project = req.project.clone();
            let window_minutes = req.window_minutes;
            let correlation_window_minutes = req.correlation_window_minutes;
            move |pool| {
                db::investigate_ai_skill_incidents(
                    pool,
                    &db::AiSkillInvestigateParams {
                        incident_id: req_incident_id,
                        skill: req_skill,
                        plugin: req_plugin,
                        ai_tool: req_tool,
                        ai_project: req_project,
                        since: from,
                        until: to,
                        limit: lookup_limit,
                        window_minutes,
                        correlation_window_minutes,
                    },
                )
            }
        })
        .await?;

    let no_data = result.evidence.is_empty() && result.total_incidents == 0;
    if no_data {
        let suggested_filters = vec![
            "widen --since (e.g. --since 30d)".to_string(),
            "drop --plugin and filter by --skill only".to_string(),
            "run `cortex sessions skill-incidents` with no filters to see what skills have events"
                .to_string(),
        ];
        return Ok(AiSkillInvestigateResponse {
            evidence: Vec::new(),
            total_incidents: 0,
            truncated: false,
            other_matching_incidents: Vec::new(),
            no_incident_low_severity_summary: false,
            no_data: true,
            suggested_filters,
        });
    }

    let mut evidence: Vec<SkillIncidentEvidence> =
        result.evidence.into_iter().map(Into::into).collect();

    // Already sorted by priority_score desc, last_seen desc (search_ai_skill_incidents
    // guarantees this via total_cmp — see Task 3). For the skill-first path,
    // slice to the requested count (default 1 unless --all, in which case use
    // the caller's --limit or fall back to 3, matching abuse_investigate's
    // default) and summarize the rest into other_matching_incidents.
    let mut other_matching_incidents = Vec::new();
    let mut no_incident_low_severity_summary = false;

    if skill_first {
        let keep = requested_limit.unwrap_or(1).max(1) as usize;
        // A bundle with zero negative-signal counts and zero anchor logs is
        // "low signal" — still return it (never an error), but flag it.
        if evidence.len() == 1 {
            let inc = &evidence[0].incident;
            let zero_signal = inc.signals_present.is_empty();
            if zero_signal {
                no_incident_low_severity_summary = true;
            }
        }
        if evidence.len() > keep {
            other_matching_incidents = evidence[keep..]
                .iter()
                .map(|bundle| SkillIncidentSummary {
                    incident_id: bundle.incident.incident_id.clone(),
                    first_seen: bundle.incident.first_seen.clone(),
                    last_seen: bundle.incident.last_seen.clone(),
                    priority_score: bundle.incident.priority_score,
                    priority_label: bundle.incident.priority_label.clone(),
                })
                .collect();
            evidence.truncate(keep);
        }
    }

    Ok(AiSkillInvestigateResponse {
        total_incidents: result.total_incidents,
        truncated: result.truncated,
        evidence,
        other_matching_incidents,
        no_incident_low_severity_summary,
        no_data: false,
        suggested_filters: Vec::new(),
    })
}
```

**Note:** `req.limit` is now interpreted two ways depending on path — for the plain `incident_id`/no-skill-filter path (Task 5's original callers, e.g. MCP/REST callers that don't set `skill`/`plugin`) it still means "how many incidents to investigate" as before; for the skill-first CLI path (`--all`/`--limit N` per the phase brief) it means "how many top-ranked matching incidents to keep." This dual meaning is intentional per the phase brief ("Add `--all`/`--limit N` for multiple incidents") — document this clearly in Task 8's docs task so callers are not surprised. If a later reviewer finds this ambiguous, splitting into two request fields (`bundle_limit` vs `investigate_top_n`) is a reasonable follow-up but is out of scope for this phase to keep the wire shape stable for the LLM-assessment phase.

Add `SessionsSkillInvestigateArgs::into_request()`/`SessionsSkillIncidentsArgs::into_request()` conversions and dispatch functions to `src/cli/dispatch_sessions.rs`:

```rust
impl SessionsSkillIncidentsArgs {
    pub(crate) fn into_request(self) -> AiSkillIncidentRequest {
        AiSkillIncidentRequest {
            skill: self.skill,
            plugin: self.plugin,
            tool: self.tool,
            project: self.project,
            session_id: self.session_id,
            hostname: self.hostname,
            since: self.since,
            until: self.until,
            limit: self.limit,
            window_minutes: self.window_minutes,
            signals: self.signals,
            min_score: self.min_score.map(|s| s.parse::<f64>()).transpose().ok().flatten(),
        }
    }
}

impl SessionsSkillInvestigateArgs {
    pub(crate) fn into_request(self) -> AiSkillInvestigateRequest {
        AiSkillInvestigateRequest {
            incident_id: self.incident_id,
            skill: self.skill,
            plugin: self.plugin,
            tool: self.tool,
            project: self.project,
            since: self.since,
            until: self.until,
            limit: if self.all { self.limit.or(Some(3)) } else { self.limit.or(Some(1)) },
            window_minutes: self.window_minutes,
            correlation_window_minutes: self.correlation_window_minutes,
        }
    }
}

pub(crate) async fn run_ai_skill_incidents(
    mode: &CliMode,
    args: SessionsSkillIncidentsArgs,
) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.list_ai_skill_incidents(req).await?,
        CliMode::Http(client) => http_or_cancel(client.ai_skill_incidents(&req)).await?,
    };
    print_ai_skill_incidents_response(&response, json)
}

pub(crate) async fn run_ai_skill_investigate(
    mode: &CliMode,
    args: SessionsSkillInvestigateArgs,
) -> Result<()> {
    let json = args.json;
    if args.skill.is_none() && args.plugin.is_none() && args.incident_id.is_none() {
        bail!(
            "sessions skill-investigate requires a skill name (positional), --plugin, or \
             --incident-id, e.g. `cortex sessions skill-investigate lavra:lavra-plan`"
        );
    }
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.investigate_ai_skill_incidents(req).await?,
        CliMode::Http(client) => http_or_cancel(client.ai_skill_investigate(&req)).await?,
    };
    print_ai_skill_investigate_response(&response, json)
}
```

(`print_ai_skill_incidents_response`/`print_ai_skill_investigate_response` are new output-formatting functions — add minimal versions to `src/cli/output/sessions.rs` or `src/cli/output/logs.rs` following the existing `print_ai_incidents_response`/`print_ai_investigate_response_with_options` pattern in `src/cli/output/sessions/more.rs`; for the no-data and low-severity-summary cases, print a clear plain-text message instead of an empty table — e.g. "No skill events found for 'lavra:lavra-plan'. Try: widen --since, or run `cortex sessions skill-incidents` first." and for `other_matching_incidents`, print a short table of `incident_id / first_seen / last_seen / priority_score / priority_label` after the primary evidence bundle.)

Add `ai_skill_incidents`/`ai_skill_investigate` methods to `src/cli/http_client.rs` mirroring `ai_incidents`/`ai_investigate` (~lines around 515-525), and wire `SessionsCommand::SkillIncidents`/`SkillInvestigate` to `dispatch::run_ai_skill_incidents`/`run_ai_skill_investigate` in `src/cli/run.rs` next to the existing `SessionsCommand::Incidents`/`Investigate` arms (~line 101).

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib skill_investigate skill_incidents -- --nocapture
cargo build
```

Expected: all new tests `ok`, full build succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/cli/args/sessions.rs src/cli/parse/sessions.rs src/cli/dispatch_sessions.rs src/cli/run.rs src/cli/http_client.rs src/cli/output/ src/app/services/ai.rs src/app/service_tests.rs
git commit -m "feat: skill-first CLI UX for skill-investigate with positional skill arg and other_matching_incidents"
```

---

### Task 8: Docs — README/CLAUDE.md action table, skill help text, ACTION_SPECS examples

**Files:**
- Modify: `/home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md` (add `skill_incidents`/`skill_investigate` rows to the "MCP Tools" action table, ~where `abuse_incidents`/`abuse_investigate` rows are)
- Modify: `src/mcp/actions.rs` (add `examples:` to the two `action_spec!` entries added in Task 5, using the real `cortex sessions skill-incidents`/`cortex sessions skill-investigate` command form, not the illustrative `cortex ai skill-investigate` form from the phase brief)
- Modify: `README.md` if it documents the MCP action list or CLI subcommands (grep first: `grep -n "abuse_incidents\|abuse_investigate" README.md`)
- Test: none (docs-only; verified by `cargo xtask check-version-sync`-style eyeball review and by Task 5-7's existing tests already covering the underlying behavior)

**Interfaces:**
- Consumes: nothing new — this task only documents interfaces already produced by Tasks 1-7.
- Produces: nothing new — no code interfaces.

- [ ] **Step 1: Write the failing test** (docs-only task — no unit test; instead this step defines the acceptance check that must fail before Step 3)

```bash
grep -n "skill_incidents\|skill_investigate" /home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md
```

Expected: no output (rows do not exist yet — this "failing test" is the absence of the documentation).

- [ ] **Step 2: Run test to verify it fails**

```bash
grep -c "skill_incidents\|skill_investigate" /home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md || echo "0 matches (expected — docs not yet added)"
```

Expected: `0 matches (expected — docs not yet added)`.

- [ ] **Step 3: Write minimal implementation**

In `CLAUDE.md`, find the MCP Tools table row for `abuse_investigate | Deep-dive investigation of an abuse incident |` and add two new rows immediately after it:

```markdown
| `skill_incidents` | List detected skill-usage incidents (negative signals after a skill loaded) |
| `skill_investigate` | Deep-dive investigation of a skill-usage incident, skill-first (accepts a skill name directly) |
```

Also update the action count in the "One MCP tool" sentence, e.g. change `47 actions` to `49 actions` (grep `47 actions` in `CLAUDE.md` first to find the exact sentence and confirm the current count before incrementing — count must reflect `ACTION_SPECS` length after Task 5's two additions).

In `src/mcp/actions.rs`, update the two `action_spec!` entries from Task 5 to include `examples:` (switching to the full macro form used by `topic_correlate`):

```rust
    action_spec!(
        "skill_incidents",
        Read,
        "List detected skill-usage incidents (negative signals after a skill loaded)",
        Moderate,
        SkillIncidents,
        flags: &[],
        examples: &[
            "cortex sessions skill-incidents --skill lavra:lavra-plan --since 7d",
            "cortex sessions skill-incidents --plugin lavra --min-score 35",
        ]
    ),
    action_spec!(
        "skill_investigate",
        Read,
        "Deep-dive investigation of a skill-usage incident, skill-first",
        Expensive,
        SkillInvestigate,
        flags: &[],
        examples: &[
            "cortex sessions skill-investigate lavra:lavra-plan",
            "cortex sessions skill-investigate lavra:lavra-plan --since 7d",
            "cortex sessions skill-investigate lavra:lavra-plan --tool codex --project /home/jmagar/workspace/cortex",
            "cortex sessions skill-investigate --plugin lavra --all --limit 5",
        ]
    ),
```

(Use the "full form" `action_spec!` macro arm — grep `macro_rules! action_spec` in `src/mcp/actions.rs` to confirm the exact arm shape for `flags: ..., examples: ...` without `positional`/`defaults`, matching e.g. the `"abuse"` entry's simpler form if it has one, or add `positional: Some("--skill"), defaults: Defaults::new()` to `skill_investigate` if the CLI's positional-binding metadata needs to flow through this table too — check whether `positional_for()`/`defaults_for()` in this file are actually consumed by the CLI parser Task 7 wrote, or whether Task 7's parser is fully self-contained. If `positional_for("skill_investigate")` is consulted anywhere in CLI completion/help code, add `positional: Some("--skill")` to keep it consistent with Task 7's parser behavior.)

If `README.md` documents the action list (from Step 1's grep), add corresponding rows there too, matching whatever format that grep reveals.

- [ ] **Step 4: Run test to verify it passes**

```bash
grep -c "skill_incidents\|skill_investigate" /home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md
cargo build
cargo test --lib mcp::actions
```

Expected: grep count `>= 2`; `cargo build` succeeds; any existing `src/mcp/actions.rs` sidecar tests (e.g. one asserting `ACTION_SPECS.len()` or exercising `positional_for`/`defaults_for`) still pass — if such a test hardcodes the total action count, update it to match the new total (this is why Step 4 runs the actions test suite explicitly; find it via `grep -rn "ACTION_SPECS.len()\|action_specs_tests" src/mcp/`).

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md src/mcp/actions.rs README.md
git commit -m "docs: document skill_incidents/skill_investigate MCP actions and CLI examples"
```

---

## Post-phase integration check (run after all 8 tasks)

Once every task above is committed, run the full-crate gate before considering this phase done:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo xtask check-version-sync
```

Bump the version per this repo's `CLAUDE.md` convention (`cargo xtask bump-version minor` — this phase adds new MCP actions, which is a `feat`-level change) and add a `CHANGELOG.md` entry describing the new `skill_incidents`/`skill_investigate` actions, the `src/app/skill_incident_findings.rs` deterministic findings module, and the skill-first `cortex sessions skill-investigate <skill>` CLI UX.

## Self-Review

### Spec coverage

| Spec requirement | Task(s) |
|---|---|
| `user_correction_after_skill` signal-anchor detector | Task 1 |
| `tool_failure_after_skill` signal-anchor detector | Task 1 |
| `scope_or_source_confusion` signal-anchor detector | Task 2 |
| `ignored_skill_or_policy_instruction` signal-anchor detector | Task 2 |
| `overlong_loop_after_skill` signal-anchor detector (volume + co-occurring negative signal, never long-but-successful alone) | Task 2 |
| Grouping by `(skill_name, skill_plugin, tool, project, session_id, hostname, window_bucket)` | Task 3 |
| Stable `incident_id` (`skill-inc-{:016x}` hash of skill/plugin/tool/project/session/hostname + sorted anchor/event ids) | Task 3 |
| Locked scoring formula + `priority_label` thresholds (low/medium/high/critical) | Task 3 |
| Sort stability via `f64::total_cmp` (not `partial_cmp`/`unwrap_or(Equal)`) | Task 3 (query sort + explicit NaN regression assertion in the test), reiterated in Global Constraints |
| `min_score` / `signals` post-grouping filters | Task 3 |
| Bounded candidate fetch with `candidate_window_truncated`/`candidate_cap` | Task 3 |
| `skill_incidents` read surface — DB query | Task 3 |
| `skill_incidents` read surface — service method, MCP action, REST route | Task 5 |
| `skill_incidents` read surface — CLI (`cortex sessions skill-incidents`) | Task 7 |
| Investigation evidence bundle (`SkillIncidentEvidence`) with 8 bounded collections and truncation flags on every one | Task 4 |
| Exact `incident_id` targeting outside the top page (uncapped lookup when `incident_id` is set) | Task 4 |
| `skill_investigate` read surface — DB query | Task 4 |
| `skill_investigate` read surface — service method, MCP action, REST route | Task 5 |
| `skill_investigate` read surface — CLI (`cortex sessions skill-investigate <skill>`) | Task 7 |
| Deterministic findings module (`SkillIncidentFindings`/`SkillFailureMode`/`SkillContributingFactor`/`SkillPreventionHint`), pure, no DB/LLM calls | Task 6 |
| All 9 named failure categories (`skill_scope_mismatch`, `missing_prerequisite_check`, `wrong_source_of_truth`, `overly_broad_research_loop`, `tool_policy_mismatch`, `missing_verification_step`, `ambiguous_skill_trigger`, `stale_or_conflicting_skill_instruction`, `assistant_overexplained_simple_answer`) plus `unknown` fallback | Task 6 |
| Every non-`unknown` finding cites `evidence_ids`; weak evidence falls back to `unknown` + `open_questions` instead of an unsupported claim | Task 6 |
| Prevention hints are skill-doc-actionable (concrete, >20 chars, named source/verification/anti-loop fixes) | Task 6 |
| Findings attached into wire type via `From<db::SkillIncidentEvidence>` (mirrors `IncidentEvidence::from` pattern) | Task 5 (conversion), Task 6 (findings source) |
| Skill-first CLI UX: `<skill>` bare positional arg binds to `--skill` | Task 7 |
| Skill-first CLI UX: `--all`/`--limit` for multiple incidents | Task 7 |
| Skill-first CLI UX: `--plugin` for plugin-level investigation | Task 7 |
| Skill-first CLI UX: `--incident-id` exact targeting, independent of positional | Task 7 |
| No-data path (`no_data` + `suggested_filters`) | Task 5 (base wiring), Task 7 (skill-first-aware final version) |
| No-incident/low-severity path (`no_incident_low_severity_summary`, zero-signal bundle returned instead of an error) | Task 7 |
| `other_matching_incidents` (top-priority incident selected, remainder summarized) | Task 7 |
| Docs: `CLAUDE.md` MCP action table rows + action count, `ACTION_SPECS` examples using the real CLI command form | Task 8 |
| Full-crate gate (`cargo fmt`, `cargo clippy -D warnings`, `cargo test`, `cargo xtask check-version-sync`) | Post-phase integration check (run after Task 8) |

No gap was found that required a new task: every signal-anchor category, the grouping/scoring/incident-id contract, both read surfaces across all four transports (DB/service/MCP/REST/CLI), the skill-first CLI UX, and the deterministic findings module (all 9 categories + `unknown`) each have an explicit implementing task with real code, not a placeholder. No new task was added.

### Placeholder scan

Two intentional, explicitly-flagged resolution points remain in the plan body — both are pre-existing verification callouts from the source material, not unresolved placeholders left for a later phase:

- Task 3 and Task 4 flag that `crate::app::skill_signal_detectors` may need to move to `src/db/skill_signal_detectors.rs` if `src/db/queries.rs` cannot depend on `src/app/` (module-layering check via `grep -n "^use crate::app" src/db/queries.rs`, resolved by `cargo check`). This is a build-time verification gate with an explicit fallback path spelled out in prose, not a TODO.
- Task 4 flags that `map_skill_event_row` is a placeholder name for the real row-mapper function from the PR 2 skill-events phase, to be located via grep and reused rather than reimplemented. This is consistent with the Global Constraints' PR-2-dependency verification requirement and is resolved before Task 4's Step 3 is written for real.

No `TODO`, `unimplemented!()`, `todo!()`, or stub function bodies exist in any task's "Write minimal implementation" step — every step contains complete, compilable Rust with real logic, matching the source material's TDD Step 1→ Step 5 structure (failing test → verify fail → real implementation → verify pass → commit).

### Type consistency

Verified field-for-field identical usage of the four locked types across every task that touches them:

- **`SkillIncident`** (`incident_id`, `skill_name`, `skill_plugin`, `tool`, `project`, `session_id`, `hostname`, `first_seen`, `last_seen`, `duration_secs`, `skill_event_count`, `skill_event_ids`, `anchor_log_ids`, `signal_counts`, `signals_present`, `priority_score`, `priority_label`, `window_minutes`) — identical field set and order in the DB-layer definition (Task 3, `src/db/models.rs`) and the app-layer mirror (Task 5, `src/app/models/ai_skill_incidents.rs`), connected by a 1:1 `From<db::SkillIncident>` impl. The test-construction helper in Task 6 (`skill_incident_findings_tests.rs`) builds the same 18-field struct with matching names.
- **`SkillIncidentEvidence`** (`incident`, `skill_events`/`skill_events_truncated`, `signal_anchors`/`signal_anchors_truncated`, `transcript_before`/`_truncated`, `transcript_after`/`_truncated`, `nearby_tool_failures`/`_truncated`, `nearby_user_corrections`/`_truncated`, `nearby_logs`/`_truncated`, `nearby_errors`/`_truncated`) — identical shape in the DB layer (Task 4) and the app-layer mirror (Task 5), which adds exactly one extra field, `findings: SkillIncidentFindings`, populated inside the `From<db::SkillIncidentEvidence>` conversion — consistent with how the locked-interfaces section at the top of the plan describes `AiSkillInvestigateResult`/`SkillIncidentEvidence` as the type an LLM-assessment phase will later serialize.
- **`AiSkillInvestigateResponse`** (`evidence`, `total_incidents`, `truncated`, `other_matching_incidents`, `no_incident_low_severity_summary`, `no_data`, `suggested_filters`) — defined once in Task 5 and never redefined; Task 7 only replaces the *body* of `AppService::investigate_ai_skill_incidents` (to populate `other_matching_incidents`/`no_incident_low_severity_summary` for the skill-first path) while keeping the exact same public signature and return type, so the wire shape stays stable for CLI, MCP, and REST callers alike, and for the downstream PR 4 dependency.
- **`SkillIncidentFindings`** (`likely_failure_modes: Vec<SkillFailureMode>`, `contributing_factors: Vec<SkillContributingFactor>`, `prevention_hints: Vec<SkillPreventionHint>`, `open_questions: Vec<String>`) and its three element types (`SkillFailureMode { category, confidence, evidence_ids }`, `SkillContributingFactor { factor, evidence_ids }`, `SkillPreventionHint { category, hint }`) — defined once in Task 6, matching the "Locked interfaces" section at the top of the document verbatim, and consumed unchanged by Task 5's `From<db::SkillIncidentEvidence>` conversion and by Task 6's own test suite.

No task redefines any of these four types with a divergent field set, and every cross-task reference (Task 5 → Task 6 findings; Task 7 → Task 5 request/response types; Task 8 → Task 5 action names) uses the same names throughout.
