# LLM Invocation Guard + Audit Infrastructure (GH #94 PR 1/4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a shared, audited, rate-limited/circuit-broken LLM invocation guard that every future LLM-backed assessment feature must call through, and migrate the existing local-only `cortex sessions assess` (Gemini CLI subprocess) assessment path onto it.

**Architecture:** A single `LlmRunner` (`src/app/llm_runner.rs`) becomes the sole enforcement point for every LLM invocation: it checks a global kill switch and per-action enablement, enforces global and per-action concurrency permits, a sliding-window per-action rate limit, and a consecutive-failure circuit breaker, then spawns the caller-supplied `run_fn` under a timeout and truncates/persists its output. Every call attempt — including denials — writes a row to a new `llm_invocations` audit table (migration 37) via a dedicated `src/db/llm_invocations.rs` query layer, and `CortexService` exposes the runner plus a read-only `llm_invocations_checked` accessor so CLI (`cortex sessions llm-invocations`), MCP (`llm_invocations` action), and REST (`GET /api/sessions/llm-invocations`) surfaces can all inspect the audit trail. `cortex sessions assess` is migrated from calling the Gemini subprocess directly to calling `LlmRunner::run`, proving the guard end-to-end before any later phase builds on it.

**Tech Stack:** Rust 2024 edition, `rusqlite` (bundled SQLite, WAL mode), `tokio` async runtime + `Semaphore`/`Mutex` for concurrency/rate-limit state, `thiserror` for typed errors, `serde`/`serde_json` for wire types.

## Global Constraints

- No LLM invocation may bypass `LlmRunner::run` (this plan's Task 3) — this is the single enforcement point for concurrency/rate-limit/circuit-breaker/kill-switch/audit.
- No background LLM enrichment unless `[llm].background_enrichment_enabled=true`.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass before any task is considered done.
- Every new MCP action needs a row in `src/mcp/actions.rs` (`ACTION_SPECS`) + a dispatch arm in `src/mcp/tools.rs` + docs updates (`docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, `docs/contracts/mcp-actions-current.md`, `CLAUDE.md` action table + count).
- **PR sequencing note:** This is PR 1 of 4 for GH #94 Plan A. PR 2 (skill event extraction, separate plan doc) also needs a new migration number and was drafted independently claiming the SAME migration number this PR claims. Whichever of PR 1 / PR 2 merges first keeps its claimed migration number as-is; the SECOND one to be implemented must re-verify the actual current `KNOWN_SCHEMA_VERSION` in `src/db/pool.rs` at implementation time (Task 1's own Step 1 already does a live grep-based check — trust that, not this note's assumption) and adjust its migration number accordingly if a collision would occur. PR 3 (skill incidents) depends on PR 2, not this PR. PR 4 (skill assessment + unified `cortex assess` CLI) depends on BOTH this PR (for `LlmRunner`) and PR 3 (for skill-incident evidence types) — do not implement PR 4 until both are merged.

---

## Eng Review Fixes Applied

This plan went through four independent engineering-review passes
(architecture, simplicity, security, performance) before implementation.
Five MUST-FIX issues were found and are already folded into the task bodies
above — implementers following this plan top-to-bottom get the fixed
version, not the original. This section documents what changed and why, so
reviewers of the plan itself (and anyone diffing against an earlier draft)
can see the fixes without re-deriving them.

**None of these fixes reduce scope.** GH #94's "Mandatory LLM Observability
and Safety Controls" acceptance criteria (concurrency limit, rate limit,
circuit breaker, timeout, dry-run/preview mode, kill switch, per-action
enablement, and CLI/MCP/REST read surfaces) are all still fully implemented
by Tasks 1–7 exactly as before. These fixes correct bugs and close gaps in
that implementation; they do not cut any required capability.

### Fix 1 — Timeout duplication regression (architecture + performance reviewers)

**Problem:** After Task 6 migrates `cortex sessions assess` onto `LlmRunner`,
two independently-configured timeouts would have wrapped the same Gemini
subprocess call: `LlmRunner::run`'s outer `tokio::time::timeout` (driven by
`[llm].timeout_secs`, Task 2, default 120) and `run_gemini_assessment`'s own
internal timeouts (driven by `GeminiAssessConfig::timeout_secs`, resolved
from the pre-existing `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` env var, also
default 120 but a SEPARATE value). If an operator changed only one of the
two, the effective timeout would silently become `min(both)` — a real,
silent latency regression for the one existing production caller.

**Fix (approach (b) — single source of truth):** `GeminiAssessConfig::from_env`
(`src/assessment.rs`) no longer reads `CORTEX_LLM_COMPLETION_TIMEOUT_SECS`;
its signature changes to `from_env(model_override: Option<String>, timeout_secs: u64)`,
and Task 6's call site passes `self.llm().timeout_secs()` (a new `pub fn
timeout_secs(&self) -> u64` accessor added to `LlmRunner` in Task 3). Setting
the legacy env var now logs a `tracing::warn!` deprecation notice instead of
silently taking effect. Approach (a) — adding a `timeout_override: Option<Duration>`
field to the locked `LlmInvocationSpec` — was considered and rejected: it
would only have fixed the OUTER timeout while leaving `run_gemini_assessment`'s
four internal `tokio::time::timeout` calls (stdout stream, process wait,
stdin close, stderr read) still driven by the old env var, and it would have
widened a cross-PR locked interface unnecessarily.

**Also fixed:** the CHANGELOG (Task 9) and `docs/CONFIG.md` (Task 8) now
explicitly call out that `cortex sessions assess` enforces `max_concurrent=1`
/ `max_per_action_concurrent=1` by default — a real, user-visible behavior
change (two overlapping interactive `assess` calls now hard-fail the second)
that the original docs task didn't mention.

**Tasks/sections changed:** Locked interfaces (`LlmRunner::new`/`timeout_secs()`
doc comments), Task 3 (`timeout_secs()` accessor + test), Task 6 (Files/Interfaces/
Step 1 test/Step 3 implementation/commit message), Task 8 (behavior-change
doc callout), Task 9 (CHANGELOG "Changed" section).

### Fix 2 — Missing `write_lock()` on audit writes (architecture + performance reviewers) — CRITICAL

**Problem:** `src/db/pool.rs` documents a standing invariant: every writer
must serialize through `crate::db::write_lock()` (a process-wide reentrant
mutex) because SQLite allows only one writer and this repo runs several
concurrent writer subsystems (syslog/Docker ingest, heartbeat, notifications,
retention maintenance). `src/db/ingest.rs`'s batch inserter,
`src/db/maintenance.rs`, and `src/app/error_detection/scanner.rs` all honor
this. Task 3's original `write_start_row`/`write_finish_row_inner` and
Task 4's `insert_llm_invocation_running`/`finish_llm_invocation` called
`pool.get()` + `conn.execute(...)` with NO write-lock guard — every audit
INSERT/UPDATE would have raced every other writer for SQLite's single write
lock, the exact hazard the invariant exists to prevent.

**Fix:** every audit write in Task 3's `write_start_row` and
`write_finish_row_inner` now acquires `let _write_guard = crate::db::write_lock();`
immediately before its `conn.execute(...)` call, mirroring the exact
acquire/scope pattern in `src/db/ingest.rs`'s `insert_logs_batch_once`. Task 4's
`crate::db::llm_invocations::{insert_llm_invocation_running, finish_llm_invocation}`
take a plain `&rusqlite::Connection` (like `insert_logs_batch_in_tx` does) and
deliberately do NOT lock themselves — the guard stays at the `llm_runner.rs`
call site, and Task 4's replacement instructions now explicitly say to keep
the `write_lock()` line unchanged when swapping the closure body to call the
shared query-layer functions.

**Tests added:** a new async test in Task 3
(`audit_write_does_not_race_a_concurrent_write_locked_writer`) runs 20
audited `LlmRunner::run` calls concurrently with 20 real
`crate::db::insert_logs_batch` calls (the actual syslog batch inserter) and
asserts neither side fails — the observable symptom (`database is locked` /
write failures) the invariant exists to prevent.

**Tasks/sections changed:** Task 3 (Interfaces, module doc comment, Step 1
test, Step 3 `write_start_row`/`write_finish_row_inner` implementations),
Task 4 (Step 3 replacement instructions, explicit "keep the guard" note).

### Fix 3 — Weak/mistimed error redaction (security reviewer, MP1)

**Problem:** Task 3's original `sanitize_error` forked a weaker heuristic
than the existing `looks_secretish` in `src/assessment.rs` — it checked only
`API_KEY=`/`TOKEN=`/`SECRET=` and omitted the `sk-`/`ghp_`/`atk_` prefix
checks the real heuristic already has. It also truncated to 2048 chars
BEFORE redacting, which could split a secret across the truncation boundary
and let the surviving half leak into the persisted `error` column.

**Fix:** `src/assessment.rs`'s `redact_secrets`/`looks_secretish` are widened
from private `fn` to `pub(crate) fn` (visibility-only change, no behavior
change) and `llm_runner.rs` imports and calls the real `redact_secrets`
directly instead of forking its own copy. `sanitize_error` now redacts FIRST,
then truncates to 2048 chars — the reverse of the original order.

**Tests added:** `error_with_secretish_tokens_is_redacted_before_persisting`
(end-to-end: an error containing `API_KEY=`, `sk-`, `ghp_`, and `TOKEN=`
shaped secrets is redacted in the persisted `llm_invocations.error` column)
and `sanitize_error_redacts_before_truncating_so_boundary_split_secrets_do_not_leak`
(a unit test that places a secret exactly at the old 2048-char truncation
boundary and asserts it's still fully redacted).

**Tasks/sections changed:** Task 3 (Interfaces, Step 1 prerequisite widening
`src/assessment.rs` visibility, Step 1 new tests, Step 3 `sanitize_error`
implementation).

### Fix 4 — `llm_invocations` read surface over-broadly scoped `cortex:read` (security reviewer, MP2)

**Problem:** Task 7 originally scoped the `llm_invocations` MCP action and
`GET /api/sessions/llm-invocations` REST route at `cortex:read`. This
table's `status`/`error`/`metadata_json` columns reveal circuit-breaker
state, kill-switch state, exact rate-limit thresholds, and host/pid — an
operational side-channel this repo's trust model does not treat as
`cortex:read`-safe (per this repo's own docs: "MCP endpoint is
unauthenticated by default... any client reaching port 3100 has full log
read access" absent `CORTEX_TOKEN`).

**Fix:** the `llm_invocations` action's `action_spec!` row in
`src/mcp/actions.rs` is now scoped `Admin`, matching the repo's existing 4
admin actions (`ack_error`, `unack_error`, `file_tails`,
`notifications_test`). This automatically sweeps `llm_invocations` into the
pre-existing generic scope-enforcement test loops in
`src/mcp/rmcp_server_tests.rs` (`mounted_policy_with_read_scope_permits_read_actions`,
`public_read_actions_require_cortex_read_scope`) with no edits needed to
those tests. On the REST side, this repo DOES have an admin/read
distinction already (`require_api_admin_token`, gated by
`CORTEX_API_ADMIN_TOKEN` / the `X-Cortex-Admin-Token` header, used by every
other admin route) — `GET /api/sessions/llm-invocations` now calls it,
matching the existing `ack_error`/`db_checkpoint` admin handler pattern. On
the CLI HTTP-client side, a new `get_json_with_admin` helper was added to
`src/cli/http_client.rs` (mirroring the existing POST-only
`post_json_with_admin_no_retry`) since no GET+admin-token helper existed
before this — `llm_invocations` is the first admin-gated GET route in this
repo.

**Tests added:** `llm_invocations_action_requires_admin_scope` (asserts
`required_scope_for("llm_invocations") == Some("cortex:admin")`) and
`llm_invocations_action_is_denied_for_read_only_scope` (an authed-request
test asserting a `cortex:read`-only caller gets a `-32600` forbidden
response), both in `src/mcp/rmcp_server_tests.rs`, mirroring this repo's
existing explicit `sessions_action_requires_read_scope` test alongside its
generic scope loops.

**Tasks/sections changed:** Task 7 (Files, Interfaces, Step 1 new tests,
Step 3 `action_spec!` scope + REST handler + CLI http_client changes, Step 4
test run, Step 5 commit), Task 8 (CLAUDE.md action table + scope-taxonomy
sentence, REST route doc admin note), Task 9 (CHANGELOG "Security" section).

### Fix 5 — Unbounded/unredacted `extra_metadata` (security reviewer, MP3)

**Problem:** `LlmInvocationSpec.extra_metadata: serde_json::Value` (Task 3)
is caller-supplied and was merged verbatim into `metadata_json` with no size
bound and no redaction pass. A future caller (PR 2-4) could stuff
prompt/evidence content into it, bypassing the "we only store byte counts,
not content" design goal — and even after Fix 4 scopes the read surface to
`cortex:admin`, an admin-scoped table is still a persisted, readable table
that should not silently accumulate secrets or unbounded blobs.

**Fix:** `build_metadata_json` (Task 3) now runs the merged metadata object
through the same `redact_secrets` pass used for error sanitization (Fix 3),
THEN enforces a new `const LLM_METADATA_MAX_BYTES: usize = 4096` cap —
distinct from and much smaller than `max_output_bytes`, since metadata is
documented as being for small structured tags only. Oversized metadata (even
after redaction) is replaced with an explicit `{"truncated": true, ...}`
marker object rather than being silently stored in full or silently dropped.
The `extra_metadata` contract ("MUST NOT carry prompt/evidence content — for
small structured tags only") is now documented directly on the field in the
"Locked interfaces for other phases" block at the top of this plan, since
PR 2-4 read this contract.

**Tests added:** `extra_metadata_with_secretish_value_is_redacted_before_persisting`
(a `TOKEN=`-shaped value inside `extra_metadata` is redacted in the
persisted `metadata_json` column) and
`extra_metadata_over_byte_cap_is_truncated_not_silently_dropped` (an 8KB
`extra_metadata` value is capped near `LLM_METADATA_MAX_BYTES` with an
explicit truncation marker, not silently stored whole).

**Tasks/sections changed:** Locked interfaces (`extra_metadata` contract doc
note), Task 3 (Interfaces, `LLM_METADATA_MAX_BYTES` constant, Step 1 new
tests, Step 3 `build_metadata_json` implementation).

---

## Locked interfaces for other phases

Other phases (skill assessment, abuse assessment, MCP assessment, hook
assessment) MUST call the LLM through this API. Do not invoke Gemini/any LLM
subprocess directly — always go through `LlmRunner::run`.

```rust
// src/app/llm_runner.rs

/// Who is making this LLM call. Recorded verbatim into
/// `llm_invocations.caller_surface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmCallerSurface {
    Cli,
    Mcp,
    Rest,
    Background,
    Test,
}

impl LlmCallerSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Rest => "rest",
            Self::Background => "background",
            Self::Test => "test",
        }
    }
}

/// Evidence bundle sizing info recorded into `evidence_counts_json`.
/// Callers build this before invoking `LlmRunner::run` so denials
/// (concurrency/rate-limit/circuit-open/disabled) still get an accurate
/// audit record without needing the LLM call to happen.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LlmEvidenceCounts {
    pub total_incidents: usize,
    pub evidence_bundle_count: usize,
    pub total_anchors: usize,
    pub truncated: bool,
}

/// Everything needed to run (or dry-run) one LLM invocation.
#[derive(Debug, Clone)]
pub struct LlmInvocationSpec {
    pub caller_surface: LlmCallerSurface,
    /// Action name, e.g. "ai_assess", "skill_assess". Used for per-action
    /// concurrency/rate-limit/circuit-breaker/enablement lookups.
    pub action: String,
    pub incident_id: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub evidence_counts: LlmEvidenceCounts,
    pub prompt: String,
    /// Provider identifier, e.g. "gemini-cli".
    pub provider: String,
    pub model: String,
    /// Program invoked, e.g. "gemini".
    pub program: String,
    /// Extra caller-defined metadata merged into `metadata_json`
    /// (host/pid are always added by the runner itself).
    ///
    /// **CONTRACT (do not violate in PR 2-4):** `extra_metadata` is for small
    /// structured tags only (e.g. `{"skill": "cortex-frustration-assessment"}`).
    /// It MUST NOT be used to carry prompt text, evidence bundle content, or
    /// any other bulk/sensitive payload — `llm_invocations` is an
    /// operational audit table, not an evidence store, and (per the eng
    /// review security fixes below) is scoped `cortex:admin`, not a general
    /// dumping ground. The runner redacts secret-shaped substrings and hard-
    /// caps the serialized size at `LLM_METADATA_MAX_BYTES` (4096 bytes) —
    /// see Task 3's `build_metadata_json` — so oversized or leaked-looking
    /// `extra_metadata` is truncated/redacted, not silently stored whole.
    pub extra_metadata: serde_json::Value,
}

/// Result of a completed (non-dry-run) invocation.
#[derive(Debug, Clone)]
pub struct LlmInvocationOutcome {
    pub invocation_id: String,
    pub output: String,
    pub duration_ms: i64,
    pub output_bytes: usize,
}

/// Result of a dry-run/preview invocation — no LLM call is made.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmDryRunOutcome {
    pub invocation_id: String,
    pub prompt_bytes: usize,
    pub evidence_counts: LlmEvidenceCounts,
    pub would_exceed_prompt_limit: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmRunnerError {
    #[error("LLM invocations are globally disabled ([llm].enabled=false or CORTEX_LLM_ENABLED=false)")]
    Disabled,
    #[error("LLM action '{0}' is disabled ([llm.actions.{0}].enabled=false)")]
    ActionDisabled(String),
    #[error("prompt size {actual} bytes exceeds max_prompt_bytes {limit} bytes")]
    PromptTooLarge { actual: usize, limit: usize },
    #[error("global concurrency limit reached (max_concurrent={0})")]
    ConcurrencyLimited(usize),
    #[error("per-action concurrency limit reached for '{action}' (max_per_action_concurrent={limit})")]
    ActionConcurrencyLimited { action: String, limit: usize },
    #[error("rate limit exceeded for action '{action}': {detail}")]
    RateLimited { action: String, detail: String },
    #[error("circuit open for action '{action}' until {retry_after}")]
    CircuitOpen { action: String, retry_after: String },
    #[error("LLM invocation '{0}' timed out after {1}s")]
    Timeout(String, u64),
    #[error("LLM invocation '{0}' was cancelled")]
    Cancelled(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub struct LlmRunner { /* private fields; constructed via LlmRunner::new */ }

impl LlmRunner {
    /// `pool` is used to write `llm_invocations` audit rows. `config` is the
    /// `[llm]` section of `Config`, and `config.timeout_secs` is now the
    /// SINGLE source of truth for LLM invocation timeouts end to end (see
    /// "Eng Review Fixes Applied" / Fix 1 below): Task 6 threads this same
    /// value into `GeminiAssessConfig` construction instead of that struct
    /// independently re-reading `CORTEX_LLM_COMPLETION_TIMEOUT_SECS`, so the
    /// outer `LlmRunner::run` timeout and the inner Gemini-subprocess
    /// timeout can never drift apart.
    pub fn new(pool: std::sync::Arc<crate::db::DbPool>, config: crate::config::LlmConfig) -> Self;

    /// The resolved `[llm].timeout_secs` this runner enforces as its outer
    /// per-invocation timeout. Added by the eng review Fix 1 (architecture +
    /// performance reviewers): callers that need to configure an INNER
    /// timeout that must never exceed the runner's outer one (e.g. Task 6's
    /// `GeminiAssessConfig`, whose own `tokio::time::timeout` calls guard
    /// the Gemini subprocess's stdout/stdin/stderr/exit handling) read this
    /// accessor instead of independently resolving their own timeout value
    /// from a separate env var, so there is exactly one source of truth.
    pub fn timeout_secs(&self) -> u64;

    /// Build the prompt/evidence bundle size report without invoking the LLM.
    /// Still writes an audit row (status "dry_run"). Does NOT consult
    /// concurrency/rate-limit/circuit-breaker state — sizing is always safe
    /// to preview.
    pub async fn dry_run(&self, spec: &LlmInvocationSpec) -> Result<LlmDryRunOutcome, LlmRunnerError>;

    /// Run one LLM invocation end-to-end: enablement checks, concurrency
    /// permits, rate limiting, circuit breaker, size limits, spawn via
    /// `run_fn`, timeout, and audit record start/finish. `run_fn` receives
    /// the validated prompt and must return the raw model output (already
    /// bounded to `max_output_bytes` is NOT required of `run_fn` — the
    /// runner truncates the captured output itself before persisting/
    /// returning it).
    pub async fn run<F, Fut>(
        &self,
        spec: LlmInvocationSpec,
        run_fn: F,
    ) -> Result<LlmInvocationOutcome, LlmRunnerError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<String>> + Send + 'static;
}
```

Callers supply `caller_surface` via `LlmCallerSurface::{Cli,Mcp,Rest,Background,Test}`,
`action` as a plain lowercase snake_case string (`"ai_assess"`, `"skill_assess"`,
`"background_enrich"` are the three actions pre-declared in config defaults —
add new `[llm.actions.<name>]` rows for any new action), `incident_id` /
`ai_tool` / `ai_project` / `ai_session_id` as `Option<String>` filters, and
`evidence_counts` as an `LlmEvidenceCounts` built from whatever evidence
bundle they assembled. The Markdown/String result comes back as
`LlmInvocationOutcome.output`. All errors are typed `LlmRunnerError`
variants — match on them to decide user-facing messages; every variant
(including denials) has already caused an audit row to be written before
the error is returned.

Config type consumed by `LlmRunner::new`:

```rust
// src/config.rs — new section, added to Config as `pub llm: LlmConfig`
pub struct LlmConfig {
    pub enabled: bool,                          // default true
    pub max_concurrent: usize,                  // default 1
    pub max_per_action_concurrent: usize,       // default 1
    pub max_invocations_per_minute: u32,        // default 3
    pub max_invocations_per_hour: u32,          // default 30
    pub failure_threshold: u32,                 // default 3
    pub cooldown_secs: u64,                     // default 300
    pub timeout_secs: u64,                      // default 120
    pub max_prompt_bytes: usize,                // default 1_048_576
    pub max_output_bytes: usize,                // default 262_144
    pub background_enrichment_enabled: bool,    // default false
    pub actions: std::collections::HashMap<String, LlmActionConfig>, // [llm.actions.*]
}
pub struct LlmActionConfig {
    pub enabled: bool,
}
```

DB row type + query function other phases can read (not required for the
`run`/`dry_run` API but useful for building admin/report tooling):

```rust
// src/db/llm_invocations.rs
pub struct LlmInvocationRow {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub caller_surface: String,
    pub action: String,
    pub provider: String,
    pub model: Option<String>,
    pub program: Option<String>,
    pub incident_id: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub evidence_counts_json: Option<String>,
    pub prompt_bytes: Option<i64>,
    pub output_bytes: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub metadata_json: Option<String>,
}

pub fn list_llm_invocations(
    conn: &rusqlite::Connection,
    limit: i64,
    since: Option<&str>,
    action: Option<&str>,
    status: Option<&str>,
) -> rusqlite::Result<Vec<LlmInvocationRow>>;
```

---

## Task 1: `llm_invocations` schema migration (migration 37)

**Files:**
- Modify: `src/db/pool.rs:1814-1975` (insert new migration block directly after the Migration 36 block, before the orphaned-maintenance-job cleanup at line ~1977; bump `KNOWN_SCHEMA_VERSION` at line 42)
- Test: sidecar convention — `src/db/pool.rs` has `#[cfg(test)] #[path = "pool_tests.rs"] mod tests;` at the bottom; add the new test to `src/db/pool_tests.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (this is the first task).
- Produces: the `llm_invocations` table + 3 indexes exactly as specified below; `KNOWN_SCHEMA_VERSION == 37`. Task 4 (DB query layer) and Task 3 (`LlmRunner`) both write/read this table by name — they do not call any Rust function this task produces, only rely on the schema existing after `init_pool`/`run_migrations` runs.

- [ ] **Step 1: Write the failing test**

  Before writing anything, verify the migration number this task claims is still free — do not hardcode migration 37 on the assumption it is still the next number. Run:

  ```bash
  grep -n "KNOWN_SCHEMA_VERSION" src/db/pool.rs
  ```

  If `KNOWN_SCHEMA_VERSION` is still `36`, migration `37` (used throughout this task) is correct and no numbers below need adjusting. If it has moved (e.g. another migration landed on `main` since this plan was drafted), substitute the actual next free migration number everywhere `37` appears in this task (the `KNOWN_SCHEMA_VERSION` constant, the `migration_applied(&conn, 37)` check, the `schema_migrations` INSERT, the module doc comment migration count, and both test assertions below) before proceeding. This live check is also called out in this plan's PR-sequencing note above, since PR 2 independently claims a migration number and must re-verify at its own implementation time to avoid a collision.

  Confirm the current known version and that no `llm_invocations` table exists pre-migration. Add to `src/db/pool_tests.rs` (check the file first for its existing `use` imports and an existing `init_pool`-based test to match style — the test below assumes `use super::*;` is already present, which is the sidecar convention in this repo):

  ```rust
  #[test]
  fn migration_37_creates_llm_invocations_table() {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let pool = init_pool(&db_path, 2, 64, 64).expect("init_pool should succeed");
      let conn = pool.get().unwrap();

      // Table exists with the exact locked column set.
      let mut stmt = conn
          .prepare("SELECT COUNT(*) FROM pragma_table_info('llm_invocations') WHERE name IN (
              'id','started_at','finished_at','duration_ms','caller_surface','action',
              'provider','model','program','incident_id','ai_tool','ai_project',
              'ai_session_id','evidence_counts_json','prompt_bytes','output_bytes',
              'status','error','metadata_json'
          )")
          .unwrap();
      let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
      assert_eq!(count, 19, "llm_invocations must have all 19 locked columns");

      // Migration is recorded and idempotent.
      let version: i64 = conn
          .query_row(
              "SELECT COUNT(*) FROM schema_migrations WHERE version = 37",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(version, 1);

      // Re-running init_pool (simulating a restart) must not error or duplicate the row.
      drop(conn);
      drop(pool);
      let pool2 = init_pool(&db_path, 2, 64, 64).expect("second init_pool should succeed");
      let conn2 = pool2.get().unwrap();
      let version2: i64 = conn2
          .query_row(
              "SELECT COUNT(*) FROM schema_migrations WHERE version = 37",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(version2, 1, "migration 37 must be idempotent across restarts");
  }

  #[test]
  fn migration_37_indexes_exist() {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let pool = init_pool(&db_path, 2, 64, 64).expect("init_pool should succeed");
      let conn = pool.get().unwrap();
      for idx in [
          "idx_llm_invocations_started",
          "idx_llm_invocations_action_started",
          "idx_llm_invocations_status_started",
      ] {
          let count: i64 = conn
              .query_row(
                  "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                  [idx],
                  |row| row.get(0),
              )
              .unwrap();
          assert_eq!(count, 1, "expected index {idx} to exist");
      }
  }
  ```

  NOTE: check the exact signature of `init_pool` in `src/db/pool.rs` before pasting — if it takes a different parameter list (e.g. `(path, pool_size, page_cache_mb, mmap_mb)` vs named struct), adjust the call to match; do not guess blindly, `grep -n "pub fn init_pool" src/db/pool.rs` first.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib db::pool_tests::migration_37_creates_llm_invocations_table -- --nocapture`
  Expected: FAIL — compiles (since the test only queries `pragma_table_info`/`sqlite_master`, no new Rust symbols are referenced) but assertion `count == 19` fails because the table does not exist yet (`pragma_table_info` returns zero rows for a nonexistent table, so `count == 0`).

- [ ] **Step 3: Write minimal implementation**

  In `src/db/pool.rs`, change line 42:

  ```rust
  pub const KNOWN_SCHEMA_VERSION: i64 = 37;
  ```

  Then insert this new block immediately after the closing `}` of the Migration 36 block (after the line `tracing::info!("Migration 36: added user/device entities and identity relationships");` and its closing `}`, i.e. right before the `// A server crash/restart mid-check leaves an orphaned 'running' maintenance` comment currently at line 1977):

  ```rust
  // Migration 37: create llm_invocations, the shared audit table for every
  // LLM-backed assessment call (ai_assess today; skill_assess/mcp_assess/
  // hook_assess in later phases). A start row is written before the
  // process/API call begins (status='running') and updated on completion.
  // Concurrency/rate-limit/circuit-open/disabled denials also write a row
  // (status set to the denial reason) so the audit trail covers every call
  // attempt, not just ones that reached the LLM.
  if !migration_applied(&conn, 37)? {
      conn.execute_batch(
          "BEGIN IMMEDIATE;

           CREATE TABLE IF NOT EXISTS llm_invocations (
             id                 TEXT PRIMARY KEY,
             started_at         TEXT NOT NULL,
             finished_at        TEXT,
             duration_ms        INTEGER,
             caller_surface     TEXT NOT NULL,
             action             TEXT NOT NULL,
             provider           TEXT NOT NULL,
             model              TEXT,
             program            TEXT,
             incident_id        TEXT,
             ai_tool            TEXT,
             ai_project         TEXT,
             ai_session_id      TEXT,
             evidence_counts_json TEXT,
             prompt_bytes       INTEGER,
             output_bytes       INTEGER,
             status             TEXT NOT NULL,
             error              TEXT,
             metadata_json      TEXT
           );

           CREATE INDEX IF NOT EXISTS idx_llm_invocations_started
               ON llm_invocations(started_at);
           CREATE INDEX IF NOT EXISTS idx_llm_invocations_action_started
               ON llm_invocations(action, started_at);
           CREATE INDEX IF NOT EXISTS idx_llm_invocations_status_started
               ON llm_invocations(status, started_at);

           INSERT OR IGNORE INTO schema_migrations (version) VALUES (37);
           COMMIT;",
      )?;
      tracing::info!("Migration 37: created llm_invocations audit table");
  }
  ```

  Also update the module doc comment at the top of `src/db/pool.rs` (around line 5) that says `**31 sequential migrations**` — bump the count to match the new total (37) so the doc comment does not silently rot further out of date:

  ```rust
  //! projections, and the **37 sequential migrations** tracked by
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib db::pool_tests::migration_37_creates_llm_invocations_table db::pool_tests::migration_37_indexes_exist`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/db/pool.rs src/db/pool_tests.rs
  git commit -m "feat: add llm_invocations audit table (migration 37)"
  ```

---

## Task 2: `LlmConfig` — `[llm]` section, defaults, env overrides

**Files:**
- Modify: `src/config.rs:66-77` (add `pub llm: LlmConfig` field to `Config`), and append new structs/impls/env-override calls/validation function near the other config sections (after `ErrorDetectionConfig` at line ~211, and inside `load_inner` near line 930, and near `validate_error_detection_config` at line ~1593)
- Test: `src/config.rs` uses `#[cfg(test)] #[path = "config_tests.rs"] mod tests;` — check the actual sidecar filename with `grep -n '#\[path' src/config.rs` first; add tests there (assume `src/config_tests.rs` per repo convention, adjust if grep shows a different path)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub struct LlmConfig` and `pub struct LlmActionConfig` (exact fields listed in "Locked interfaces" above) as `Config.llm: LlmConfig`. Task 3 (`LlmRunner::new`) takes `crate::config::LlmConfig` by value. The env var `CORTEX_LLM_ENABLED` overriding `config.llm.enabled` is load-bearing for the "global kill switch" requirement — Task 3's tests rely on being able to construct an `LlmConfig` directly (not just via env), so keep all fields `pub`.

- [ ] **Step 1: Write the failing test**

  First run `grep -n '#\[path' src/config.rs` to find the exact sidecar test file name, then add to that file (assumed `src/config_tests.rs`):

  ```rust
  #[test]
  fn llm_config_defaults_match_locked_table() {
      let cfg = crate::config::LlmConfig::default();
      assert!(cfg.enabled);
      assert_eq!(cfg.max_concurrent, 1);
      assert_eq!(cfg.max_per_action_concurrent, 1);
      assert_eq!(cfg.max_invocations_per_minute, 3);
      assert_eq!(cfg.max_invocations_per_hour, 30);
      assert_eq!(cfg.failure_threshold, 3);
      assert_eq!(cfg.cooldown_secs, 300);
      assert_eq!(cfg.timeout_secs, 120);
      assert_eq!(cfg.max_prompt_bytes, 1_048_576);
      assert_eq!(cfg.max_output_bytes, 262_144);
      assert!(!cfg.background_enrichment_enabled);
      assert!(cfg.actions.is_empty(), "no actions configured by default until config.toml declares them");
  }

  #[test]
  fn llm_config_parses_from_toml_with_action_subtables() {
      let toml_str = r#"
          [llm]
          enabled = true
          max_concurrent = 2

          [llm.actions.ai_assess]
          enabled = true

          [llm.actions.background_enrich]
          enabled = false
      "#;
      let parsed: crate::config::Config = toml::from_str(toml_str).unwrap();
      assert_eq!(parsed.llm.max_concurrent, 2);
      // Fields not set in the [llm] table still take their defaults.
      assert_eq!(parsed.llm.max_invocations_per_minute, 3);
      assert!(parsed.llm.actions.get("ai_assess").unwrap().enabled);
      assert!(!parsed.llm.actions.get("background_enrich").unwrap().enabled);
  }

  #[test]
  fn cortex_llm_enabled_env_var_overrides_config() {
      // SAFETY: test runs single-threaded within cargo's per-test process
      // isolation is NOT guaranteed across threads; follow this repo's
      // existing pattern for env-var tests — check an existing
      // env_override_bool test in this file for the serialization guard
      // (e.g. a `Mutex` around env-mutating tests) and reuse it here rather
      // than introducing a new one.
      std::env::set_var("CORTEX_LLM_ENABLED", "false");
      let mut cfg = crate::config::LlmConfig::default();
      crate::config::env_override_bool("CORTEX_LLM_ENABLED", &mut cfg.enabled).unwrap();
      assert!(!cfg.enabled);
      std::env::remove_var("CORTEX_LLM_ENABLED");
  }
  ```

  NOTE: `env_override_bool` is currently a private `fn` in `src/config.rs` (no `pub`). Before writing the third test, check its visibility — if it is not `pub(crate)`, either mark it `pub(crate)` in Step 3 (needed anyway since the `load_inner` wiring call in Step 3 already uses it) or write the env-var test inside `config.rs`'s own inline `#[cfg(test)] mod tests` if one exists there instead of the sidecar. Prefer keeping `env_override_bool` `pub(crate)` since Task 3 does NOT need it (the runner reads `LlmConfig.enabled` only, already resolved by `Config::load()`), so no cross-module leakage risk.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib config_tests::llm_config_defaults_match_locked_table -- --nocapture`
  Expected: FAIL with a compile error — `crate::config::LlmConfig` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

  In `src/config.rs`, add `pub llm: LlmConfig` to the `Config` struct:

  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize, Default)]
  #[serde(default)]
  pub struct Config {
      pub receiver: ReceiverConfig,
      pub storage: StorageConfig,
      pub mcp: McpConfig,
      pub api: ApiConfig,
      pub docker_ingest: DockerIngestConfig,
      pub enrichment: EnrichmentConfigToml,
      pub error_detection: ErrorDetectionConfig,
      pub notifications: NotificationsConfig,
      pub llm: LlmConfig,
  }
  ```

  Add the new structs after `EnrichmentConfigToml`'s `impl Default` block (after line ~246):

  ```rust
  // ---------------------------------------------------------------------------
  // LLM invocation guard configuration
  //
  // Shared by every LLM-backed assessment feature (ai_assess today;
  // skill_assess / mcp_assess / hook_assess added by later phases). See
  // `src/app/llm_runner.rs` for the runtime enforcement of these limits.
  // Loaded from `[llm]` in `config.toml` or `CORTEX_LLM_*` env vars.
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(default)]
  pub struct LlmConfig {
      /// Global kill switch. When false, every `LlmRunner::run` call is
      /// denied immediately (still audited with status "disabled").
      /// Default: true. Env override: `CORTEX_LLM_ENABLED`.
      pub enabled: bool,
      /// Max invocations running concurrently across all actions.
      /// Default: 1.
      pub max_concurrent: usize,
      /// Max invocations running concurrently for a single action.
      /// Default: 1.
      pub max_per_action_concurrent: usize,
      /// Max invocations per action per rolling 60s window. Default: 3.
      pub max_invocations_per_minute: u32,
      /// Max invocations per action per rolling 3600s window. Default: 30.
      pub max_invocations_per_hour: u32,
      /// Consecutive failures/timeouts for an action before its circuit
      /// opens. Default: 3.
      pub failure_threshold: u32,
      /// How long an open circuit stays open before allowing another
      /// attempt (seconds). Default: 300.
      pub cooldown_secs: u64,
      /// Per-invocation timeout (seconds). Default: 120. Mirrors the
      /// pre-existing `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` env var read by
      /// `GeminiAssessConfig::from_env` in `src/assessment.rs` — Task 5
      /// wires this value through instead of that struct re-reading the
      /// env var independently.
      pub timeout_secs: u64,
      /// Max prompt+evidence size in bytes. Requests over this are rejected
      /// before spawning any process. Default: 1_048_576 (1 MiB).
      pub max_prompt_bytes: usize,
      /// Max captured output size in bytes; output beyond this is
      /// truncated. Default: 262_144 (256 KiB).
      pub max_output_bytes: usize,
      /// Whether ANY background (non-interactive, non-CLI/MCP/REST-request)
      /// code path may invoke an LLM. Default: false. There must be no code
      /// path that runs LLM calls in the background without this being
      /// explicitly true — `LlmRunner::run` checks this whenever
      /// `caller_surface == Background`.
      pub background_enrichment_enabled: bool,
      /// Per-action enablement, keyed by action name (e.g. "ai_assess").
      /// An action with no entry here is treated as enabled=true UNLESS
      /// its name is "background_enrich", which defaults to disabled via
      /// `background_enrichment_enabled` regardless of this map.
      pub actions: std::collections::HashMap<String, LlmActionConfig>,
  }

  impl Default for LlmConfig {
      fn default() -> Self {
          Self {
              enabled: true,
              max_concurrent: 1,
              max_per_action_concurrent: 1,
              max_invocations_per_minute: 3,
              max_invocations_per_hour: 30,
              failure_threshold: 3,
              cooldown_secs: 300,
              timeout_secs: 120,
              max_prompt_bytes: 1_048_576,
              max_output_bytes: 262_144,
              background_enrichment_enabled: false,
              actions: std::collections::HashMap::new(),
          }
      }
  }

  /// Per-action `[llm.actions.<name>]` toggle.
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(default)]
  pub struct LlmActionConfig {
      pub enabled: bool,
  }

  impl Default for LlmActionConfig {
      fn default() -> Self {
          Self { enabled: true }
      }
  }
  ```

  Wire the env override in `load_inner`, immediately after the existing `env_override_parse("CORTEX_ERR_FLOOR_PER_SOURCE_CAP", ...)` block (around line 930, right before the `// [mcp.auth] env overrides.` comment):

  ```rust
  // [llm] env overrides.
  env_override_bool("CORTEX_LLM_ENABLED", &mut config.llm.enabled)?;
  ```

  Mark `env_override_bool` `pub(crate)` (it is currently private `fn`; only visibility changes, no behavior change):

  ```rust
  pub(crate) fn env_override_bool(key: &str, target: &mut bool) -> anyhow::Result<()> {
  ```

  Add validation, following the existing `validate_error_detection_config` pattern, and call it from `load_inner` next to the other `validate_*` calls (around line 1079, after `validate_error_detection_config(&config.error_detection)?;`):

  ```rust
  validate_llm_config(&config.llm)?;
  ```

  ```rust
  fn validate_llm_config(cfg: &LlmConfig) -> anyhow::Result<()> {
      if cfg.max_concurrent == 0 {
          anyhow::bail!("[llm] max_concurrent must be > 0");
      }
      if cfg.max_per_action_concurrent == 0 {
          anyhow::bail!("[llm] max_per_action_concurrent must be > 0");
      }
      if cfg.timeout_secs == 0 {
          anyhow::bail!("[llm] timeout_secs must be > 0");
      }
      if cfg.max_prompt_bytes == 0 {
          anyhow::bail!("[llm] max_prompt_bytes must be > 0");
      }
      if cfg.max_output_bytes == 0 {
          anyhow::bail!("[llm] max_output_bytes must be > 0");
      }
      Ok(())
  }
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib config_tests::llm_config_defaults_match_locked_table config_tests::llm_config_parses_from_toml_with_action_subtables config_tests::cortex_llm_enabled_env_var_overrides_config`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/config.rs src/config_tests.rs
  git commit -m "feat: add [llm] config section with per-action enablement"
  ```

---

## Task 3: `LlmRunner` core — concurrency, rate limit, circuit breaker, kill switch (no process spawning yet)

**Files:**
- Create: `src/app/llm_runner.rs`
- Test: `src/app/llm_runner_tests.rs` (sidecar convention: add `#[cfg(test)] #[path = "llm_runner_tests.rs"] mod tests;` at the bottom of `src/app/llm_runner.rs`)
- Modify: `src/app.rs` (or wherever `mod` declarations for `src/app/*.rs` live — run `grep -n '^mod \|^pub mod' src/app.rs` first) to add `pub mod llm_runner;` (or `mod llm_runner;` + targeted `pub use` — match whatever visibility pattern the file already uses for sibling modules like `error`)

**Interfaces:**
- Consumes: `crate::config::LlmConfig` / `crate::config::LlmActionConfig` (Task 2), `crate::db::DbPool` (existing), `crate::db::write_lock()` (existing, `src/db/pool.rs` — every audit write in this task acquires it, matching the standing single-writer invariant; see Fix 2 below), `crate::assessment::redact_secrets` (existing, widened to `pub(crate)` as this task's own first Step 3 edit — see Fix 3 below), and (for the audit write) the `insert_llm_invocation_running` / `finish_llm_invocation` functions produced by Task 4 — HOWEVER, to keep Task 3 and Task 4 independently testable, Task 3 stubs its own minimal audit writer inline in this task (a private `fn write_audit_row` using raw `rusqlite::Connection::execute`, itself guarded by `write_lock()`) and Task 4 later replaces that internal call with the shared `src/db/llm_invocations.rs` functions (which preserve the same `write_lock()` guard). This avoids a circular task dependency. Task 6 (migrate `ai assess`) is the one that actually calls `LlmRunner::run` end-to-end with a real process spawn.
- Produces: `pub struct LlmRunner`, `pub enum LlmCallerSurface`, `pub struct LlmEvidenceCounts`, `pub struct LlmInvocationSpec`, `pub struct LlmInvocationOutcome`, `pub struct LlmDryRunOutcome`, `pub enum LlmRunnerError`, `const LLM_METADATA_MAX_BYTES: usize` — the exact API in "Locked interfaces for other phases" above. `LlmRunner::run`'s `run_fn` closure type signature (`FnOnce(String) -> Fut + Send + 'static` where `Fut: Future<Output = anyhow::Result<String>> + Send + 'static`) is exactly what Task 5 (process-spawn wiring) and Task 6 (migration) close over. `extra_metadata` on `LlmInvocationSpec` is redacted and size-capped before persisting (see Fix 5 below) — this is a behavior guarantee downstream tasks (PR 2-4) may rely on but must NOT treat as license to pass bulk content through it (see the contract note on `extra_metadata` above).

  **Eng review fixes applied in this task (see "Eng Review Fixes Applied" section near the top of this plan for full detail):**
  - **Fix 2 (CRITICAL — architecture + performance reviewers):** `write_start_row` and `write_finish_row_inner` now acquire `crate::db::write_lock()` immediately before their `conn.execute(...)` call, matching every other writer in the repo.
  - **Fix 3 (security reviewer, MP1):** `sanitize_error` now calls the real `crate::assessment::redact_secrets` (widened to `pub(crate)`) instead of a forked, weaker heuristic, and redacts BEFORE truncating to 2048 chars (previously truncated first, which could split a secret across the boundary).
  - **Fix 5 (security reviewer, MP3):** `build_metadata_json` now runs the merged metadata object through `redact_secrets` and enforces `LLM_METADATA_MAX_BYTES` (4096 bytes), truncating with an explicit `"truncated": true` marker rather than silently storing an oversized or secret-bearing blob.

- [ ] **Step 1: Write the failing test**

  ```rust
  // src/app/llm_runner_tests.rs
  use super::*;
  use crate::config::LlmConfig;
  use std::sync::Arc;

  fn test_pool() -> Arc<crate::db::DbPool> {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      // Leak the tempdir so the pool's backing file survives the test body;
      // acceptable in test code, matches existing test_service() patterns —
      // grep `fn test_service` in src/app/service_tests.rs for the exact
      // idiom this repo uses to keep a TempDir alive alongside a DbPool.
      let pool = crate::db::init_pool(&db_path, 2, 64, 64).unwrap();
      std::mem::forget(dir);
      Arc::new(pool)
  }

  fn base_spec(action: &str) -> LlmInvocationSpec {
      LlmInvocationSpec {
          caller_surface: LlmCallerSurface::Test,
          action: action.to_string(),
          incident_id: Some("inc-1".to_string()),
          ai_tool: None,
          ai_project: None,
          ai_session_id: None,
          evidence_counts: LlmEvidenceCounts::default(),
          prompt: "hello".to_string(),
          provider: "test-provider".to_string(),
          model: "test-model".to_string(),
          program: "test-program".to_string(),
          extra_metadata: serde_json::json!({}),
      }
  }

  // Eng review fix (Fix 1 — architecture + performance reviewers): the
  // `timeout_secs()` accessor is the single source of truth Task 6 threads
  // into `GeminiAssessConfig` instead of that struct independently
  // re-reading `CORTEX_LLM_COMPLETION_TIMEOUT_SECS`. See "Eng Review Fixes
  // Applied" at the top of this plan.
  #[test]
  fn timeout_secs_accessor_exposes_resolved_config_value() {
      let mut cfg = LlmConfig::default();
      cfg.timeout_secs = 45;
      let pool = test_pool();
      let runner = LlmRunner::new(pool, cfg);
      assert_eq!(runner.timeout_secs(), 45);
  }

  #[tokio::test]
  async fn disabled_runner_denies_and_audits() {
      let pool = test_pool();
      let mut cfg = LlmConfig::default();
      cfg.enabled = false;
      let runner = LlmRunner::new(pool.clone(), cfg);

      let result = runner
          .run(base_spec("ai_assess"), |_prompt| async { Ok("unused".to_string()) })
          .await;

      assert!(matches!(result, Err(LlmRunnerError::Disabled)));

      let conn = pool.get().unwrap();
      let status: String = conn
          .query_row(
              "SELECT status FROM llm_invocations WHERE action = 'ai_assess' ORDER BY started_at DESC LIMIT 1",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(status, "disabled");
  }

  #[tokio::test]
  async fn prompt_over_limit_is_rejected_before_spawn() {
      let pool = test_pool();
      let mut cfg = LlmConfig::default();
      cfg.max_prompt_bytes = 4;
      let runner = LlmRunner::new(pool.clone(), cfg);

      let mut spec = base_spec("ai_assess");
      spec.prompt = "way too long".to_string();

      let result = runner
          .run(spec, |_prompt| async { panic!("run_fn must not be called when prompt exceeds limit") })
          .await;

      assert!(matches!(
          result,
          Err(LlmRunnerError::PromptTooLarge { actual: 12, limit: 4 })
      ));
  }

  #[tokio::test]
  async fn global_concurrency_limit_denies_second_concurrent_call() {
      let pool = test_pool();
      let mut cfg = LlmConfig::default();
      cfg.max_concurrent = 1;
      cfg.max_per_action_concurrent = 5; // isolate global limit from per-action limit
      let runner = Arc::new(LlmRunner::new(pool.clone(), cfg));

      let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
      let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
      let runner_a = runner.clone();
      let handle = tokio::spawn(async move {
          runner_a
              .run(base_spec("ai_assess"), move |_prompt| async move {
                  started_tx.send(()).ok();
                  release_rx.await.ok();
                  Ok("first".to_string())
              })
              .await
      });
      started_rx.await.unwrap();

      let second = runner
          .run(base_spec("skill_assess"), |_prompt| async { Ok("second".to_string()) })
          .await;
      assert!(matches!(second, Err(LlmRunnerError::ConcurrencyLimited(1))));

      release_tx.send(()).ok();
      let first = handle.await.unwrap();
      assert!(first.is_ok());
  }

  #[tokio::test]
  async fn rate_limit_denies_fourth_call_within_a_minute() {
      let pool = test_pool();
      let mut cfg = LlmConfig::default();
      cfg.max_invocations_per_minute = 3;
      cfg.max_invocations_per_hour = 1000;
      let runner = LlmRunner::new(pool.clone(), cfg);

      for _ in 0..3 {
          let result = runner
              .run(base_spec("ai_assess"), |_prompt| async { Ok("ok".to_string()) })
              .await;
          assert!(result.is_ok());
      }

      let fourth = runner
          .run(base_spec("ai_assess"), |_prompt| async { Ok("ok".to_string()) })
          .await;
      assert!(matches!(fourth, Err(LlmRunnerError::RateLimited { .. })));
  }

  #[tokio::test]
  async fn circuit_opens_after_failure_threshold_and_audits_denial() {
      let pool = test_pool();
      let mut cfg = LlmConfig::default();
      cfg.failure_threshold = 2;
      cfg.max_invocations_per_minute = 100;
      cfg.max_invocations_per_hour = 100;
      let runner = LlmRunner::new(pool.clone(), cfg);

      for _ in 0..2 {
          let result = runner
              .run(base_spec("ai_assess"), |_prompt| async {
                  Err(anyhow::anyhow!("simulated LLM failure"))
              })
              .await;
          assert!(result.is_err());
      }

      let third = runner
          .run(base_spec("ai_assess"), |_prompt| async {
              panic!("run_fn must not be called while circuit is open")
          })
          .await;
      assert!(matches!(third, Err(LlmRunnerError::CircuitOpen { .. })));

      let conn = pool.get().unwrap();
      let denied_count: i64 = conn
          .query_row(
              "SELECT COUNT(*) FROM llm_invocations WHERE action = 'ai_assess' AND status = 'circuit_open'",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(denied_count, 1, "circuit_open denial must itself be audited");
  }

  #[tokio::test]
  async fn dry_run_never_invokes_llm_and_reports_sizes() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());
      let spec = base_spec("ai_assess");

      let outcome = runner.dry_run(&spec).await.unwrap();
      assert_eq!(outcome.prompt_bytes, "hello".len());
      assert!(!outcome.would_exceed_prompt_limit);

      let conn = pool.get().unwrap();
      let status: String = conn
          .query_row(
              "SELECT status FROM llm_invocations WHERE id = ?1",
              [outcome.invocation_id],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(status, "dry_run");
  }

  #[tokio::test]
  async fn successful_run_writes_success_audit_row_with_timing() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());

      let outcome = runner
          .run(base_spec("ai_assess"), |prompt| async move {
              assert_eq!(prompt, "hello");
              Ok("assessment markdown".to_string())
          })
          .await
          .unwrap();

      assert_eq!(outcome.output, "assessment markdown");
      assert!(outcome.duration_ms >= 0);

      let conn = pool.get().unwrap();
      let (status, finished_at, incident_id): (String, Option<String>, Option<String>) = conn
          .query_row(
              "SELECT status, finished_at, incident_id FROM llm_invocations WHERE id = ?1",
              [outcome.invocation_id],
              |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
          )
          .unwrap();
      assert_eq!(status, "success");
      assert!(finished_at.is_some());
      assert_eq!(incident_id.as_deref(), Some("inc-1"));
  }

  // --- Eng review fix: MP1 (security reviewer) — sanitize_error must catch
  // the same secret shapes `looks_secretish` catches elsewhere in this repo,
  // and must redact BEFORE truncating so a secret straddling the truncation
  // boundary can't leak its surviving half. See "Eng Review Fixes Applied"
  // (Fix 3) at the top of this plan.
  #[tokio::test]
  async fn error_with_secretish_tokens_is_redacted_before_persisting() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());

      let result = runner
          .run(base_spec("ai_assess"), |_prompt| async {
              Err(anyhow::anyhow!(
                  "gemini auth failed: API_KEY=super-secret-value sk-abc123secretvalue ghp_deadbeef1234 TOKEN=another-one"
              ))
          })
          .await;
      assert!(result.is_err());

      let conn = pool.get().unwrap();
      let error: String = conn
          .query_row(
              "SELECT error FROM llm_invocations WHERE action = 'ai_assess' AND status = 'error' ORDER BY started_at DESC LIMIT 1",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert!(
          !error.contains("super-secret-value"),
          "API_KEY value must be redacted, got: {error}"
      );
      assert!(
          !error.contains("sk-abc123secretvalue"),
          "sk- prefixed token must be redacted, got: {error}"
      );
      assert!(
          !error.contains("ghp_deadbeef1234"),
          "ghp_ prefixed token must be redacted, got: {error}"
      );
      assert!(
          !error.contains("another-one"),
          "TOKEN value must be redacted, got: {error}"
      );
      assert!(
          error.contains("[REDACTED]"),
          "expected [REDACTED] marker in sanitized error, got: {error}"
      );
  }

  #[test]
  fn sanitize_error_redacts_before_truncating_so_boundary_split_secrets_do_not_leak() {
      // Build an error whose secret token straddles the old 2048-char
      // truncation boundary: 2040 bytes of padding, then a secret that
      // would previously be split mid-token by `.take(2048)` BEFORE
      // redaction ran, leaking the surviving half. Redact-first must catch
      // the whole token regardless of where it falls.
      let padding = "x".repeat(2040);
      let err = anyhow::anyhow!(format!("{padding} API_KEY=leaked-secret-value-should-not-appear"));
      let sanitized = sanitize_error(&err);
      assert!(
          !sanitized.contains("leaked-secret-value-should-not-appear"),
          "secret must not survive truncation in either half, got: {sanitized}"
      );
  }

  // --- Eng review fix: MP3 (security reviewer) — extra_metadata must be
  // redacted and size-capped before it lands in metadata_json, since it is
  // caller-supplied and PR 2-4 must not be able to smuggle prompt/evidence
  // content into an (admin-scoped, per Fix 4) but still-persisted table.
  // See "Eng Review Fixes Applied" (Fix 5).
  #[tokio::test]
  async fn extra_metadata_with_secretish_value_is_redacted_before_persisting() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());

      let mut spec = base_spec("ai_assess");
      spec.extra_metadata = serde_json::json!({
          "note": "leaked TOKEN=do-not-persist-this-value here"
      });

      let outcome = runner
          .run(spec, |_prompt| async { Ok("ok".to_string()) })
          .await
          .unwrap();

      let conn = pool.get().unwrap();
      let metadata_json: String = conn
          .query_row(
              "SELECT metadata_json FROM llm_invocations WHERE id = ?1",
              [outcome.invocation_id],
              |row| row.get(0),
          )
          .unwrap();
      assert!(
          !metadata_json.contains("do-not-persist-this-value"),
          "secret-shaped extra_metadata value must be redacted, got: {metadata_json}"
      );
      assert!(metadata_json.contains("[REDACTED]"));
  }

  #[tokio::test]
  async fn extra_metadata_over_byte_cap_is_truncated_not_silently_dropped() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());

      let mut spec = base_spec("ai_assess");
      // Comfortably over LLM_METADATA_MAX_BYTES (4096).
      spec.extra_metadata = serde_json::json!({ "blob": "y".repeat(8192) });

      let outcome = runner
          .run(spec, |_prompt| async { Ok("ok".to_string()) })
          .await
          .unwrap();

      let conn = pool.get().unwrap();
      let metadata_json: String = conn
          .query_row(
              "SELECT metadata_json FROM llm_invocations WHERE id = ?1",
              [outcome.invocation_id],
              |row| row.get(0),
          )
          .unwrap();
      assert!(
          metadata_json.len() <= LLM_METADATA_MAX_BYTES + 256,
          "metadata_json must be capped near LLM_METADATA_MAX_BYTES, got {} bytes",
          metadata_json.len()
      );
      assert!(
          metadata_json.contains("truncated"),
          "oversized metadata must carry an explicit truncation marker, not be silently stored in full; got: {metadata_json}"
      );
  }

  // --- Eng review fix: CRITICAL (architecture + performance reviewers) —
  // every audit write must go through `crate::db::write_lock()`, matching
  // the standing invariant documented in `src/db/pool.rs`. This test
  // doesn't (and can't, from a black-box unit test) prove serialization by
  // itself, but it exercises a write concurrently with another
  // `write_lock()`-guarded writer (`crate::db::ingest::insert_logs_batch`)
  // and asserts both complete without a `database is locked` error — the
  // observable symptom the invariant exists to prevent. See "Eng Review
  // Fixes Applied" (Fix 2).
  #[tokio::test]
  async fn audit_write_does_not_race_a_concurrent_write_locked_writer() {
      let pool = test_pool();
      let runner = LlmRunner::new(pool.clone(), LlmConfig::default());

      fn test_log_entry(n: usize) -> crate::db::LogBatchEntry {
          crate::db::LogBatchEntry {
              timestamp: chrono::Utc::now().to_rfc3339(),
              hostname: "llm-runner-lock-test".to_string(),
              facility: None,
              severity: "info".to_string(),
              app_name: None,
              process_id: None,
              message: format!("write_lock contention probe {n}"),
              raw: format!("write_lock contention probe {n}"),
              source_ip: "127.0.0.1:514".to_string(),
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
          }
      }

      let pool_for_ingest = pool.clone();
      let ingest_handle = tokio::task::spawn_blocking(move || {
          for n in 0..20 {
              crate::db::insert_logs_batch(&pool_for_ingest, &[test_log_entry(n)])
                  .expect("concurrent syslog batch insert must not fail under write_lock contention");
          }
      });

      for _ in 0..20 {
          runner
              .run(base_spec("ai_assess"), |_p| async { Ok("ok".to_string()) })
              .await
              .expect("concurrent audited LLM run must not fail under write_lock contention");
      }

      ingest_handle.await.unwrap();
  }
  ```

  NOTE: `crate::db::LogBatchEntry` field list must exactly match the real struct in `src/db/models.rs` — grep `grep -n "pub struct LogBatchEntry" -A 25 src/db/models.rs` first and adjust field names/types if they've drifted since this plan was drafted (the field list above was copied from the repo's own `src/db/ingest_tests.rs::make_entry` helper at plan-fix time). `crate::db::insert_logs_batch` is already `pub fn insert_logs_batch(pool: &DbPool, entries: &[LogBatchEntry]) -> Result<usize>` re-exported at the `db` module root (`src/db/ingest.rs:10`) — no new visibility change needed.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib app::llm_runner_tests:: -- --nocapture`
  Expected: FAIL with a compile error — `src/app/llm_runner.rs` does not exist yet, so `crate::app::llm_runner::{LlmRunner, ...}` is unresolved. (After Task 3's implementation lands, the new redaction/metadata/write-lock tests added here by the eng-review fixes are also expected to fail first against the ORIGINAL Step 3 code below before the fixed code block is applied — this task's Step 3 already contains the fixed code, so implementers following this plan top-to-bottom will see them go straight to PASS in Step 4.)

- [ ] **Step 3: Write minimal implementation**

  **Eng review fix prerequisite (Fix 3 — security reviewer V1/V4):** before
  writing `src/app/llm_runner.rs`, widen visibility of the two existing
  redaction helpers in `src/assessment.rs` from private to `pub(crate)` so
  `llm_runner.rs` can reuse the real, stronger heuristic instead of forking
  a weaker one. This is a visibility-only change — no behavior change to
  `src/assessment.rs` itself:

  ```rust
  // src/assessment.rs — change these two functions from private `fn` to
  // `pub(crate) fn` (no other change; call sites within assessment.rs are
  // unaffected since pub(crate) is still visible to the whole crate
  // including this module):

  pub(crate) fn redact_secrets(text: &str) -> String {
      text.split_whitespace()
          .map(|token| {
              if looks_secretish(token) {
                  "[REDACTED]"
              } else {
                  token
              }
          })
          .collect::<Vec<_>>()
          .join(" ")
  }

  pub(crate) fn looks_secretish(token: &str) -> bool {
      let upper = token.to_ascii_uppercase();
      upper.contains("API_KEY=")
          || upper.contains("TOKEN=")
          || upper.contains("SECRET=")
          || token.starts_with("sk-")
          || token.starts_with("ghp_")
          || token.starts_with("atk_")
  }
  ```

  `llm_runner.rs` calls `crate::assessment::redact_secrets` directly rather
  than forking its own weaker copy — this keeps exactly one redaction
  heuristic in the crate, so a future addition to `looks_secretish` (e.g. a
  new secret prefix) automatically covers both the Gemini-stderr redaction
  path and the `llm_invocations` audit path.

  ```rust
  // src/app/llm_runner.rs
  //! Shared LLM invocation guard: every LLM-backed assessment feature routes
  //! through `LlmRunner::run` (or `LlmRunner::dry_run` for a preview) so
  //! concurrency limits, rate limits, circuit breaking, timeouts, size
  //! limits, and audit logging are enforced in exactly one place.
  //!
  //! See `llm_invocations` (migration 37, `src/db/pool.rs`) for the audit
  //! schema and `[llm]` in `src/config.rs` for the tunables.
  //!
  //! Every audit write acquires `crate::db::write_lock()` before touching
  //! the connection, matching the standing single-writer invariant
  //! documented at the top of `src/db/pool.rs` — see the `write_start_row`/
  //! `write_finish_row_inner` bodies below.

  use std::collections::HashMap;
  use std::sync::Arc;
  use std::time::{Duration, Instant};

  use tokio::sync::{Mutex, Semaphore};

  use crate::assessment::redact_secrets;
  use crate::config::LlmConfig;
  use crate::db::DbPool;

  /// Hard cap on the serialized size of `metadata_json`, in bytes. Distinct
  /// from (and much smaller than) `LlmConfig.max_output_bytes` — metadata is
  /// for small structured tags only, never prompt/evidence content (see the
  /// `extra_metadata` contract note in "Locked interfaces for other phases"
  /// at the top of this plan). Oversized metadata is truncated with an
  /// explicit `"truncated": true` marker, never silently stored in full.
  const LLM_METADATA_MAX_BYTES: usize = 4096;

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum LlmCallerSurface {
      Cli,
      Mcp,
      Rest,
      Background,
      Test,
  }

  impl LlmCallerSurface {
      pub fn as_str(self) -> &'static str {
          match self {
              Self::Cli => "cli",
              Self::Mcp => "mcp",
              Self::Rest => "rest",
              Self::Background => "background",
              Self::Test => "test",
          }
      }
  }

  #[derive(Debug, Clone, Default, serde::Serialize)]
  pub struct LlmEvidenceCounts {
      pub total_incidents: usize,
      pub evidence_bundle_count: usize,
      pub total_anchors: usize,
      pub truncated: bool,
  }

  #[derive(Debug, Clone)]
  pub struct LlmInvocationSpec {
      pub caller_surface: LlmCallerSurface,
      pub action: String,
      pub incident_id: Option<String>,
      pub ai_tool: Option<String>,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub evidence_counts: LlmEvidenceCounts,
      pub prompt: String,
      pub provider: String,
      pub model: String,
      pub program: String,
      pub extra_metadata: serde_json::Value,
  }

  #[derive(Debug, Clone)]
  pub struct LlmInvocationOutcome {
      pub invocation_id: String,
      pub output: String,
      pub duration_ms: i64,
      pub output_bytes: usize,
  }

  #[derive(Debug, Clone, serde::Serialize)]
  pub struct LlmDryRunOutcome {
      pub invocation_id: String,
      pub prompt_bytes: usize,
      pub evidence_counts: LlmEvidenceCounts,
      pub would_exceed_prompt_limit: bool,
  }

  #[derive(Debug, thiserror::Error)]
  pub enum LlmRunnerError {
      #[error(
          "LLM invocations are globally disabled ([llm].enabled=false or CORTEX_LLM_ENABLED=false)"
      )]
      Disabled,
      #[error("LLM action '{0}' is disabled ([llm.actions.{0}].enabled=false)")]
      ActionDisabled(String),
      #[error("prompt size {actual} bytes exceeds max_prompt_bytes {limit} bytes")]
      PromptTooLarge { actual: usize, limit: usize },
      #[error("global concurrency limit reached (max_concurrent={0})")]
      ConcurrencyLimited(usize),
      #[error(
          "per-action concurrency limit reached for '{action}' (max_per_action_concurrent={limit})"
      )]
      ActionConcurrencyLimited { action: String, limit: usize },
      #[error("rate limit exceeded for action '{action}': {detail}")]
      RateLimited { action: String, detail: String },
      #[error("circuit open for action '{action}' until {retry_after}")]
      CircuitOpen { action: String, retry_after: String },
      #[error("LLM invocation '{0}' timed out after {1}s")]
      Timeout(String, u64),
      #[error("LLM invocation '{0}' was cancelled")]
      Cancelled(String),
      #[error(transparent)]
      Internal(#[from] anyhow::Error),
  }

  /// Per-action sliding-window rate + failure-streak state.
  #[derive(Default)]
  struct ActionState {
      /// Timestamps (monotonic `Instant`) of invocations started in the
      /// last hour; pruned lazily on each check.
      recent_starts: Vec<Instant>,
      consecutive_failures: u32,
      /// Set when the circuit opens; cleared on the next successful call
      /// after `circuit_open_until` has elapsed.
      circuit_open_until: Option<Instant>,
      /// Per-action concurrency permits, created lazily per action name.
      permits: Option<Arc<Semaphore>>,
  }

  pub struct LlmRunner {
      pool: Arc<DbPool>,
      config: LlmConfig,
      global_permits: Arc<Semaphore>,
      action_state: Mutex<HashMap<String, ActionState>>,
  }

  impl LlmRunner {
      pub fn new(pool: Arc<DbPool>, config: LlmConfig) -> Self {
          let global_permits = Arc::new(Semaphore::new(config.max_concurrent));
          Self {
              pool,
              config,
              global_permits,
              action_state: Mutex::new(HashMap::new()),
          }
      }

      fn action_enabled(&self, action: &str) -> bool {
          if action == "background_enrich" && !self.config.background_enrichment_enabled {
              return false;
          }
          self.config
              .actions
              .get(action)
              .map(|a| a.enabled)
              .unwrap_or(true)
      }

      /// The resolved `[llm].timeout_secs` this runner enforces as its own
      /// outer per-invocation timeout. Eng review fix (Fix 1): exposed so
      /// callers that also need an inner, subprocess-level timeout (Task 6's
      /// `GeminiAssessConfig`) can read the SAME value instead of resolving
      /// their own from a separate env var — see "Eng Review Fixes Applied"
      /// at the top of this plan.
      pub fn timeout_secs(&self) -> u64 {
          self.config.timeout_secs
      }

      pub async fn dry_run(&self, spec: &LlmInvocationSpec) -> Result<LlmDryRunOutcome, LlmRunnerError> {
          let prompt_bytes = spec.prompt.len();
          let would_exceed = prompt_bytes > self.config.max_prompt_bytes;
          let invocation_id = new_invocation_id();
          self.write_start_row(&invocation_id, spec, "dry_run", prompt_bytes)
              .await?;
          self.write_finish_row(&invocation_id, "dry_run", None, 0)
              .await?;
          Ok(LlmDryRunOutcome {
              invocation_id,
              prompt_bytes,
              evidence_counts: spec.evidence_counts.clone(),
              would_exceed_prompt_limit: would_exceed,
          })
      }

      pub async fn run<F, Fut>(
          &self,
          spec: LlmInvocationSpec,
          run_fn: F,
      ) -> Result<LlmInvocationOutcome, LlmRunnerError>
      where
          F: FnOnce(String) -> Fut + Send + 'static,
          Fut: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
      {
          let action = spec.action.clone();
          let prompt_bytes = spec.prompt.len();

          // 1. Global kill switch.
          if !self.config.enabled {
              let id = new_invocation_id();
              self.write_start_row(&id, &spec, "disabled", prompt_bytes)
                  .await?;
              self.write_finish_row(&id, "disabled", None, 0).await?;
              return Err(LlmRunnerError::Disabled);
          }

          // 2. Per-action enablement (covers background_enrichment_enabled too).
          if !self.action_enabled(&action) {
              let id = new_invocation_id();
              self.write_start_row(&id, &spec, "disabled", prompt_bytes)
                  .await?;
              self.write_finish_row(&id, "disabled", None, 0).await?;
              return Err(LlmRunnerError::ActionDisabled(action));
          }

          // 3. Size limit — reject before spawning anything.
          if prompt_bytes > self.config.max_prompt_bytes {
              let id = new_invocation_id();
              self.write_start_row(&id, &spec, "error", prompt_bytes)
                  .await?;
              self.write_finish_row(
                  &id,
                  "error",
                  Some("prompt_too_large"),
                  0,
              )
              .await?;
              return Err(LlmRunnerError::PromptTooLarge {
                  actual: prompt_bytes,
                  limit: self.config.max_prompt_bytes,
              });
          }

          // 4. Circuit breaker.
          let action_permits = {
              let mut state = self.action_state.lock().await;
              let entry = state.entry(action.clone()).or_default();
              if let Some(open_until) = entry.circuit_open_until {
                  if Instant::now() < open_until {
                      drop(state);
                      let id = new_invocation_id();
                      self.write_start_row(&id, &spec, "circuit_open", prompt_bytes)
                          .await?;
                      self.write_finish_row(&id, "circuit_open", Some("circuit_open"), 0)
                          .await?;
                      return Err(LlmRunnerError::CircuitOpen {
                          action: action.clone(),
                          retry_after: format!("{:?}", open_until),
                      });
                  }
                  entry.circuit_open_until = None;
              }

              // 5. Rate limit (sliding window over recent_starts).
              let now = Instant::now();
              entry
                  .recent_starts
                  .retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
              let last_minute = entry
                  .recent_starts
                  .iter()
                  .filter(|t| now.duration_since(**t) < Duration::from_secs(60))
                  .count() as u32;
              if last_minute >= self.config.max_invocations_per_minute {
                  drop(state);
                  let id = new_invocation_id();
                  self.write_start_row(&id, &spec, "rate_limited", prompt_bytes)
                      .await?;
                  self.write_finish_row(&id, "rate_limited", Some("rate_limited_per_minute"), 0)
                      .await?;
                  return Err(LlmRunnerError::RateLimited {
                      action: action.clone(),
                      detail: format!(
                          "{last_minute}/{} invocations in the last minute",
                          self.config.max_invocations_per_minute
                      ),
                  });
              }
              let last_hour = entry.recent_starts.len() as u32;
              if last_hour >= self.config.max_invocations_per_hour {
                  drop(state);
                  let id = new_invocation_id();
                  self.write_start_row(&id, &spec, "rate_limited", prompt_bytes)
                      .await?;
                  self.write_finish_row(&id, "rate_limited", Some("rate_limited_per_hour"), 0)
                      .await?;
                  return Err(LlmRunnerError::RateLimited {
                      action: action.clone(),
                      detail: format!(
                          "{last_hour}/{} invocations in the last hour",
                          self.config.max_invocations_per_hour
                      ),
                  });
              }
              entry.recent_starts.push(now);

              let permits = entry
                  .permits
                  .get_or_insert_with(|| {
                      Arc::new(Semaphore::new(self.config.max_per_action_concurrent))
                  })
                  .clone();
              permits
          };

          // 6. Concurrency permits (global, then per-action). Acquire
          //    try_acquire (non-blocking): callers get an immediate denial
          //    rather than queuing, matching "max_concurrent=1 means at most
          //    one in flight, others are rejected" from the requirements.
          let _global_permit = match self.global_permits.clone().try_acquire_owned() {
              Ok(permit) => permit,
              Err(_) => {
                  let id = new_invocation_id();
                  self.write_start_row(&id, &spec, "denied", prompt_bytes)
                      .await?;
                  self.write_finish_row(&id, "denied", Some("global_concurrency_limited"), 0)
                      .await?;
                  return Err(LlmRunnerError::ConcurrencyLimited(self.config.max_concurrent));
              }
          };
          let _action_permit = match action_permits.try_acquire_owned() {
              Ok(permit) => permit,
              Err(_) => {
                  let id = new_invocation_id();
                  self.write_start_row(&id, &spec, "denied", prompt_bytes)
                      .await?;
                  self.write_finish_row(&id, "denied", Some("action_concurrency_limited"), 0)
                      .await?;
                  return Err(LlmRunnerError::ActionConcurrencyLimited {
                      action: action.clone(),
                      limit: self.config.max_per_action_concurrent,
                  });
              }
          };

          // 7. Run with timeout; record start row now that all guards passed.
          let invocation_id = new_invocation_id();
          self.write_start_row(&invocation_id, &spec, "running", prompt_bytes)
              .await?;
          let start = Instant::now();
          let timeout = Duration::from_secs(self.config.timeout_secs);
          let prompt = spec.prompt.clone();
          let run_result = tokio::time::timeout(timeout, run_fn(prompt)).await;
          let duration_ms = start.elapsed().as_millis() as i64;

          let mut state = self.action_state.lock().await;
          let entry = state.entry(action.clone()).or_default();

          match run_result {
              Ok(Ok(output)) => {
                  entry.consecutive_failures = 0;
                  let output_bytes = output.len().min(self.config.max_output_bytes);
                  let truncated_output: String = output.chars().take(output_bytes).collect();
                  drop(state);
                  self.write_finish_row_with_output(
                      &invocation_id,
                      "success",
                      None,
                      duration_ms,
                      truncated_output.len(),
                  )
                  .await?;
                  Ok(LlmInvocationOutcome {
                      invocation_id,
                      output: truncated_output,
                      duration_ms,
                      output_bytes,
                  })
              }
              Ok(Err(err)) => {
                  entry.consecutive_failures += 1;
                  let should_open = entry.consecutive_failures >= self.config.failure_threshold;
                  if should_open {
                      entry.circuit_open_until =
                          Some(Instant::now() + Duration::from_secs(self.config.cooldown_secs));
                  }
                  drop(state);
                  self.write_finish_row(&invocation_id, "error", Some(&sanitize_error(&err)), duration_ms)
                      .await?;
                  Err(LlmRunnerError::Internal(err))
              }
              Err(_elapsed) => {
                  entry.consecutive_failures += 1;
                  let should_open = entry.consecutive_failures >= self.config.failure_threshold;
                  if should_open {
                      entry.circuit_open_until =
                          Some(Instant::now() + Duration::from_secs(self.config.cooldown_secs));
                  }
                  drop(state);
                  self.write_finish_row(
                      &invocation_id,
                      "timeout",
                      Some("timed out"),
                      duration_ms,
                  )
                  .await?;
                  Err(LlmRunnerError::Timeout(invocation_id, self.config.timeout_secs))
              }
          }
      }

      // --- audit writers -----------------------------------------------
      // NOTE: Task 4 replaces the bodies of these three methods with calls
      // into `crate::db::llm_invocations::{insert_llm_invocation_running,
      // finish_llm_invocation}` — kept as direct rusqlite calls here so
      // Task 3 does not depend on Task 4 landing first. Task 4 preserves
      // the `crate::db::write_lock()` guard exactly as written here — it
      // moves into the new `src/db/llm_invocations.rs` functions, which is
      // where every other writer in this repo (`src/db/ingest.rs`,
      // `src/db/maintenance.rs`, `src/app/error_detection/scanner.rs`)
      // acquires it: immediately before the `conn.execute(...)` call,
      // scoped to end when the guard drops at the end of the closure.
      //
      // Eng review fix (CRITICAL — architecture + performance reviewers):
      // every write here MUST hold `crate::db::write_lock()` for the
      // duration of the `execute` call. Without it, every audit INSERT/
      // UPDATE races the syslog batch inserter, heartbeat, notifications,
      // and retention maintenance for SQLite's single write lock — the
      // exact hazard the invariant documented at the top of
      // `src/db/pool.rs` exists to prevent. See "Eng Review Fixes Applied"
      // (Fix 2) at the top of this plan.

      async fn write_start_row(
          &self,
          id: &str,
          spec: &LlmInvocationSpec,
          status: &str,
          prompt_bytes: usize,
      ) -> Result<(), LlmRunnerError> {
          let pool = self.pool.clone();
          let id = id.to_string();
          let caller_surface = spec.caller_surface.as_str().to_string();
          let action = spec.action.clone();
          let provider = spec.provider.clone();
          let model = spec.model.clone();
          let program = spec.program.clone();
          let incident_id = spec.incident_id.clone();
          let ai_tool = spec.ai_tool.clone();
          let ai_project = spec.ai_project.clone();
          let ai_session_id = spec.ai_session_id.clone();
          let evidence_counts_json = serde_json::to_string(&spec.evidence_counts).ok();
          let status = status.to_string();
          let metadata_json = build_metadata_json(&spec.extra_metadata);

          tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
              let conn = pool.get()?;
              // Serialize with every other writer in the process — see the
              // module-level invariant note and `src/db/pool.rs`'s own
              // doc comment on `write_lock()`. Reentrant, so this is safe
              // even if a future caller nests it, but the guard here is
              // scoped to just this INSERT.
              let _write_guard = crate::db::write_lock();
              conn.execute(
                  "INSERT INTO llm_invocations
                       (id, started_at, caller_surface, action, provider, model, program,
                        incident_id, ai_tool, ai_project, ai_session_id,
                        evidence_counts_json, prompt_bytes, status, metadata_json)
                   VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?2, ?3, ?4, ?5, ?6,
                           ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                  rusqlite::params![
                      id,
                      caller_surface,
                      action,
                      provider,
                      model,
                      program,
                      incident_id,
                      ai_tool,
                      ai_project,
                      ai_session_id,
                      evidence_counts_json,
                      prompt_bytes as i64,
                      status,
                      metadata_json,
                  ],
              )?;
              Ok(())
          })
          .await
          .map_err(|e| LlmRunnerError::Internal(anyhow::anyhow!("audit write join error: {e}")))?
          .map_err(LlmRunnerError::Internal)
      }

      async fn write_finish_row(
          &self,
          id: &str,
          status: &str,
          error: Option<&str>,
          duration_ms: i64,
      ) -> Result<(), LlmRunnerError> {
          self.write_finish_row_inner(id, status, error, duration_ms, None)
              .await
      }

      async fn write_finish_row_with_output(
          &self,
          id: &str,
          status: &str,
          error: Option<&str>,
          duration_ms: i64,
          output_bytes: usize,
      ) -> Result<(), LlmRunnerError> {
          self.write_finish_row_inner(id, status, error, duration_ms, Some(output_bytes))
              .await
      }

      async fn write_finish_row_inner(
          &self,
          id: &str,
          status: &str,
          error: Option<&str>,
          duration_ms: i64,
          output_bytes: Option<usize>,
      ) -> Result<(), LlmRunnerError> {
          let pool = self.pool.clone();
          let id = id.to_string();
          let status = status.to_string();
          let error = error.map(|e| e.to_string());
          let output_bytes = output_bytes.map(|b| b as i64);

          tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
              let conn = pool.get()?;
              // Same write_lock() invariant as write_start_row above.
              let _write_guard = crate::db::write_lock();
              conn.execute(
                  "UPDATE llm_invocations
                   SET finished_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                       duration_ms = ?2,
                       status = ?3,
                       error = ?4,
                       output_bytes = COALESCE(?5, output_bytes)
                   WHERE id = ?1",
                  rusqlite::params![id, duration_ms, status, error, output_bytes],
              )?;
              Ok(())
          })
          .await
          .map_err(|e| LlmRunnerError::Internal(anyhow::anyhow!("audit write join error: {e}")))?
          .map_err(LlmRunnerError::Internal)
      }
  }

  /// Build the `metadata_json` column value: merge caller-supplied
  /// `extra_metadata` with runner-added `host`/`pid`, then run the WHOLE
  /// serialized object through the same `redact_secrets` pass used for
  /// error messages, then enforce `LLM_METADATA_MAX_BYTES`.
  ///
  /// Eng review fix (security reviewer V3): `extra_metadata` is
  /// caller-supplied and, per the `extra_metadata` contract documented in
  /// "Locked interfaces for other phases", MUST NOT carry prompt/evidence
  /// content — but nothing stops a future caller (PR 2-4) from doing so by
  /// mistake. Redacting + size-capping here is the enforcement backstop:
  /// secret-shaped strings never reach the column, and oversized metadata
  /// is truncated with an explicit marker rather than silently stored in
  /// full. See "Eng Review Fixes Applied" (Fix 5).
  fn build_metadata_json(extra: &serde_json::Value) -> String {
      let host = hostname_best_effort();
      let pid = std::process::id();
      let mut obj = extra.as_object().cloned().unwrap_or_default();
      obj.insert("host".to_string(), serde_json::json!(host));
      obj.insert("pid".to_string(), serde_json::json!(pid));
      let raw = serde_json::Value::Object(obj).to_string();

      // Redact FIRST (same heuristic as error sanitization), THEN bound
      // size — never truncate before redacting, or a secret can be split
      // across the boundary and its surviving half leak.
      let redacted = redact_secrets(&raw);

      if redacted.len() <= LLM_METADATA_MAX_BYTES {
          return redacted;
      }

      // Oversized even after redaction: truncate to a valid-JSON-ish
      // marker object rather than silently storing a partial/corrupt blob.
      // We deliberately do NOT try to preserve partial structure — an
      // oversized `extra_metadata` is itself a contract violation (see the
      // `extra_metadata` doc note), so the marker calls that out instead of
      // pretending truncation is a normal, silent path.
      serde_json::json!({
          "truncated": true,
          "reason": "extra_metadata exceeded LLM_METADATA_MAX_BYTES",
          "original_bytes": redacted.len(),
          "max_bytes": LLM_METADATA_MAX_BYTES,
          "host": host,
          "pid": pid,
      })
      .to_string()
  }

  fn hostname_best_effort() -> String {
      std::env::var("HOSTNAME")
          .ok()
          .filter(|v| !v.is_empty())
          .unwrap_or_else(|| "unknown".to_string())
  }

  /// Sanitize an error before persisting: redact FIRST using the same
  /// `redact_secrets`/`looks_secretish` heuristic as
  /// `src/assessment.rs` (imported, not forked — see the Step 3
  /// prerequisite above that widens those two functions to `pub(crate)`),
  /// THEN bound length. Redact-before-truncate is load-bearing: bounding
  /// first can split a secret across the truncation boundary and let the
  /// surviving half leak into the persisted `error` column.
  ///
  /// Eng review fix (security reviewer V1/V4, MP1): this previously forked
  /// a weaker heuristic (missing the `sk-`/`ghp_`/`atk_` prefix checks) and
  /// truncated before redacting. See "Eng Review Fixes Applied" (Fix 3).
  fn sanitize_error(err: &anyhow::Error) -> String {
      let text = err.to_string();
      let redacted = redact_secrets(&text);
      redacted.chars().take(2048).collect()
  }

  /// Stable-across-start/finish invocation id. Timestamp-prefixed for sort
  /// order and human debuggability, matching the timestamp+random style
  /// used elsewhere in this repo for generated identifiers (see
  /// `src/db/error_signatures.rs` signature_hash generation for the
  /// nearest precedent — this uses a simple counter+random suffix instead
  /// of a content hash since there is no stable content to hash before the
  /// invocation starts).
  fn new_invocation_id() -> String {
      let now = chrono::Utc::now();
      let rand_suffix: u32 = {
          use std::time::{SystemTime, UNIX_EPOCH};
          let nanos = SystemTime::now()
              .duration_since(UNIX_EPOCH)
              .map(|d| d.subsec_nanos())
              .unwrap_or(0);
          nanos ^ std::process::id()
      };
      format!("llm-{}-{:08x}", now.format("%Y%m%dT%H%M%S%.6f"), rand_suffix)
  }

  #[cfg(test)]
  #[path = "llm_runner_tests.rs"]
  mod tests;
  ```

  Register the module — run `grep -n '^mod \|^pub mod' src/app.rs` first to find the exact list, then add in the same style (alphabetically, matching sibling `mod error;` / `mod services;` visibility):

  ```rust
  pub mod llm_runner;
  ```

  If `thiserror` is not already a dependency (check `grep -n '^thiserror' Cargo.toml` — it almost certainly is since `src/app/error.rs` already uses `#[derive(Error)]` from it), no `Cargo.toml` change is needed.

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib app::llm_runner_tests::`
  Expected: PASS for all 13 tests (the original 7 plus the 6 added by the eng-review fixes: `timeout_secs_accessor_exposes_resolved_config_value`, `error_with_secretish_tokens_is_redacted_before_persisting`, `sanitize_error_redacts_before_truncating_so_boundary_split_secrets_do_not_leak`, `extra_metadata_with_secretish_value_is_redacted_before_persisting`, `extra_metadata_over_byte_cap_is_truncated_not_silently_dropped`, `audit_write_does_not_race_a_concurrent_write_locked_writer`). If `try_acquire_owned` semantics make `global_concurrency_limit_denies_second_concurrent_call` flaky (race between the spawned task acquiring its permit and the main task calling `run`), that is expected to be resolved by the `started_rx.await` synchronization already in the test — no sleep-based polling needed.

- [ ] **Step 5: Commit**
  ```bash
  git add src/app/llm_runner.rs src/app/llm_runner_tests.rs src/app.rs
  git commit -m "feat: add LlmRunner shared invocation guard (concurrency, rate limit, circuit breaker, kill switch)"
  ```

---

## Task 4: `src/db/llm_invocations.rs` — shared audit query layer

**Files:**
- Create: `src/db/llm_invocations.rs`
- Modify: `src/db.rs` (add `pub(crate) mod llm_invocations;` and re-export `pub use llm_invocations::{LlmInvocationRow, insert_llm_invocation_running, finish_llm_invocation, list_llm_invocations};` in the same alphabetically-grouped style as the existing `pub(crate) mod notifications;` / `pub use` blocks)
- Modify: `src/app/llm_runner.rs` — replace the three inline `write_start_row` / `write_finish_row` / `write_finish_row_with_output` rusqlite bodies (from Task 3) with calls to the new `src/db/llm_invocations.rs` functions, so there is exactly one place that knows the `llm_invocations` column list.
- Test: `src/db/llm_invocations_tests.rs` (sidecar convention, `#[cfg(test)] #[path = "llm_invocations_tests.rs"] mod tests;` at bottom of `src/db/llm_invocations.rs`)

**Interfaces:**
- Consumes: the `llm_invocations` table from Task 1; `LlmEvidenceCounts` type from Task 3 is NOT depended on here — this module works with plain `Option<String>`/`i64` primitives like `src/db/notifications.rs` does, so `LlmRunner` (Task 3) is responsible for calling `serde_json::to_string` before passing evidence counts in.
- Produces:
  ```rust
  pub struct LlmInvocationInsertParams { /* one field per NOT-NULL-at-insert-time column */ }
  pub fn insert_llm_invocation_running(conn: &rusqlite::Connection, id: &str, p: &LlmInvocationInsertParams) -> rusqlite::Result<()>;
  pub fn finish_llm_invocation(conn: &rusqlite::Connection, id: &str, status: &str, error: Option<&str>, duration_ms: i64, output_bytes: Option<i64>) -> rusqlite::Result<()>;
  pub struct LlmInvocationRow { /* exact fields listed in "Locked interfaces" above */ }
  pub fn list_llm_invocations(conn: &rusqlite::Connection, limit: i64, since: Option<&str>, action: Option<&str>, status: Option<&str>) -> rusqlite::Result<Vec<LlmInvocationRow>>;
  ```
  Task 7 (CLI/MCP/REST read surfaces) calls `list_llm_invocations` directly (via a new `CortexService` method that wraps it in `run_db`, matching `notifications_recent_checked`'s pattern in `src/app/services/rag.rs`).

- [ ] **Step 1: Write the failing test**

  ```rust
  // src/db/llm_invocations_tests.rs
  use super::*;

  fn test_conn() -> rusqlite::Connection {
      let dir = tempfile::tempdir().unwrap();
      let db_path = dir.path().join("test.db");
      let pool = crate::db::init_pool(&db_path, 1, 64, 64).unwrap();
      std::mem::forget(dir);
      pool.get().unwrap()
  }

  fn sample_params() -> LlmInvocationInsertParams {
      LlmInvocationInsertParams {
          caller_surface: "test".to_string(),
          action: "ai_assess".to_string(),
          provider: "gemini-cli".to_string(),
          model: Some("gemini-3.1-flash-lite-preview".to_string()),
          program: Some("gemini".to_string()),
          incident_id: Some("inc-42".to_string()),
          ai_tool: None,
          ai_project: Some("cortex".to_string()),
          ai_session_id: None,
          evidence_counts_json: Some(r#"{"total_incidents":1}"#.to_string()),
          prompt_bytes: Some(128),
          status: "running".to_string(),
          metadata_json: Some(r#"{"host":"devhost","pid":123}"#.to_string()),
      }
  }

  #[test]
  fn insert_then_finish_round_trips() {
      let conn = test_conn();
      insert_llm_invocation_running(&conn, "llm-test-1", &sample_params()).unwrap();
      finish_llm_invocation(&conn, "llm-test-1", "success", None, 4200, Some(512)).unwrap();

      let rows = list_llm_invocations(&conn, 10, None, None, None).unwrap();
      assert_eq!(rows.len(), 1);
      let row = &rows[0];
      assert_eq!(row.id, "llm-test-1");
      assert_eq!(row.status, "success");
      assert_eq!(row.duration_ms, Some(4200));
      assert_eq!(row.output_bytes, Some(512));
      assert_eq!(row.incident_id.as_deref(), Some("inc-42"));
      assert!(row.finished_at.is_some());
  }

  #[test]
  fn list_filters_by_action_and_status_and_since() {
      let conn = test_conn();
      insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
      finish_llm_invocation(&conn, "llm-a", "success", None, 100, Some(10)).unwrap();

      let mut other = sample_params();
      other.action = "skill_assess".to_string();
      insert_llm_invocation_running(&conn, "llm-b", &other).unwrap();
      finish_llm_invocation(&conn, "llm-b", "error", Some("boom"), 50, None).unwrap();

      let ai_only = list_llm_invocations(&conn, 10, None, Some("ai_assess"), None).unwrap();
      assert_eq!(ai_only.len(), 1);
      assert_eq!(ai_only[0].id, "llm-a");

      let errors_only = list_llm_invocations(&conn, 10, None, None, Some("error")).unwrap();
      assert_eq!(errors_only.len(), 1);
      assert_eq!(errors_only[0].id, "llm-b");

      let future_since = list_llm_invocations(&conn, 10, Some("2999-01-01T00:00:00Z"), None, None).unwrap();
      assert!(future_since.is_empty());
  }

  #[test]
  fn list_respects_limit_and_orders_newest_first() {
      let conn = test_conn();
      for i in 0..5 {
          let id = format!("llm-{i}");
          insert_llm_invocation_running(&conn, &id, &sample_params()).unwrap();
          finish_llm_invocation(&conn, &id, "success", None, 10, Some(1)).unwrap();
      }
      let rows = list_llm_invocations(&conn, 2, None, None, None).unwrap();
      assert_eq!(rows.len(), 2);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib db::llm_invocations_tests:: -- --nocapture`
  Expected: FAIL with a compile error — `src/db/llm_invocations.rs` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

  ```rust
  // src/db/llm_invocations.rs
  //! Database operations for the shared LLM invocation audit table
  //! (`llm_invocations`, migration 37). `LlmRunner`
  //! (`src/app/llm_runner.rs`) is the only writer; the CLI/MCP/REST read
  //! surfaces (`sessions llm-invocations`, MCP `llm_invocations` action,
  //! `GET /api/sessions/llm-invocations`) are the readers.
  //!
  //! Call from inside `tokio::task::spawn_blocking`, never from async
  //! context directly (same convention as `src/db/notifications.rs`).

  use rusqlite::params;

  /// Parameters for the initial (status='running' or a denial status)
  /// insert. `id` is passed separately since callers generate it before
  /// building the params (needed so denial paths can audit without a
  /// completed spec).
  pub struct LlmInvocationInsertParams {
      pub caller_surface: String,
      pub action: String,
      pub provider: String,
      pub model: Option<String>,
      pub program: Option<String>,
      pub incident_id: Option<String>,
      pub ai_tool: Option<String>,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub evidence_counts_json: Option<String>,
      pub prompt_bytes: Option<i64>,
      pub status: String,
      pub metadata_json: Option<String>,
  }

  pub fn insert_llm_invocation_running(
      conn: &rusqlite::Connection,
      id: &str,
      p: &LlmInvocationInsertParams,
  ) -> rusqlite::Result<()> {
      conn.execute(
          "INSERT INTO llm_invocations
               (id, started_at, caller_surface, action, provider, model, program,
                incident_id, ai_tool, ai_project, ai_session_id,
                evidence_counts_json, prompt_bytes, status, metadata_json)
           VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?2, ?3, ?4, ?5, ?6,
                   ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
          params![
              id,
              p.caller_surface,
              p.action,
              p.provider,
              p.model,
              p.program,
              p.incident_id,
              p.ai_tool,
              p.ai_project,
              p.ai_session_id,
              p.evidence_counts_json,
              p.prompt_bytes,
              p.status,
              p.metadata_json,
          ],
      )?;
      Ok(())
  }

  pub fn finish_llm_invocation(
      conn: &rusqlite::Connection,
      id: &str,
      status: &str,
      error: Option<&str>,
      duration_ms: i64,
      output_bytes: Option<i64>,
  ) -> rusqlite::Result<()> {
      conn.execute(
          "UPDATE llm_invocations
           SET finished_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
               duration_ms = ?2,
               status = ?3,
               error = ?4,
               output_bytes = COALESCE(?5, output_bytes)
           WHERE id = ?1",
          params![id, duration_ms, status, error, output_bytes],
      )?;
      Ok(())
  }

  /// A row from `llm_invocations`, as returned to CLI/MCP/REST readers.
  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct LlmInvocationRow {
      pub id: String,
      pub started_at: String,
      pub finished_at: Option<String>,
      pub duration_ms: Option<i64>,
      pub caller_surface: String,
      pub action: String,
      pub provider: String,
      pub model: Option<String>,
      pub program: Option<String>,
      pub incident_id: Option<String>,
      pub ai_tool: Option<String>,
      pub ai_project: Option<String>,
      pub ai_session_id: Option<String>,
      pub evidence_counts_json: Option<String>,
      pub prompt_bytes: Option<i64>,
      pub output_bytes: Option<i64>,
      pub status: String,
      pub error: Option<String>,
      pub metadata_json: Option<String>,
  }

  /// Fetch recent invocations, optionally filtered by `action`/`status` and
  /// bounded to those started at or after `since` (ISO8601). `limit` is
  /// clamped to `[1, 500]`, matching `notifications::firings_recent`.
  pub fn list_llm_invocations(
      conn: &rusqlite::Connection,
      limit: i64,
      since: Option<&str>,
      action: Option<&str>,
      status: Option<&str>,
  ) -> rusqlite::Result<Vec<LlmInvocationRow>> {
      let clamped_limit = limit.clamp(1, 500);
      let mut stmt = conn.prepare(
          "SELECT id, started_at, finished_at, duration_ms, caller_surface, action,
                  provider, model, program, incident_id, ai_tool, ai_project,
                  ai_session_id, evidence_counts_json, prompt_bytes, output_bytes,
                  status, error, metadata_json
           FROM llm_invocations
           WHERE (?1 IS NULL OR started_at >= ?1)
             AND (?2 IS NULL OR action = ?2)
             AND (?3 IS NULL OR status = ?3)
           ORDER BY started_at DESC
           LIMIT ?4",
      )?;
      let rows = stmt
          .query_map(params![since, action, status, clamped_limit], |row| {
              Ok(LlmInvocationRow {
                  id: row.get(0)?,
                  started_at: row.get(1)?,
                  finished_at: row.get(2)?,
                  duration_ms: row.get(3)?,
                  caller_surface: row.get(4)?,
                  action: row.get(5)?,
                  provider: row.get(6)?,
                  model: row.get(7)?,
                  program: row.get(8)?,
                  incident_id: row.get(9)?,
                  ai_tool: row.get(10)?,
                  ai_project: row.get(11)?,
                  ai_session_id: row.get(12)?,
                  evidence_counts_json: row.get(13)?,
                  prompt_bytes: row.get(14)?,
                  output_bytes: row.get(15)?,
                  status: row.get(16)?,
                  error: row.get(17)?,
                  metadata_json: row.get(18)?,
              })
          })?
          .collect::<rusqlite::Result<Vec<_>>>()?;
      Ok(rows)
  }

  #[cfg(test)]
  #[path = "llm_invocations_tests.rs"]
  mod tests;
  ```

  In `src/db.rs`, add (alphabetically among the existing `mod`/`pub(crate) mod` lines, e.g. right after `mod ingest;` and before `mod maintenance;` — match exact existing ordering):

  ```rust
  pub(crate) mod llm_invocations;
  ```

  and add to the `pub use` block (grouped near `pub(crate) use notifications;` — actually `notifications` is `pub(crate) mod notifications;` with no top-level re-export of its items in the excerpt seen; confirm with `grep -n "notifications::" src/db.rs` — if `notifications` items are NOT re-exported at the `db` module root and callers instead write `crate::db::notifications::FiringRow`, follow that exact pattern for `llm_invocations` too, i.e. do NOT add a `pub use llm_invocations::*` — callers write `crate::db::llm_invocations::{LlmInvocationRow, list_llm_invocations, ...}` directly, matching how `src/app/services/rag.rs` writes `crate::db::notifications::FiringRow`).

  Now update `src/app/llm_runner.rs` (from Task 3) to replace the bodies of `write_start_row`, `write_finish_row_inner` with calls into the new module instead of inline SQL. **Eng review fix (Fix 2) note:** `crate::db::llm_invocations::{insert_llm_invocation_running, finish_llm_invocation}` take a plain `&rusqlite::Connection` — they do NOT acquire `write_lock()` themselves, exactly like `insert_logs_batch_in_tx` in `src/db/ingest.rs` does not lock itself (its caller, `insert_logs_batch_once`, does). The lock MUST stay at the `llm_runner.rs` call site, immediately around `pool.get()` + the call — do NOT drop the `let _write_guard = crate::db::write_lock();` line Task 3 already added when doing this replacement:

  ```rust
  // In write_start_row's spawn_blocking closure, replace the inline
  // `conn.execute(...)` block with the shared query-layer call — KEEP the
  // `let _write_guard = crate::db::write_lock();` line from Task 3
  // immediately above this, unchanged:
  let params = crate::db::llm_invocations::LlmInvocationInsertParams {
      caller_surface,
      action,
      provider,
      model: Some(model),
      program: Some(program),
      incident_id,
      ai_tool,
      ai_project,
      ai_session_id,
      evidence_counts_json,
      prompt_bytes: Some(prompt_bytes as i64),
      status,
      metadata_json: Some(metadata_json),
  };
  crate::db::llm_invocations::insert_llm_invocation_running(&conn, &id, &params)?;
  ```

  ```rust
  // In write_finish_row_inner's spawn_blocking closure, replace the inline
  // `conn.execute(...)` block with the shared query-layer call — again,
  // KEEP the `let _write_guard = crate::db::write_lock();` line from
  // Task 3 immediately above this, unchanged:
  crate::db::llm_invocations::finish_llm_invocation(
      &conn, &id, &status, error.as_deref(), duration_ms, output_bytes,
  )?;
  ```

  The full closure body after this edit reads (shown for `write_start_row`; `write_finish_row_inner` follows the identical shape):

  ```rust
  tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
      let conn = pool.get()?;
      let _write_guard = crate::db::write_lock();
      let params = crate::db::llm_invocations::LlmInvocationInsertParams {
          caller_surface, action, provider, model: Some(model), program: Some(program),
          incident_id, ai_tool, ai_project, ai_session_id, evidence_counts_json,
          prompt_bytes: Some(prompt_bytes as i64), status, metadata_json: Some(metadata_json),
      };
      crate::db::llm_invocations::insert_llm_invocation_running(&conn, &id, &params)?;
      Ok(())
  })
  .await
  .map_err(|e| LlmRunnerError::Internal(anyhow::anyhow!("audit write join error: {e}")))?
  .map_err(LlmRunnerError::Internal)
  ```

  (Keep the surrounding `tokio::task::spawn_blocking` wrapper, `pool.get()?` call, and `write_lock()` guard exactly as Task 3 wrote them — only the body inside the guard changes from inline SQL to the shared query-layer call.)

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib db::llm_invocations_tests:: app::llm_runner_tests::`
  Expected: PASS — both the new DB-layer tests and the Task 3 `LlmRunner` tests (now routed through the shared query functions) still pass.

- [ ] **Step 5: Commit**
  ```bash
  git add src/db/llm_invocations.rs src/db.rs src/app/llm_runner.rs
  git commit -m "feat: extract llm_invocations DB query layer, wire LlmRunner to use it"
  ```

---

## Task 5: Wire `LlmRunner` into `CortexService` and `RuntimeCore`

**Files:**
- Modify: `src/app/services.rs:114-125` (add `llm_runner: Arc<crate::app::llm_runner::LlmRunner>` field to `CortexService`), and near line 141-160 (constructor) and the `with_file_tail_registry`-style builder methods around line 180-190 (add `with_llm_runner`)
- Modify: `src/runtime.rs` around line 264 (thread `LlmRunner::new(Arc::clone(&pool), config.llm.clone())` through the builder chain)
- Test: add to `src/app/service_tests.rs` (existing sidecar; confirm exact path with `grep -n '#\[path' src/app/services.rs`)

**Interfaces:**
- Consumes: `LlmRunner::new(pool: Arc<DbPool>, config: LlmConfig)` (Task 3), `Config.llm` (Task 2).
- Produces: `CortexService::llm(&self) -> &crate::app::llm_runner::LlmRunner` accessor, matching the existing `pub fn alerts(&self) -> AlertsDomain<'_>` style but returning a plain reference (no domain wrapper needed since `LlmRunner` is already a cohesive struct with its own methods). Task 6 (migrate `ai assess`) and Task 7 (read surfaces) both call `state.service.llm()` / `service.llm()` to reach the runner.

- [ ] **Step 1: Write the failing test**

  ```rust
  // append to src/app/service_tests.rs (or wherever the sidecar is — confirm path first)
  #[tokio::test]
  async fn service_exposes_llm_runner_with_configured_defaults() {
      let (service, _pool, _dir) = test_service();
      // Exercise the accessor exists and is wired to a real DbPool by
      // running a denied (disabled) dry_run-equivalent call end to end.
      let mut cfg = crate::config::LlmConfig::default();
      cfg.enabled = false;
      // test_service() wires LlmRunner with LlmConfig::default() (enabled
      // by default); this test only asserts the accessor is reachable and
      // returns something whose type is `&LlmRunner` — full behavior is
      // covered by app::llm_runner_tests. A compile-time check plus one
      // trivial call is sufficient here.
      let spec = crate::app::llm_runner::LlmInvocationSpec {
          caller_surface: crate::app::llm_runner::LlmCallerSurface::Test,
          action: "ai_assess".to_string(),
          incident_id: None,
          ai_tool: None,
          ai_project: None,
          ai_session_id: None,
          evidence_counts: crate::app::llm_runner::LlmEvidenceCounts::default(),
          prompt: "ping".to_string(),
          provider: "test".to_string(),
          model: "test".to_string(),
          program: "test".to_string(),
          extra_metadata: serde_json::json!({}),
      };
      let outcome = service
          .llm()
          .run(spec, |_p| async { Ok("pong".to_string()) })
          .await
          .unwrap();
      assert_eq!(outcome.output, "pong");
  }
  ```

  NOTE: first read `fn test_service()` in `src/app/service_tests.rs` (or wherever it's defined — `grep -n "fn test_service" src/app/*.rs`) to see its exact return type and how it currently constructs `CortexService`; adjust the test above if `test_service()` needs a new parameter or if `CortexService` construction there needs updating to pass an `LlmRunner`.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib app::service_tests::service_exposes_llm_runner_with_configured_defaults -- --nocapture`
  Expected: FAIL with a compile error — `CortexService::llm` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

  In `src/app/services.rs`, add the field to the struct (after `file_tail_statuses`):

  ```rust
  pub struct CortexService {
      pool: Arc<DbPool>,
      pub(super) storage: StorageConfig,
      db_permits: Arc<Semaphore>,
      pub(super) heavy_read_permits: Arc<Semaphore>,
      acquire_timeout: Duration,
      pub(super) os: Arc<dyn OsAdapter + Send + Sync>,
      file_tail_registry: Option<Arc<FileTailRegistry>>,
      file_tail_reconcile: Option<Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>>,
      file_tail_statuses: Option<Arc<dyn Fn() -> Vec<FileTailStatus> + Send + Sync>>,
      llm_runner: Arc<crate::app::llm_runner::LlmRunner>,
  }
  ```

  In `CortexService::new`, construct a default `LlmRunner` from `LlmConfig::default()` (callers that need non-default config call the new builder below):

  ```rust
  pub(crate) fn new(pool: Arc<DbPool>, storage: StorageConfig) -> Self {
      let permits = read_permits_for_pool(storage.pool_size);
      let heavy_read_concurrency = storage.heavy_read_concurrency;
      Self {
          pool: pool.clone(),
          storage,
          db_permits: Arc::new(Semaphore::new(permits)),
          heavy_read_permits: Arc::new(Semaphore::new(heavy_read_concurrency)),
          acquire_timeout: DB_ACQUIRE_TIMEOUT,
          os: Arc::new(SystemOsAdapter),
          file_tail_registry: None,
          file_tail_reconcile: None,
          file_tail_statuses: None,
          llm_runner: Arc::new(crate::app::llm_runner::LlmRunner::new(
              pool,
              crate::config::LlmConfig::default(),
          )),
      }
  }
  ```

  Add the builder method next to `with_file_tail_registry` (~line 180):

  ```rust
  pub(crate) fn with_llm_config(mut self, config: crate::config::LlmConfig) -> Self {
      self.llm_runner = Arc::new(crate::app::llm_runner::LlmRunner::new(
          self.pool.clone(),
          config,
      ));
      self
  }
  ```

  Add the accessor next to other domain accessors (in `src/app/services/domains.rs`, inside `impl CortexService`, after `pub fn compose`):

  ```rust
  pub fn llm(&self) -> &crate::app::llm_runner::LlmRunner {
      &self.llm_runner
  }
  ```

  NOTE: `llm_runner` is a private field on `CortexService` defined in `src/app/services.rs`, but `domains.rs` is a sibling module (`src/app/services/domains.rs`) — check whether `domains.rs` currently accesses private `CortexService` fields directly (it does not appear to; the existing domain structs only hold `service: &'a CortexService` and call `self.service.<method>()`). Either (a) mark `llm_runner` field `pub(super)` so `services/domains.rs` (a child module of `services`) can read it directly, matching the existing `pub(super) storage` / `pub(super) os` visibility already on the struct, or (b) add the `llm()` accessor method directly in `src/app/services.rs` next to `CortexService::new` instead of in `domains.rs`. Prefer (b) — simpler, no visibility widening needed, and matches that `pool`/`db_permits`/`acquire_timeout` (fully private) are only ever touched from within `services.rs` itself, never from `domains.rs`.

  Revised: add to `src/app/services.rs`, directly inside `impl CortexService` (the same `impl` block as `new`/`with_llm_config`):

  ```rust
  pub fn llm(&self) -> &crate::app::llm_runner::LlmRunner {
      &self.llm_runner
  }
  ```

  In `src/runtime.rs`, thread the real config through at construction (around line 264, next to the existing `with_file_tail_registry` chain):

  ```rust
  let mut service = CortexService::new(Arc::clone(&pool), config.storage.clone())
      .with_llm_config(config.llm.clone());
  ```

  (Insert `.with_llm_config(config.llm.clone())` into the existing chain/assignment at that call site — read the surrounding 20 lines in `runtime.rs` first since `service` is subsequently reassigned via `service = service.with_file_tail_registry(...)` etc.; either chain it inline or add one more `service = service.with_llm_config(config.llm.clone());` line alongside the others.)

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib app::service_tests::service_exposes_llm_runner_with_configured_defaults`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/app/services.rs src/app/services/domains.rs src/runtime.rs src/app/service_tests.rs
  git commit -m "feat: wire LlmRunner into CortexService and RuntimeCore"
  ```

---

## Task 6: Migrate `cortex sessions assess` (Gemini subprocess call) onto `LlmRunner`

**Files:**
- Modify: `src/app/services/assessment.rs:1-65` (replace the direct `run_gemini_assessment(&prompt, &gemini_config, on_delta)` call with `state.service.llm().run(spec, run_fn)`)
- Modify: `src/assessment.rs` — **Eng review fix (Fix 1):** `GeminiAssessConfig::from_env` signature changes from `from_env(model_override: Option<String>) -> Self` to `from_env(model_override: Option<String>, timeout_secs: u64) -> Self` (stops independently reading `CORTEX_LLM_COMPLETION_TIMEOUT_SECS`; logs a deprecation warning if that env var is still set). No other change to the Gemini spawn/parse internals — this task otherwise only wraps `run_gemini_assessment` as the `run_fn` closure body. Confirm `run_gemini_assessment`'s current signature (`pub(crate) async fn run_gemini_assessment<F>(prompt: &str, config: &GeminiAssessConfig, on_delta: F) -> Result<String>`) stays callable from inside the `LlmRunner::run` closure — it needs to become `'static` (owns `prompt`/`config` instead of borrowing) since `run_fn: FnOnce(String) -> Fut + Send + 'static`.
- Modify: `src/assessment_tests.rs` — update every existing `GeminiAssessConfig::from_env(...)` test call site (grep `grep -rn "GeminiAssessConfig::from_env" src/assessment_tests.rs` first) to pass an explicit `timeout_secs` argument, and add the new `from_env_uses_the_passed_timeout_not_the_legacy_env_var` test (see Fix 1 note above this task's Step 1).
- Test: `src/app/service_tests.rs` or a new focused sidecar test in `src/app/services/assessment_tests.rs` if one exists (`grep -n '#\[path' src/app/services/assessment.rs` — currently that file has NO `#[cfg(test)]` block based on the earlier read, so create `src/app/services/assessment_tests.rs` and add the `#[cfg(test)] #[path = "assessment_tests.rs"] mod tests;` hook at the bottom of `assessment.rs`)

**Interfaces:**
- Consumes: `CortexService::llm(&self) -> &LlmRunner` (Task 5), `LlmRunner::run` / `LlmRunner::timeout_secs()` (Task 3), `LlmInvocationSpec`/`LlmCallerSurface`/`LlmEvidenceCounts` (Task 3), existing `GeminiAssessConfig::from_env` (Fix 1 below changes its signature — see prerequisite), `build_assessment_prompt`, `run_gemini_assessment` (pre-existing in `src/assessment.rs`, signature unchanged for `run_gemini_assessment`/`build_assessment_prompt`).
- Produces: `CortexService::run_gemini_assess` / `run_gemini_assess_with_delta` keep their EXACT existing public signatures (`pub async fn run_gemini_assess(&self, req: AiAssessRequest) -> ServiceResult<AiAssessResponse>` and `pub async fn run_gemini_assess_with_delta<F>(&self, req: AiAssessRequest, on_delta: F) -> ServiceResult<AiAssessResponse> where F: FnMut(&str) -> anyhow::Result<()> + Send`) — this is a pure internal migration, `src/cli/dispatch_sessions.rs:493-538` (`run_ai_assess`) requires ZERO changes. Every `llm_invocations` row for `action='ai_assess'` from this point forward is written by `LlmRunner`, not ad hoc.

  **Eng review fix (Fix 1 — architecture + performance reviewers, timeout duplication regression):** before this migration, `run_gemini_assessment`'s own `tokio::time::timeout(Duration::from_secs(config.timeout_secs), ...)` calls (in `src/assessment.rs`) were the ONLY timeout on the Gemini subprocess call, driven by `GeminiAssessConfig::timeout_secs` (`CORTEX_LLM_COMPLETION_TIMEOUT_SECS`, default 120). After this migration, `LlmRunner::run` ALSO wraps the whole call in its own `tokio::time::timeout(Duration::from_secs(self.config.timeout_secs), ...)` driven by a SEPARATE, independently configured `[llm].timeout_secs` (also default 120). If an operator sets only `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` without also setting `[llm].timeout_secs`, the effective timeout silently becomes `min(both)` and drifts if either is changed independently — a real, silent latency regression for the one existing production caller.

  **Chosen fix — approach (b):** stop `GeminiAssessConfig::from_env` from reading `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` for this call path; instead thread `LlmRunner::timeout_secs()` (i.e. the resolved `[llm].timeout_secs`) into `GeminiAssessConfig` construction, so there is exactly ONE source of truth end to end. Approach (a) — adding a `timeout_override: Option<Duration>` field to the locked `LlmInvocationSpec` struct — was considered and rejected: `LlmInvocationSpec`/`LlmRunner::run` is a locked interface PR 2-4 depend on, and an override field would only fix the OUTER `LlmRunner::run` timeout while leaving `run_gemini_assessment`'s FOUR separate internal `tokio::time::timeout` calls (stdout stream, process wait, stdin close, stderr read — see `src/assessment.rs:153-221`) still independently driven by the old, unrelated env var. Approach (b) fixes the root cause (two independently-resolved config values) rather than papering over just the outer layer, and is a smaller net diff since it touches one already-`pub(crate)` struct (`GeminiAssessConfig`) instead of widening a locked cross-PR interface.

  First, in `src/assessment.rs`, change `GeminiAssessConfig::from_env` to accept the resolved timeout instead of re-reading the env var, and log a deprecation warning if the old env var is still set so operators notice it's now a no-op for this path:

  ```rust
  // src/assessment.rs — replace the existing GeminiAssessConfig::from_env
  // with a version that takes the resolved timeout as a parameter instead
  // of independently resolving it from CORTEX_LLM_COMPLETION_TIMEOUT_SECS.
  impl GeminiAssessConfig {
      /// `timeout_secs` MUST be the same resolved value `LlmRunner` uses as
      /// its own outer timeout (`LlmRunner::timeout_secs()`, i.e.
      /// `[llm].timeout_secs` / `CORTEX_LLM_TIMEOUT_SECS`) — see the eng
      /// review Fix 1 note in the LLM invocation guard plan. This
      /// eliminates the double-timeout-source bug where
      /// `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` (this struct's old
      /// independent env read) and `[llm].timeout_secs` (LlmRunner's) could
      /// silently disagree.
      pub(crate) fn from_env(model_override: Option<String>, timeout_secs: u64) -> Self {
          if non_empty_env("CORTEX_LLM_COMPLETION_TIMEOUT_SECS").is_some() {
              tracing::warn!(
                  "CORTEX_LLM_COMPLETION_TIMEOUT_SECS is set but is now superseded by \
                   [llm].timeout_secs (CORTEX_LLM_TIMEOUT_SECS) for the `cortex sessions assess` \
                   path; the old env var is ignored here. Set [llm].timeout_secs instead."
              );
          }
          Self {
              program: env_or_default("CORTEX_HEADLESS_GEMINI_CMD", "gemini"),
              model: model_override
                  .or_else(|| non_empty_env("CORTEX_HEADLESS_GEMINI_MODEL"))
                  .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string()),
              source_home: non_empty_env("CORTEX_HEADLESS_GEMINI_HOME").map(PathBuf::from),
              timeout_secs: timeout_secs.max(1),
          }
      }
  }
  ```

  This is a breaking signature change to a `pub(crate)` function with exactly one call site in production code (`src/app/services/assessment.rs:17`, which Task 6's own Step 3 rewrites below to pass `self.llm().timeout_secs()`) plus its test callers in `src/assessment_tests.rs` — grep `grep -rn "GeminiAssessConfig::from_env" src/` first and update every call site found (test call sites pass a literal timeout, e.g. `GeminiAssessConfig::from_env(None, 120)`, matching whatever timeout that specific test needs).

  Add a test for the deprecation warning and the new single-source-of-truth behavior directly in `src/assessment_tests.rs` (sidecar of `src/assessment.rs`):

  ```rust
  // src/assessment_tests.rs — append near other GeminiAssessConfig tests
  #[test]
  fn from_env_uses_the_passed_timeout_not_the_legacy_env_var() {
      // SAFETY: matches this repo's existing pattern for serializing
      // env-mutating tests — reuse whatever Mutex/guard
      // src/assessment_tests.rs already uses for CORTEX_HEADLESS_GEMINI_*
      // env vars (grep `Mutex` or `serial` in this file first).
      std::env::set_var("CORTEX_LLM_COMPLETION_TIMEOUT_SECS", "5");
      let cfg = GeminiAssessConfig::from_env(None, 77);
      assert_eq!(
          cfg.timeout_secs, 77,
          "GeminiAssessConfig must use the passed-in resolved timeout, \
           NOT re-read CORTEX_LLM_COMPLETION_TIMEOUT_SECS, so it can never \
           silently disagree with LlmRunner's own outer timeout"
      );
      std::env::remove_var("CORTEX_LLM_COMPLETION_TIMEOUT_SECS");
  }
  ```

  This test is the "assert the two values don't silently produce a shorter-than-expected effective timeout" check the fix requirement calls for: with the old code, setting `CORTEX_LLM_COMPLETION_TIMEOUT_SECS=5` while `[llm].timeout_secs` stayed at its default 120 would have silently made the effective end-to-end timeout 5s (`min(5, 120)`); with this fix, the legacy env var is inert for this path and `77` (the value `LlmRunner::timeout_secs()` would have resolved to) wins unconditionally.

- [ ] **Step 1: Write the failing test**

  ```rust
  // src/app/services/assessment_tests.rs
  use super::*;

  #[tokio::test]
  async fn ai_assess_writes_llm_invocation_audit_row_via_runner() {
      // Uses the same test harness pattern as src/app/service_tests.rs's
      // test_service() — confirm the exact helper name/signature there
      // first; this test assumes it returns (CortexService, Arc<DbPool>, TempDir).
      let (service, pool, _dir) = crate::app::service_tests::test_service();

      // Seed one AI incident so investigate_ai_incidents(...) inside
      // run_gemini_assess_with_delta finds a matching incident_id. Reuse
      // whatever seeding helper existing assessment/investigate tests use —
      // grep `fn seed_ai_incident` or similar in src/app/service_tests.rs
      // or src/db/*_tests.rs before writing this from scratch.
      let incident_id = crate::app::service_tests::seed_ai_incident_for_assess_tests(&pool);

      // Force CORTEX_HEADLESS_GEMINI_CMD to a stub script so this test does
      // not depend on a real Gemini CLI being installed. Follow the exact
      // pattern used by existing tests in src/assessment_tests.rs that
      // already stub the Gemini binary (grep `CORTEX_HEADLESS_GEMINI_CMD`
      // there) rather than inventing a new stub mechanism.
      let stub = crate::assessment_tests::write_stub_gemini_script(
          "echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}'",
      );
      std::env::set_var("CORTEX_HEADLESS_GEMINI_CMD", stub.path());

      let req = crate::app::AiAssessRequest {
          incident_id: incident_id.clone(),
          model: None,
          project: None,
          tool: None,
          since: None,
          until: None,
          window_minutes: None,
          correlation_window_minutes: None,
          terms: Vec::new(),
          limit: None,
      };
      let response = service.run_gemini_assess(req).await.unwrap();
      assert_eq!(response.incident_id, incident_id);

      let conn = pool.get().unwrap();
      let count: i64 = conn
          .query_row(
              "SELECT COUNT(*) FROM llm_invocations WHERE action = 'ai_assess' AND status = 'success'",
              [],
              |row| row.get(0),
          )
          .unwrap();
      assert_eq!(count, 1, "run_gemini_assess must audit through LlmRunner");

      std::env::remove_var("CORTEX_HEADLESS_GEMINI_CMD");
  }
  ```

  NOTE: This test leans on existing Gemini-stubbing test infrastructure in `src/assessment_tests.rs`. Before writing it, run `grep -n "CORTEX_HEADLESS_GEMINI_CMD\|fn.*stub" src/assessment_tests.rs` to find the real helper name(s) and adapt the test to call them correctly — do not invent `write_stub_gemini_script` if a different helper already exists; reuse it under its real name.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib app::services::assessment_tests::ai_assess_writes_llm_invocation_audit_row_via_runner -- --nocapture`
  Expected: FAIL — either a compile error (if the seeding/stub helpers need small adjustments to match actual names) or, once compiling, a runtime assertion failure (`count == 0`) because `run_gemini_assess_with_delta` still calls `run_gemini_assessment` directly, bypassing `LlmRunner` entirely, so no `llm_invocations` row exists yet.

- [ ] **Step 3: Write minimal implementation**

  Replace `src/app/services/assessment.rs` in full:

  ```rust
  use super::*;
  use crate::app::llm_runner::{LlmCallerSurface, LlmEvidenceCounts, LlmInvocationSpec};

  impl CortexService {
      pub async fn run_gemini_assess(&self, req: AiAssessRequest) -> ServiceResult<AiAssessResponse> {
          self.run_gemini_assess_with_delta(req, |_| Ok(())).await
      }

      pub async fn run_gemini_assess_with_delta<F>(
          &self,
          req: AiAssessRequest,
          mut on_delta: F,
      ) -> ServiceResult<AiAssessResponse>
      where
          F: FnMut(&str) -> anyhow::Result<()> + Send,
      {
          let incident_id = req.incident_id.clone();
          // Eng review fix (Fix 1): pass LlmRunner's own resolved timeout
          // through instead of letting GeminiAssessConfig::from_env
          // independently re-read CORTEX_LLM_COMPLETION_TIMEOUT_SECS — see
          // the eng review fix note above this task's Step 1.
          let gemini_config = GeminiAssessConfig::from_env(req.model.clone(), self.llm().timeout_secs());
          let invest_req = AiInvestigateRequest {
              incident_id: Some(incident_id.clone()),
              project: req.project.clone(),
              tool: req.tool.clone(),
              since: req.since.clone(),
              until: req.until.clone(),
              limit: Some(req.limit.unwrap_or(200).max(200)),
              window_minutes: req.window_minutes,
              correlation_window_minutes: req.correlation_window_minutes,
              terms: req.terms.clone(),
          };
          let invest_resp = self.investigate_ai_incidents(invest_req).await?;

          let matching: Vec<_> = invest_resp
              .evidence
              .iter()
              .filter(|e| e.incident.incident_id == incident_id)
              .collect();

          if matching.is_empty() {
              return Err(ServiceError::InvalidInput(format!(
                  "no incident found with id '{}'; run `cortex sessions incidents` to list available ids",
                  incident_id
              )));
          }

          let evidence_json = serde_json::to_string_pretty(&matching)
              .map_err(|e| ServiceError::Internal(anyhow::anyhow!("json serialize failed: {e}")))?;
          let prompt = build_assessment_prompt(&evidence_json);
          let prompt_preview = prompt.chars().take(500).collect::<String>();
          let evidence_summary = AiAssessEvidenceSummary {
              total_incidents: invest_resp.total_incidents,
              evidence_bundle_count: matching.len(),
              total_anchors: matching.iter().map(|e| e.anchors.len()).sum(),
          };

          // `on_delta` is `FnMut` and borrows `self`/the caller's stack, so it
          // cannot cross into the `'static` `run_fn` closure `LlmRunner::run`
          // requires. Stream deltas through a channel instead: the run_fn
          // task forwards each parsed delta line, and this function drains
          // the channel concurrently with awaiting the run.
          let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
          let gemini_config_owned = gemini_config.clone();
          let run_fut = self.llm().run(
              LlmInvocationSpec {
                  caller_surface: LlmCallerSurface::Cli, // overridden per-caller in a later phase task if MCP/REST also call assess directly; ai_assess is CLI-only today per dispatch_sessions.rs's CliMode::Http bail.
                  action: "ai_assess".to_string(),
                  incident_id: Some(incident_id.clone()),
                  ai_tool: req.tool.clone(),
                  ai_project: req.project.clone(),
                  ai_session_id: None,
                  evidence_counts: LlmEvidenceCounts {
                      total_incidents: evidence_summary.total_incidents,
                      evidence_bundle_count: evidence_summary.evidence_bundle_count,
                      total_anchors: evidence_summary.total_anchors,
                      truncated: invest_resp.truncated,
                  },
                  prompt: prompt.clone(),
                  provider: "gemini-cli".to_string(),
                  model: gemini_config_owned.model.clone(),
                  program: gemini_config_owned.program.clone(),
                  extra_metadata: serde_json::json!({}),
              },
              move |prompt| async move {
                  run_gemini_assessment(&prompt, &gemini_config_owned, move |delta: &str| {
                      let _ = delta_tx.send(delta.to_string());
                      Ok(())
                  })
                  .await
              },
          );
          tokio::pin!(run_fut);

          let assessment = loop {
              tokio::select! {
                  biased;
                  Some(delta) = delta_rx.recv() => {
                      on_delta(&delta).map_err(ServiceError::Internal)?;
                  }
                  result = &mut run_fut => {
                      // Drain any remaining buffered deltas before returning.
                      while let Ok(delta) = delta_rx.try_recv() {
                          on_delta(&delta).map_err(ServiceError::Internal)?;
                      }
                      break result;
                  }
              }
          }
          .map_err(|err| ServiceError::Internal(anyhow::anyhow!(err)))?
          .output;

          Ok(AiAssessResponse {
              incident_id,
              assessment,
              prompt_preview,
              evidence_summary,
          })
      }
  }
  ```

  Add `#[derive(Debug, Clone)]` to `GeminiAssessConfig` in `src/assessment.rs` if it does not already derive `Clone` (it currently does: `#[derive(Debug, Clone)]` is already present at line 42 per the file read earlier — no change needed there).

  Add the sidecar test hook at the bottom of `src/app/services/assessment.rs`:

  ```rust
  #[cfg(test)]
  #[path = "assessment_tests.rs"]
  mod tests;
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib app::services::assessment_tests:: assessment_tests::from_env_uses_the_passed_timeout_not_the_legacy_env_var cli::dispatch_sessions_tests::`
  Expected: PASS — the new audit test and the new single-source-of-truth timeout test both pass, and ALL pre-existing `dispatch_sessions_tests` covering `run_ai_assess` (CLI layer, unaffected signature) and `assessment_tests` (now updated call sites) continue to pass unchanged.

- [ ] **Step 5: Commit**
  ```bash
  git add src/app/services/assessment.rs src/app/services/assessment_tests.rs \
          src/assessment.rs src/assessment_tests.rs
  git commit -m "feat: migrate cortex sessions assess to route through LlmRunner

Also unifies the Gemini subprocess timeout with [llm].timeout_secs
(eng review Fix 1) so CORTEX_LLM_COMPLETION_TIMEOUT_SECS and
[llm].timeout_secs can no longer silently disagree."
  ```

---

## Task 7: New read surfaces — CLI `sessions llm-invocations`, MCP `llm_invocations` action, REST `GET /api/sessions/llm-invocations`

**Files:**
- Modify: `src/app/models/ops.rs` (add `LlmInvocationsRequest`/response type near `NotificationsRecentRequest` at line ~84)
- Modify: `src/app/services/rag.rs` (add `CortexService::llm_invocations_checked`, following `notifications_recent_checked` at line 20)
- Modify: `src/mcp/actions.rs` (add `LlmInvocations` to `ActionHandler` enum at line ~94, add an `action_spec!("llm_invocations", Read, ..., Cheap, LlmInvocations)` row near `notifications_recent` at line ~473-479)
- Modify: `src/mcp/tools.rs` (add `H::LlmInvocations => tool_llm_invocations(state, args).await,` near line 116, and a `tool_llm_invocations` fn near `tool_notifications_recent` at line ~536)
- Modify: `src/api.rs` (add `.route("/api/sessions/llm-invocations", get(ai_llm_invocations))` near line 265, plus the handler fn near `notifications_recent` at line ~791, plus add `LlmInvocationsRequest` to the `use crate::app::{...}` import block at line ~30-44)
- Modify: `src/cli/args/sessions.rs` (add `LlmInvocations(SessionsLlmInvocationsArgs)` to `SessionsCommand` enum at line ~25, add `SessionsLlmInvocationsArgs` struct)
- Modify: `src/cli/parse/sessions.rs` (add `"llm-invocations"` to `SESSIONS_SUBCOMMANDS` at line ~45, add `"llm-invocations" => parse_sessions_llm_invocations(rest),` dispatch arm at line ~81)
- Create/Modify: `src/cli/parse/sessions/more.rs` (add `parse_sessions_llm_invocations` fn, following `parse_sessions_similar` at line ~11)
- Modify: `src/cli/dispatch_sessions.rs` (add `run_ai_llm_invocations` fn, following `run_ai_investigate` at line ~475)
- Modify: `src/cli/run.rs` (add `super::SessionsCommand::LlmInvocations(args) => dispatch::run_ai_llm_invocations(&mode, args).await,` at line ~141, right after the `Assess` arm)
- Modify: `src/cli/http_client.rs` (add `pub async fn ai_llm_invocations(&self, req: &LlmInvocationsRequest) -> Result<Vec<LlmInvocationRow>>`, following `ai_investigate` at line ~555 — **eng review Fix 4:** also add a new `get_json_with_admin` helper alongside `post_json_with_admin_no_retry`, since `llm_invocations` is the first admin-gated GET route and no GET+admin-token helper exists yet)
- Modify: `src/cli/args.rs` (add `SessionsLlmInvocationsArgs` to the re-export `use` block at line ~13)
- Test: `src/cli/dispatch_sessions_tests.rs`, `src/mcp` tool-dispatch tests (find the existing test file covering `tool_notifications_recent` via `grep -rn "tool_notifications_recent\|notifications_recent" src/mcp/*_tests.rs` and add alongside it), `src/api.rs`'s own test module (check `grep -n '#\[cfg(test)\]' src/api.rs` for its sidecar or inline test location)

**Interfaces:**
- Consumes: `CortexService::llm(&self)` is NOT used here — this task reads via `crate::db::llm_invocations::list_llm_invocations` (Task 4) through a new `CortexService::llm_invocations_checked` service method (mirrors `notifications_recent_checked`), NOT through `LlmRunner` (the runner is for invoking the LLM, not for reads).
- Produces: `pub struct LlmInvocationsRequest { pub limit: Option<i64>, pub since: Option<String>, pub action: Option<String>, pub status: Option<String> }` in `src/app/models/ops.rs`; `CortexService::llm_invocations_checked(&self, req: LlmInvocationsRequest) -> ServiceResult<Vec<crate::db::llm_invocations::LlmInvocationRow>>`. No later phase depends on this task's output (it's a terminal read surface), so exact wire format matters more for human/agent consumers than for other Rust code. **Eng review fix (Fix 4 — security reviewer V2):** the MCP `llm_invocations` action is scoped `cortex:admin` (not `cortex:read`) and the REST route is gated by `require_api_admin_token`/`X-Cortex-Admin-Token` — `CortexService::llm_invocations_checked` itself carries no scope gate (scope gating is an MCP/REST transport-layer concern, matching how the other admin actions' service methods are also ungated at the service layer), so `CliMode::Local` callers are unaffected but `CliMode::Http` requires `CORTEX_API_ADMIN_TOKEN`.

- [ ] **Step 1: Write the failing test**

  Add to `src/app/service_tests.rs`:

  ```rust
  #[tokio::test]
  async fn llm_invocations_checked_returns_recent_rows_filtered() {
      let (service, pool, _dir) = test_service();
      let conn = pool.get().unwrap();
      let params = crate::db::llm_invocations::LlmInvocationInsertParams {
          caller_surface: "test".to_string(),
          action: "ai_assess".to_string(),
          provider: "gemini-cli".to_string(),
          model: Some("m".to_string()),
          program: Some("gemini".to_string()),
          incident_id: None,
          ai_tool: None,
          ai_project: None,
          ai_session_id: None,
          evidence_counts_json: None,
          prompt_bytes: Some(10),
          status: "running".to_string(),
          metadata_json: None,
      };
      crate::db::llm_invocations::insert_llm_invocation_running(&conn, "llm-x", &params).unwrap();
      crate::db::llm_invocations::finish_llm_invocation(&conn, "llm-x", "success", None, 5, Some(20))
          .unwrap();
      drop(conn);

      let rows = service
          .llm_invocations_checked(crate::app::LlmInvocationsRequest {
              limit: Some(10),
              since: None,
              action: Some("ai_assess".to_string()),
              status: None,
          })
          .await
          .unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].id, "llm-x");
  }
  ```

  Add to `src/cli/dispatch_sessions_tests.rs` (check its existing imports/style first via `grep -n "^use\|fn run_ai_investigate_test\|#\[tokio::test\]" src/cli/dispatch_sessions_tests.rs`, adapt names accordingly):

  ```rust
  #[tokio::test]
  async fn parses_llm_invocations_flags() {
      let parsed = crate::cli::parse::parse_args(&[
          "sessions".to_string(),
          "llm-invocations".to_string(),
          "--since".to_string(),
          "24h".to_string(),
          "--limit".to_string(),
          "50".to_string(),
          "--json".to_string(),
      ])
      .unwrap();
      match parsed {
          crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::LlmInvocations(args)) => {
              assert_eq!(args.limit, Some(50));
              assert!(args.json);
              assert!(args.since.is_some());
          }
          other => panic!("expected SessionsCommand::LlmInvocations, got {other:?}"),
      }
  }
  ```

  NOTE: `parse_args`/`CliCommand` entry point name may differ — `grep -n "pub fn parse_args\|pub(crate) fn parse" src/cli/parse.rs` first and use the real top-level parse entry point (it may require a leading `"cortex".to_string()` argv[0] or may take `&[String]` starting from argv[1] — check an existing test in `src/cli/parse_tests.rs` for the exact calling convention before writing this test).

  **Eng review fix (Fix 4 — security reviewer, MP2): explicit MCP scope-enforcement test.** The generic parametrized tests in `src/mcp/rmcp_server_tests.rs` (`mounted_policy_with_read_scope_permits_read_actions` / the admin-denial loop inside it, and `public_read_actions_require_cortex_read_scope`) already iterate `actions::ACTION_SPECS` filtered by `required_scope_for(action) == Some("cortex:admin")`, so marking `llm_invocations`'s `action_spec!` row `Admin` (done above) automatically sweeps it into their existing assertions — no change needed to those two tests themselves. Add ONE additional, explicitly-named test to `src/mcp/rmcp_server_tests.rs` (not the generic loop) so a reviewer/future refactor can see `llm_invocations` scope enforcement called out by name, mirroring how this repo already has an explicit `sessions_action_requires_read_scope` test alongside its generic loop:

  ```rust
  // src/mcp/rmcp_server_tests.rs — add near sessions_action_requires_read_scope.
  // Uses the exact same helpers as the existing generic admin-denial loop in
  // mounted_policy_with_read_scope_permits_read_actions (mounted_state(),
  // auth_ctx_with_scopes, rmcp_router_with_auth, post_rmcp, jsonrpc_request) —
  // grep `grep -n "fn mounted_state\|fn rmcp_router_with_auth\|fn auth_ctx_with_scopes\|fn seed_auth_action_log" src/mcp/rmcp_server_tests.rs` first to confirm signatures before pasting, since this file's exact test-setup helpers may have shifted since this plan was drafted.
  #[test]
  fn llm_invocations_action_requires_admin_scope() {
      assert_eq!(
          required_scope_for("llm_invocations"),
          Some("cortex:admin"),
          "llm_invocations exposes circuit-breaker/kill-switch operational \
           state and must be admin-scoped, not cortex:read (eng review Fix 4)"
      );
  }

  #[tokio::test]
  async fn llm_invocations_action_is_denied_for_read_only_scope() {
      let (state, pool, _dir) = mounted_state();
      seed_auth_action_log(&pool);
      let auth = auth_ctx_with_scopes(vec!["cortex:read"]);
      let router = rmcp_router_with_auth(state, auth);

      let (status, response) = post_rmcp(
          router,
          jsonrpc_request(
              30,
              "tools/call",
              Some(json!({"name": "cortex", "arguments": {"action": "llm_invocations"}})),
          ),
      )
      .await;
      assert_eq!(status, StatusCode::OK);
      assert_eq!(
          response["error"]["code"], -32600,
          "llm_invocations must be denied for a cortex:read-only caller; response: {response}"
      );
      let msg = response["error"]["message"].as_str().unwrap_or("");
      assert!(
          msg.contains("requires scope: cortex:admin"),
          "denial message should reference admin scope; got: {msg}"
      );
  }
  ```

  These two tests are additive — `llm_invocations` also gets automatically swept into the pre-existing generic loops in `mounted_policy_with_read_scope_permits_read_actions` (the admin-denial half) and `public_read_actions_require_cortex_read_scope` purely by virtue of its `action_spec!` row being `Admin`; no edits to those two tests are needed.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib app::service_tests::llm_invocations_checked_returns_recent_rows_filtered cli::dispatch_sessions_tests::parses_llm_invocations_flags mcp::rmcp_server_tests::llm_invocations_action_requires_admin_scope mcp::rmcp_server_tests::llm_invocations_action_is_denied_for_read_only_scope -- --nocapture`
  Expected: FAIL with compile errors — `LlmInvocationsRequest`, `llm_invocations_checked`, `SessionsCommand::LlmInvocations`, and `required_scope_for("llm_invocations")` (returns `None`/`Some("cortex:__deny__")` since the action doesn't exist yet) all unresolved or failing.

- [ ] **Step 3: Write minimal implementation**

  In `src/app/models/ops.rs`, add after `NotificationsRecentRequest`'s `impl` block (line ~94):

  ```rust
  #[derive(Debug, Clone, Default, Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct LlmInvocationsRequest {
      pub limit: Option<i64>,
      pub since: Option<String>,
      pub action: Option<String>,
      pub status: Option<String>,
  }

  impl LlmInvocationsRequest {
      pub fn effective_limit(&self) -> i64 {
          self.limit.unwrap_or(50).clamp(1, 500)
      }
  }
  ```

  Ensure `LlmInvocationsRequest` is re-exported from wherever `NotificationsRecentRequest` is (check `src/app.rs` or `src/app/models.rs` for the `pub use models::ops::{..., NotificationsRecentRequest, ...};` line and add `LlmInvocationsRequest` to the same list).

  In `src/app/services/rag.rs`, add after `notifications_recent_checked`:

  ```rust
  pub async fn llm_invocations_checked(
      &self,
      req: LlmInvocationsRequest,
  ) -> ServiceResult<Vec<crate::db::llm_invocations::LlmInvocationRow>> {
      let limit = req.effective_limit();
      self.run_db("llm_invocations", move |pool| {
          let conn = pool.get()?;
          crate::db::llm_invocations::list_llm_invocations(
              &conn,
              limit,
              req.since.as_deref(),
              req.action.as_deref(),
              req.status.as_deref(),
          )
          .map_err(anyhow::Error::from)
      })
      .await
  }
  ```

  (Add `LlmInvocationsRequest` to the `use super::*;`/explicit import list at the top of `rag.rs` if it uses explicit imports rather than a glob — check the top of the file first.)

  **Eng review fix (Fix 4 — security reviewer V2):** `llm_invocations` exposes `status`/`error`/`metadata_json` fields that reveal circuit-breaker state, kill-switch state, exact rate-limit thresholds, and host/pid — an operational side-channel this repo's trust model does not treat as `cortex:read`-safe (per `CLAUDE.md`: "MCP endpoint is unauthenticated by default... any client reaching port 3100 has full log read access" absent `CORTEX_TOKEN`). Scope this action `Admin`, not `Read`, mirroring the repo's existing 4 admin actions (`ack_error`, `unack_error`, `file_tails`, `notifications_test`).

  In `src/mcp/actions.rs`, add `LlmInvocations` to the `ActionHandler` enum (alphabetically near `NotificationsRecent`, line ~94):

  ```rust
      NotificationsRecent,
      LlmInvocations,
  ```

  and add the action spec row right after `notifications_recent`'s (line ~479) — **scoped `Admin`, matching `ack_error`/`unack_error`/`file_tails`/`notifications_test`, NOT `Read`:**

  ```rust
      action_spec!(
          "llm_invocations",
          Admin,
          "Recent LLM invocation audit records (concurrency/rate-limit/circuit-breaker denials included) — admin-scoped: exposes operational kill-switch/circuit-breaker state",
          Cheap,
          LlmInvocations
      ),
  ```

  In `src/mcp/tools.rs`, add the dispatch arm right after `NotificationsRecent` (line ~116):

  ```rust
          H::NotificationsRecent => tool_notifications_recent(state, args).await,
          H::LlmInvocations => tool_llm_invocations(state, args).await,
  ```

  and the handler fn right after `tool_notifications_recent` (line ~540):

  ```rust
  async fn tool_llm_invocations(state: &AppState, args: Value) -> anyhow::Result<Value> {
      let req: crate::app::LlmInvocationsRequest = action_payload(args, "llm_invocations")?;
      let rows = state.service.llm_invocations_checked(req).await?;
      Ok(serde_json::to_value(rows)?)
  }
  ```

  (Add `LlmInvocationsRequest` to the `use crate::app::{...}` import block at the top of `tools.rs`, line ~22-28, alongside `NotificationsRecentRequest`.) No further gating code is needed in `tools.rs`/`rmcp_server.rs` itself — scope enforcement is driven generically off `ActionSpec.scope` via `actions::required_scope_for(action)` (see `src/mcp/rmcp_server.rs`'s existing fail-closed scope check, already exercised by the parametrized admin-action tests in `src/mcp/rmcp_server_tests.rs`), so marking this row `Admin` is sufficient — the same mechanism that already denies `cortex:read`-only callers from `ack_error`/`unack_error`/`file_tails`/`notifications_test` automatically covers `llm_invocations` too.

  **Eng review fix (Fix 4, REST layer):** this repo's REST layer DOES have an admin-vs-read distinction — `require_api_admin_token(&state, &headers)` (checked against `CORTEX_API_ADMIN_TOKEN` / the `X-Cortex-Admin-Token` header), already used by every admin POST route (`ack_error`, `unack_error`, `db_checkpoint`, `db_vacuum`, `db_backup`, `prune_ai_checkpoints`). `GET /api/sessions/llm-invocations` is the first ADMIN-gated GET route in this file — mirror the existing POST admin routes' gate check, adapted for a GET+`Query` handler:

  In `src/api.rs`, add `LlmInvocationsRequest` to the import block (line ~40, alongside `NotificationsRecentRequest`), add the route (line ~265, grouped with the other `/api/sessions/*` GET routes):

  ```rust
          .route("/api/sessions/llm-invocations", get(ai_llm_invocations))
  ```

  and the handler (right after `notifications_recent`, line ~796) — **gated by `require_api_admin_token`, unlike the plain `respond(...)`-only `notifications_recent` handler above it:**

  ```rust
  async fn ai_llm_invocations(
      State(state): State<ApiState>,
      ConnectInfo(peer): ConnectInfo<SocketAddr>,
      headers: HeaderMap,
      Query(req): Query<LlmInvocationsRequest>,
  ) -> axum::response::Response {
      if let Some(resp) = require_api_admin_token(&state, &headers) {
          return resp;
      }
      tracing::warn!(caller_ip = %peer.ip(), "admin: llm_invocations invoked");
      respond(state.service.llm_invocations_checked(req).await)
  }
  ```

  (`ConnectInfo<SocketAddr>` and `HeaderMap` extractors, and the `require_api_admin_token`/`tracing::warn!` pattern, are copied verbatim from the existing `ack_error`/`db_checkpoint` admin handlers in this same file — grep `grep -n "ConnectInfo<SocketAddr>" src/api.rs` first to confirm the exact import path already in scope, no new import should be needed since other admin handlers in this file already use it.)

  In `src/cli/args/sessions.rs`, add to `SessionsCommand` (line ~25, right after `Assess`):

  ```rust
      LlmInvocations(SessionsLlmInvocationsArgs),
  ```

  and a new args struct (near `SessionsSimilarArgs` or `SessionsAskHistoryArgs` — place alongside similarly-shaped structs):

  ```rust
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub(crate) struct SessionsLlmInvocationsArgs {
      pub since: Option<String>,
      pub action: Option<String>,
      pub status: Option<String>,
      pub limit: Option<i64>,
      pub json: bool,
  }

  impl SessionsLlmInvocationsArgs {
      pub(crate) fn into_request(self) -> crate::app::LlmInvocationsRequest {
          crate::app::LlmInvocationsRequest {
              limit: self.limit,
              since: self.since,
              action: self.action,
              status: self.status,
          }
      }
  }
  ```

  In `src/cli/args.rs`, add `SessionsLlmInvocationsArgs` to the existing re-export `use` block (line ~13, alongside `SessionsAssessArgs`).

  In `src/cli/parse/sessions.rs`, add `"llm-invocations"` to `SESSIONS_SUBCOMMANDS` (line ~45, after `"assess"`) and the dispatch arm (line ~81, after the `"assess"` arm):

  ```rust
          "llm-invocations" => parse_sessions_llm_invocations(rest),
  ```

  In `src/cli/parse/sessions/more.rs`, add (following the exact `parse_sessions_similar` flag-parsing style at line ~11):

  ```rust
  pub(crate) fn parse_sessions_llm_invocations(args: &[String]) -> Result<CliCommand> {
      let mut parsed = SessionsLlmInvocationsArgs::default();
      let mut flags = FlagCursor::new(args);
      while let Some(arg) = flags.next() {
          match arg.as_str() {
              "--json" => parsed.json = true,
              "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
              "--action" => parsed.action = Some(flags.value("--action")?),
              "--status" => parsed.status = Some(flags.value("--status")?),
              "--limit" => {
                  let raw = flags.value("--limit")?;
                  parsed.limit = Some(
                      raw.parse::<i64>()
                          .map_err(|_| anyhow::anyhow!("--limit must be an integer, got '{raw}'"))?,
                  );
              }
              _ if arg.starts_with("--since=") => {
                  parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
              }
              _ if arg.starts_with("--action=") => {
                  parsed.action = Some(value_after_equals(arg, "--action")?)
              }
              _ if arg.starts_with("--status=") => {
                  parsed.status = Some(value_after_equals(arg, "--status")?)
              }
              _ if arg.starts_with("--limit=") => {
                  let raw = value_after_equals(arg, "--limit")?;
                  parsed.limit = Some(
                      raw.parse::<i64>()
                          .map_err(|_| anyhow::anyhow!("--limit must be an integer, got '{raw}'"))?,
                  );
              }
              _ => bail!("unknown flag for sessions llm-invocations: {arg}"),
          }
      }
      Ok(CliCommand::Sessions(SessionsCommand::LlmInvocations(parsed)))
  }
  ```

  (Add `parse_sessions_llm_invocations` to the `use self::more::{...}` import list at the top of `src/cli/parse/sessions.rs`, line ~4, and add `SessionsLlmInvocationsArgs` to the `use super::super::{...}` import list at line ~16.)

  In `src/cli/dispatch_sessions.rs`, add after `run_ai_investigate` (line ~491). **Eng review fix (Fix 4):** `llm_invocations` is admin-scoped end to end (MCP `cortex:admin`, REST `X-Cortex-Admin-Token`) — in `CliMode::Local` this is transparent (the local service call has no scope gate, matching how other admin actions behave when run in-process), but `CliMode::Http` now requires `CORTEX_API_ADMIN_TOKEN` to be set client-side (see the `get_json_with_admin` change above) or the call fails with a clear error from `get_json_with_admin`'s `ok_or_else`. Document this in the doc comment:

  ```rust
  /// `cortex sessions llm-invocations` — list recent LLM invocation audit
  /// records (concurrency/rate-limit/circuit-breaker denials included).
  ///
  /// Admin-scoped: exposes operational kill-switch/circuit-breaker state,
  /// not just log content. In `CliMode::Http`, requires
  /// `CORTEX_API_ADMIN_TOKEN` to be set — the request fails with a clear
  /// error otherwise (see eng review Fix 4).
  pub(crate) async fn run_ai_llm_invocations(
      mode: &CliMode,
      args: SessionsLlmInvocationsArgs,
  ) -> Result<()> {
      let json = args.json;
      let req = args.into_request();
      let response = match mode {
          CliMode::Local(service) => service.llm_invocations_checked(req).await?,
          CliMode::Http(client) => http_or_cancel(client.ai_llm_invocations(&req)).await?,
      };
      if json {
          println!("{}", serde_json::to_string_pretty(&response)?);
      } else if response.is_empty() {
          println!("No LLM invocations recorded.");
      } else {
          for row in &response {
              println!(
                  "[{}] {} action={} status={} duration_ms={}",
                  row.started_at,
                  row.id,
                  row.action,
                  row.status,
                  row.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "-".to_string()),
              );
          }
      }
      Ok(())
  }
  ```

  In `src/cli/run.rs`, add after the `Assess` arm (line ~141):

  ```rust
              super::SessionsCommand::LlmInvocations(args) => {
                  dispatch::run_ai_llm_invocations(&mode, args).await
              }
  ```

  **Eng review fix (Fix 4, CLI HTTP-mode client):** `GET /api/sessions/llm-invocations` is admin-gated (see the REST handler above), but the existing `get_json` helper in `http_client.rs` sends no admin token — it's only ever been used for `cortex:read`-tier GET routes. There is no existing GET+admin-token helper in this file (the only admin-token helper, `post_json_with_admin_no_retry`, is POST-only, used by `ack_error`/`unack_error`). Add a GET counterpart rather than reusing the read-only `get_json`:

  ```rust
  // In src/cli/http_client.rs, add near post_json_with_admin_no_retry
  // (same file, same impl block) — GET counterpart for admin-gated reads.
  async fn get_json_with_admin<Req, Resp>(&self, path: &str, req: Option<&Req>) -> Result<Resp>
  where
      Req: Serialize + ?Sized,
      Resp: DeserializeOwned,
  {
      let token = self.api_admin_token.as_deref().ok_or_else(|| {
          anyhow!("CORTEX_API_ADMIN_TOKEN is required for this HTTP API read")
      })?;
      let mut admin_value =
          HeaderValue::from_str(token).context("failed to construct admin token header")?;
      admin_value.set_sensitive(true);
      let admin_header = HeaderName::from_static("x-cortex-admin-token");
      let url = self.url(path)?;
      let send = || async {
          let mut builder = self
              .inner
              .request(Method::GET, url.clone())
              .header(admin_header.clone(), admin_value.clone());
          if let Some(r) = req {
              builder = builder.query(r);
          }
          builder.send().await
      };
      self.execute_once(send, path).await
  }
  ```

  Then add, after `ai_investigate` (line ~563), using the new admin-gated GET helper instead of the plain `get_json`:

  ```rust
      pub async fn ai_llm_invocations(
          &self,
          req: &crate::app::LlmInvocationsRequest,
      ) -> Result<Vec<crate::db::llm_invocations::LlmInvocationRow>> {
          self.get_json_with_admin("/api/sessions/llm-invocations", Some(req)).await
      }
  ```

  This means `cortex sessions llm-invocations` in HTTP-client mode requires `CORTEX_API_ADMIN_TOKEN` to be set client-side, exactly like `cortex sessions ack-error`/`unack-error` already do — update `run_ai_llm_invocations`'s CLI help text / `--help` output (Task 8) to note this requirement.

  Add `run_ai_llm_invocations` to the `use ... dispatch::{run_ai_abuse, run_ai_add, ...}` import list in `src/cli/dispatch.rs` (line ~368) if `run.rs` imports dispatch functions from there rather than qualifying with `dispatch::` inline (check which pattern `run.rs` actually uses — the excerpt already shown uses `dispatch::run_ai_assess` fully qualified, so likely NO import-list change is needed; only touch `dispatch.rs`'s import list if `cargo build` reports an unresolved import).

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib app::service_tests::llm_invocations_checked_returns_recent_rows_filtered cli::dispatch_sessions_tests::parses_llm_invocations_flags mcp::rmcp_server_tests::llm_invocations_action_requires_admin_scope mcp::rmcp_server_tests::llm_invocations_action_is_denied_for_read_only_scope`
  Expected: PASS — including the two new eng-review-fix scope tests (Fix 4). Then run the full workspace test suite once to catch any missed wiring: `cargo test --lib 2>&1 | tail -60` — expect PASS (0 failures); if `cargo build` reports unresolved imports (e.g. missing entries in a `use` block for `LlmInvocationsRequest` or `SessionsLlmInvocationsArgs` in a file not explicitly listed above), fix those imports following the same pattern as the sibling type already imported in that file. Also confirm the pre-existing generic scope loops (`mounted_policy_with_read_scope_permits_read_actions`, `public_read_actions_require_cortex_read_scope`) still pass with `llm_invocations` now present in `ACTION_SPECS` — they should, since both are written generically over `ACTION_SPECS` and require no per-action edits.

- [ ] **Step 5: Commit**
  ```bash
  git add src/app/models/ops.rs src/app/services/rag.rs src/mcp/actions.rs src/mcp/tools.rs \
          src/mcp/rmcp_server_tests.rs \
          src/api.rs src/cli/args/sessions.rs src/cli/args.rs src/cli/parse/sessions.rs \
          src/cli/parse/sessions/more.rs src/cli/dispatch_sessions.rs src/cli/run.rs \
          src/cli/http_client.rs src/app/service_tests.rs src/cli/dispatch_sessions_tests.rs
  git commit -m "feat: add llm_invocations CLI/MCP/REST read surfaces (admin-scoped)

llm_invocations is scoped cortex:admin (not cortex:read) because it exposes
circuit-breaker/kill-switch/rate-limit operational state; the REST route
is gated by the existing X-Cortex-Admin-Token mechanism (eng review Fix 4)."
  ```

---

## Task 8: Documentation updates

**Files:**
- Modify: `README.md` (find the MCP action count / action table — likely near a "47 actions" mention; bump to 48 and add `llm_invocations` row)
- Modify: `docs/api.md` (route matrix — add `GET /api/sessions/llm-invocations`; bump the route count comment in `src/api.rs`'s own doc header too, e.g. "57 routes" → "58 routes" at `src/api.rs:4`)
- Modify: `docs/mcp/TOOLS.md` (action list)
- Modify: `docs/mcp/SCHEMA.md` (if it enumerates action names/schemas)
- Modify: `docs/contracts/mcp-actions-current.md` (authoritative action contract snapshot)
- Modify: `/home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md` (the project CLAUDE.md action table under "## MCP Tools" — add a row for `llm_invocations`, and note "48 actions" instead of "47 actions" in the "## MCP Tools" intro sentence)
- Modify: `docs/CONFIG.md` (document the new `[llm]` config section, mirroring how `[notifications]`/`[error_detection]` are documented there)
- Modify: `CHANGELOG.md` (new entry — this phase's changes bump the version per the repo's mandatory version-bump convention; see Task 9)

**Interfaces:**
- Consumes: nothing new — this task only documents the surfaces Tasks 1-7 already built. No code interfaces produced.
- Produces: nothing consumable by other tasks; purely descriptive.

- [ ] **Step 1: Write the failing test**

  Documentation has no automated test in this repo beyond `cargo xtask check-release-versions` (which checks CHANGELOG presence, covered by Task 9) and any doc-drift lint. Skip the red/green cycle for this task; instead use a grep-based verification step in place of Step 1/Step 2:

  Run: `grep -rn "notifications_recent" README.md docs/api.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/contracts/mcp-actions-current.md docs/CONFIG.md` to find every location that documents a comparable existing action/config-section, so the new `llm_invocations` entries land in the same files at the same structural position.

- [ ] **Step 2: (informational — confirm the audit locations)**

  Expected: the grep above returns at least one hit per file (if a file has zero hits, that file does not enumerate individual actions and should be skipped for the action-list edit, but still gets the count bump if it mentions "47 actions").

- [ ] **Step 3: Write the documentation edits**

  For each file found in Step 1, add a row/entry for `llm_invocations` structurally identical to the `notifications_recent` entry (same table columns, same heading level). Also:

  - In `README.md` and `/home/jmagar/workspace/cortex/.claude/worktrees/happy-kepler-2d8fa5/CLAUDE.md`, run `grep -n "47 action\|47 MCP action" README.md CLAUDE.md` and change every `47` count to `48` at those exact locations. Add this row to CLAUDE.md's action table (matching the existing table's exact column format — `| Action | Description |`):
    ```markdown
    | `llm_invocations` | **(admin)** Recent LLM invocation audit records (concurrency/rate-limit/circuit-breaker denials included) |
    ```
    Place it near `notifications_recent`'s row for locality with the other observability-ish actions. **Eng review fix (Fix 4):** also update the "Scope taxonomy" sentence in CLAUDE.md's "## MCP Tools" section (the one currently listing the four admin actions `ack_error`, `unack_error`, `file_tails`, `notifications_test`) to add `llm_invocations` as a fifth admin action — e.g. change "the four **admin** actions `ack_error`, `unack_error`, `file_tails`, and `notifications_test`" to "the five **admin** actions `ack_error`, `unack_error`, `file_tails`, `notifications_test`, and `llm_invocations`".

  - In `docs/CONFIG.md`, add a new `### [llm]` section documenting every field from the "Required config shape" table in this plan's brief (enabled, max_concurrent, max_per_action_concurrent, max_invocations_per_minute, max_invocations_per_hour, failure_threshold, cooldown_secs, timeout_secs, max_prompt_bytes, max_output_bytes, background_enrichment_enabled, and the `[llm.actions.<name>]` sub-tables), following whatever heading/table style `docs/CONFIG.md` uses for `[notifications]` or `[error_detection]`.

  - **Eng review fix (Fix 1 — architecture reviewer, user-visible behavior change callout):** in `docs/CONFIG.md`'s new `[llm]` section AND in the `CHANGELOG.md` entry (Task 9), explicitly call out that `cortex sessions assess` now enforces `max_concurrent=1` / `max_per_action_concurrent=1` by default (the `LlmConfig` defaults — see Task 2). Concretely: **two overlapping interactive `cortex sessions assess` invocations will now hard-fail the second with a `ConcurrencyLimited`/`ActionConcurrencyLimited` error, where previously (calling `run_gemini_assessment` directly with no guard) they ran concurrently.** This is a real regression risk for any operator/script that fires off concurrent assess calls today — add this as its own `docs/CONFIG.md` callout, worded along these lines:

    ```markdown
    > **Behavior change:** prior to this release, `cortex sessions assess`
    > had no concurrency guard — multiple overlapping invocations ran in
    > parallel. As of this release it routes through `LlmRunner`, whose
    > defaults (`max_concurrent=1`, `max_per_action_concurrent=1`) mean a
    > second concurrent `assess` call is now REJECTED with a concurrency-
    > limited error instead of running alongside the first. Raise
    > `[llm].max_concurrent` / `[llm].max_per_action_concurrent` if your
    > workflow depends on concurrent assessments.
    ```

  - In `docs/api.md`, add the `GET /api/sessions/llm-invocations` row to the route matrix table (query params: `limit`, `since`, `action`, `status`; response: array of `LlmInvocationRow`) — **and note it requires `cortex:admin`** (see Fix 4 in "Eng Review Fixes Applied" and Task 7's updated scope).

  - In `src/api.rs`'s module doc comment (line 4), bump the route count.

  - In `src/db/pool.rs`'s module doc comment (already updated in Task 1 Step 3 to say 37 migrations — verify it's consistent, no further change needed here).

- [ ] **Step 4: Run test to verify it passes**

  Run: `grep -rn "llm_invocations" README.md docs/api.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/contracts/mcp-actions-current.md docs/CONFIG.md CLAUDE.md`
  Expected: at least one match per file (confirms every doc file was actually touched, not skipped).

- [ ] **Step 5: Commit**
  ```bash
  git add README.md docs/api.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md \
          docs/contracts/mcp-actions-current.md docs/CONFIG.md CLAUDE.md src/api.rs
  git commit -m "docs: document llm_invocations action, REST route, and [llm] config section"
  ```

---

## Task 9: Version bump + CHANGELOG entry

**Files:**
- Modify: `Cargo.toml` (`[package] version`), `Cargo.lock` (the `cortex` entry), `server.json`, `mcpb/manifest.json`, `docker-compose.prod.yml`, `CHANGELOG.md` — all via the repo's `cargo xtask` tool, per this repo's mandatory "every feature branch push MUST bump the version" rule (see project CLAUDE.md "Version Bumping" section).

**Interfaces:**
- Consumes: nothing from earlier tasks except that all of Tasks 1-8's changes must already be committed (this is intentionally the last task in the phase).
- Produces: nothing consumable by other tasks — this is a release-mechanics task, always last.

- [ ] **Step 1: Write the failing test**

  Run: `cargo xtask check-version-sync`
  Expected: at this point (before bumping) it should still PASS trivially since no version-bearing file was touched by Tasks 1-8 — so instead, verify the CHANGELOG gate fails:

  Run: `cargo xtask check-release-versions`
  Expected: FAIL — no new CHANGELOG.md entry exists yet for a version that reflects this phase's `feat:` commits (Tasks 1, 3, 4, 5, 6, 7 all used `feat:` prefixes, so this phase requires a **minor** bump per the repo's commit-prefix-to-bump-type rule).

- [ ] **Step 2: Confirm the failure**

  Run: `cargo xtask check-release-versions 2>&1 | tail -20`
  Expected output contains an error indicating the current `Cargo.toml` version has no matching `CHANGELOG.md` entry, or that version-bearing files are out of sync.

- [ ] **Step 3: Bump and document**

  Run:
  ```bash
  cargo xtask bump-version minor
  ```

  This rewrites `Cargo.toml`, `Cargo.lock` (cortex entry), `server.json`, `mcpb/manifest.json`, and `docker-compose.prod.yml` in place. Then manually add the `CHANGELOG.md` entry (xtask does not author changelog prose) under the new version heading, e.g.:

  ```markdown
  ## [3.2.0] - 2026-07-01

  ### Added
  - Shared `LlmRunner` invocation guard for all LLM-backed assessment features: global/per-action concurrency limits, per-action rate limiting, cooldown circuit breaker, per-invocation timeout, prompt/output byte caps, dry-run preview mode, and a global + per-action kill switch (`[llm]` config section, `CORTEX_LLM_ENABLED` env override).
  - `llm_invocations` audit table (migration 37) recording every LLM invocation attempt, including denials (rate-limited, circuit-open, disabled, concurrency-limited).
  - New read surfaces for the audit trail: CLI `cortex sessions llm-invocations`, MCP action `llm_invocations` (requires `cortex:admin` scope), REST `GET /api/sessions/llm-invocations` (requires the admin API token).
  - `cortex sessions assess` (the Gemini CLI subprocess assessment path) now routes through `LlmRunner` instead of invoking the Gemini subprocess directly.

  ### Changed
  - **Behavior change:** `cortex sessions assess` now enforces `[llm].max_concurrent=1` / `[llm].max_per_action_concurrent=1` by default. Two overlapping interactive `assess` invocations will now hard-fail the second with a concurrency-limited error, where previously they ran concurrently with no guard. Raise `[llm].max_concurrent` / `[llm].max_per_action_concurrent` if your workflow depends on concurrent assessments.
  - `GeminiAssessConfig::from_env` no longer reads `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` for the `cortex sessions assess` path; the Gemini subprocess timeout is now driven solely by `[llm].timeout_secs` (`CORTEX_LLM_TIMEOUT_SECS`), eliminating a silent dual-timeout-source bug. Setting `CORTEX_LLM_COMPLETION_TIMEOUT_SECS` now logs a deprecation warning and has no effect on this path.

  ### Security
  - The `llm_invocations` audit read surface (CLI/MCP/REST) is scoped `cortex:admin`, not `cortex:read` — its `status`/`error`/`metadata_json` fields expose circuit-breaker and kill-switch operational state that is not appropriate for the broad `cortex:read` trust tier.
  ```

  (Confirm the exact version number `cargo xtask bump-version minor` produced — it must match `Cargo.toml`'s new `[package] version` exactly; adjust the `## [X.Y.Z]` heading above to match.)

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo xtask check-version-sync && cargo xtask check-release-versions`
  Expected: both PASS (exit code 0).

  Also run the full test suite one final time to confirm nothing in Tasks 1-9 regressed: `cargo test --lib 2>&1 | tail -40` and `cargo clippy --all-targets -- -D warnings 2>&1 | tail -60` — both expected to pass cleanly before this phase is considered done.

- [ ] **Step 5: Commit**
  ```bash
  git add Cargo.toml Cargo.lock server.json mcpb/manifest.json docker-compose.prod.yml CHANGELOG.md
  git commit -m "chore: bump version to 3.2.0 for LLM invocation guard feature"
  ```


---

## Self-Review

### Spec coverage

Mapping GH #94's "Mandatory LLM Observability and Safety Controls" requirements to the task that implements each one:

- **Audit record fields** (who/what/when/how-much/outcome for every LLM call attempt) — Task 1 (`llm_invocations` schema: `id`, `started_at`, `finished_at`, `duration_ms`, `caller_surface`, `action`, `provider`, `model`, `program`, `incident_id`, `ai_tool`, `ai_project`, `ai_session_id`, `evidence_counts_json`, `prompt_bytes`, `output_bytes`, `status`, `error`, `metadata_json`) + Task 4 (typed `LlmInvocationRow`/insert/finish query layer).
- **Global concurrency limit** — Task 3 (`LlmRunner::run` step 6, `global_permits: Arc<Semaphore>` sized from `LlmConfig.max_concurrent`, denies with `LlmRunnerError::ConcurrencyLimited` and audits status `denied`/`global_concurrency_limited`).
- **Per-action concurrency limit** — Task 3 (same step, per-action `Semaphore` sized from `max_per_action_concurrent`, denies with `LlmRunnerError::ActionConcurrencyLimited`).
- **Rate limits (per-minute and per-hour)** — Task 3 (`LlmRunner::run` step 5, sliding-window `recent_starts` check against `max_invocations_per_minute` / `max_invocations_per_hour`, denies with `LlmRunnerError::RateLimited` and audits status `rate_limited`).
- **Circuit breaker** — Task 3 (`LlmRunner::run` step 4, `consecutive_failures` / `circuit_open_until` in `ActionState`, opens after `failure_threshold` consecutive failures/timeouts for `cooldown_secs`, denies with `LlmRunnerError::CircuitOpen` and audits status `circuit_open`).
- **Per-invocation timeout** — Task 3 (`LlmRunner::run` step 7, `tokio::time::timeout(Duration::from_secs(config.timeout_secs), run_fn(prompt))`, audits status `timeout` and counts toward the circuit breaker).
- **Prompt/output size limits** — Task 3 (`LlmRunner::run` step 3 rejects prompts over `max_prompt_bytes` before spawning anything, audits status `error`/`prompt_too_large`; successful output is truncated to `max_output_bytes` before persisting/returning).
- **Dry-run mode** — Task 3 (`LlmRunner::dry_run`, reports prompt size and `would_exceed_prompt_limit` without invoking the LLM or consulting concurrency/rate-limit/circuit state, still writes an audited `dry_run` row).
- **Global kill switch** — Task 2 (`LlmConfig.enabled`, `CORTEX_LLM_ENABLED` env override) + Task 3 (`LlmRunner::run` step 1 denies with `LlmRunnerError::Disabled`, audits status `disabled`).
- **Per-action enablement** — Task 2 (`LlmConfig.actions: HashMap<String, LlmActionConfig>`, `[llm.actions.<name>]`) + Task 3 (`LlmRunner::run` step 2 / `action_enabled()`, denies with `LlmRunnerError::ActionDisabled`; also enforces `background_enrichment_enabled` for the `background_enrich` action specifically).
- **Migrating `cortex sessions assess` onto the guard** — Task 6 (`CortexService::run_gemini_assess_with_delta` replaces its direct `run_gemini_assessment(...)` call with `self.llm().run(spec, run_fn)`, preserving the existing public signature and delta-streaming behavior via an mpsc channel bridge).
- **New read surfaces** — Task 7 (CLI `cortex sessions llm-invocations`, MCP action `llm_invocations` scoped `cortex:admin`, REST `GET /api/sessions/llm-invocations` gated by `require_api_admin_token`, all backed by `CortexService::llm_invocations_checked` → `crate::db::llm_invocations::list_llm_invocations`).

No gap was found requiring a new task — all eleven requirement categories map onto Tasks 1–7 as drafted.

### Placeholder scan

Every task (1 through 9) contains complete, concrete Rust/TOML/Markdown/bash — no `TODO`, `FIXME`, `unimplemented!()`, `todo!()`, "add appropriate error handling", or unfilled code-block bodies were found anywhere in the plan. Where a task depends on a repo detail not yet re-confirmed (e.g. the exact `init_pool` signature, the sidecar test file path for `src/config.rs`, the exact `test_service()` return shape, the real Gemini-stub helper name in `src/assessment_tests.rs`), the plan gives an explicit `grep -n ...` command to run first and states the fallback/adjustment to make — these are legitimate "verify-then-adapt" instructions for the implementing agent, not unfinished placeholders, and each one names the exact command to resolve the ambiguity rather than leaving it open-ended.

### Type consistency

`LlmRunner` / `LlmInvocationSpec` / `LlmCallerSurface` / `LlmEvidenceCounts` / `LlmInvocationOutcome` / `LlmDryRunOutcome` / `LlmRunnerError` are defined once, in Task 3 (`src/app/llm_runner.rs`), matching field-for-field and variant-for-variant the "Locked interfaces for other phases" block at the top of this document. Downstream tasks consume them identically:

- Task 4 (`src/db/llm_invocations.rs`) deliberately does NOT depend on `LlmEvidenceCounts` — it takes a plain `Option<String>` (`evidence_counts_json`) matching the column type, with `LlmRunner` (Task 3) responsible for the `serde_json::to_string` conversion. This is a documented, intentional decoupling, not a mismatch.
- Task 5 (`CortexService`) stores `Arc<crate::app::llm_runner::LlmRunner>` and exposes `pub fn llm(&self) -> &crate::app::llm_runner::LlmRunner`, constructed via `LlmRunner::new(pool: Arc<DbPool>, config: LlmConfig)` — identical signature to Task 3's definition and Task 2's `LlmConfig`.
- Task 6 (`assessment.rs`) constructs `LlmInvocationSpec` with all fields present and correctly typed (`caller_surface: LlmCallerSurface::Cli`, `evidence_counts: LlmEvidenceCounts { .. }`, etc.) and calls `self.llm().run(spec, run_fn)` where `run_fn: impl FnOnce(String) -> Fut + Send + 'static` matches Task 3's `run_fn` bound exactly (verified via the `move |prompt| async move { ... }` closure signature).
- Task 7 does not construct or consume `LlmInvocationSpec`/`LlmRunner` at all (by design — it is a read-only surface over Task 4's `LlmInvocationRow`/`list_llm_invocations`, explicitly called out in its own Interfaces section as NOT using `CortexService::llm()`).

No signature drift was found between the locked interface block and any task's usage.

### Post-review update

After this Self-Review was originally written, four independent engineering
reviews (architecture, simplicity, security, performance) were run against
this plan and found 5 MUST-FIX issues. All 5 have been applied directly into
the task bodies above (real code, not placeholders — the affected Step 1/
Step 3 code blocks were edited in place) and are documented in the "Eng
Review Fixes Applied" section near the top of this plan: (1) a timeout
duplication regression between `[llm].timeout_secs` and the legacy
`CORTEX_LLM_COMPLETION_TIMEOUT_SECS` in Task 6's Gemini migration, (2) a
missing `crate::db::write_lock()` guard on every audit write in Tasks 3/4
(CRITICAL — this repo's standing single-writer invariant), (3) a weaker,
mistimed secret-redaction heuristic in Task 3's `sanitize_error`, (4) the
`llm_invocations` read surface being scoped `cortex:read` instead of
`cortex:admin` despite exposing operational kill-switch/circuit-breaker
state, and (5) `extra_metadata` having no redaction pass or size cap. None
of the fixes reduced the scope of GH #94's mandatory concurrency/rate-limit/
circuit-breaker/kill-switch/dry-run/read-surface requirements — they
correct bugs and close gaps within that same required scope. The type-
consistency and spec-coverage analysis above remains accurate after these
fixes; the only interface addition is `LlmRunner::timeout_secs(&self) -> u64`
(Fix 1) and `const LLM_METADATA_MAX_BYTES: usize` (Fix 5), both purely
additive to the locked interface block and non-breaking for PR 2-4.
