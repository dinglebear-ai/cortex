---
date: 2026-05-28 03:03:03 EST
repo: git@github.com:jmagar/syslog-mcp.git
branch: feat/service-layer-timing
head: a434a78b643ceed57fc5b1aa810575f5084c129c
plan: docs/superpowers/plans/2026-05-18-service-layer-timing.md
agent: Claude (claude-sonnet-4-6)
session id: 1fb487c6-968f-43b0-896c-050d349dbe3b
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/1fb487c6-968f-43b0-896c-050d349dbe3b.jsonl
working directory: /home/jmagar/workspace/syslog-mcp/.worktrees/service-layer-timing
worktree: /home/jmagar/workspace/syslog-mcp/.worktrees/service-layer-timing a434a78 [feat/service-layer-timing]
pr: "#57 feat(service): add per-action DB timing instrumentation to run_db — https://github.com/jmagar/syslog-mcp/pull/57"
---

## User Request

Continue the `work-it` workflow for plan `docs/superpowers/plans/2026-05-18-service-layer-timing.md` from where a prior compacted session left off. The prior session had completed the core implementation and PR creation; this session picks up at the review-fix phase identified by the architecture review.

## Session Overview

Resolved all review findings from the service-layer timing PR (#57): fixed a HIGH-priority silent-failure bug in `correlate_state.resolve_host`, restructured `run_db` to emit timing events on all early-return paths (they were previously silent), added warn-on-slow escalation at 500 ms, renamed an inconsistent op label, extended the test suite to cover the slow+error and semaphore-closed paths, addressed a P1 reviewer comment about version bumping, and pushed 5 follow-up commits to the PR.

## Sequence of Events

1. Read the current `run_db` implementation at `src/app/service.rs:448–479` to confirm the early-return bug described in the prior session's architecture review.
2. Added `pub(super) const SLOW_DB_MS: u128 = 500` at module level, restructured `run_db` to use explicit `match` blocks so all three early-return paths (timeout, semaphore-closed, JoinError) emit `tracing::warn` before returning, and added the warn-on-slow threshold branch.
3. Renamed op label `"incident"` → `"incident.search"` at `service.rs:387` for dotted-convention consistency.
4. Added `run_db_emits_warn_on_slow_op` and `run_db_emits_warn_on_semaphore_closed` tests to `service_tests.rs`; ran `cargo test` — 1178 passed.
5. Committed and pushed the early-return fix + two new tests; ran two `rustfmt` format passes required by pre-commit hook.
6. Launched code-simplifier passes 2 (tests) and 3 (docs/config) in parallel.
   - Simplifier 2 tightened the warn tests: added `permit_ms`/`exec_ms` field assertions to slow-op test; added `matches!(err, ServiceError::Busy(_))` to semaphore-closed test.
   - Simplifier 3 found no actionable issues.
7. Committed simplifier improvements; ran `cargo test` again — 1178 passed.
8. Launched pr-review-toolkit sweep in parallel: silent-failure-hunter, code-reviewer, pr-test-analyzer.
9. Applied three actionable findings from the sweep:
   - **CRITICAL (silent-failure-hunter)**: `correlate_state.resolve_host` at `service.rs:682` used `.unwrap_or(false)` to discard `COUNT(*)` errors — changed to `?`.
   - **MEDIUM (silent-failure-hunter)**: `JoinError` branch logged "db task panic" unconditionally — added `e.is_cancelled()` branch for accurate "db task cancelled" message on graceful-shutdown aborts.
   - **GAP 1 (pr-test-analyzer)**: Slow+error branch had no test — added `run_db_emits_warn_on_slow_op_with_error`.
10. Fixed type-inference compile error on the new test (added `let _: ServiceResult<()> =` annotation).
11. Ran `cargo test` — 1179 passed. Committed and pushed the three fixes.
12. Committed simplifier test improvements (format-only); pushed.
13. Fetched PR comments — one P1 comment from automated reviewer requesting version bump for the feature branch.
14. Bumped `Cargo.toml` from `0.35.0` → `0.35.1`, added `[0.35.1]` section to `CHANGELOG.md`, committed and pushed.
15. Replied to the inline PR comment confirming the bump was applied.
16. Saved session to `docs/sessions/`.

## Key Findings

- **Silent early returns in `run_db`** (`service.rs:454–461` before fix): two `?` operators on the timeout and semaphore-closed paths returned before `tracing::debug!` fired — the most critical failure cases (pool saturation) produced no trace event at all.
- **Silent failure in `correlate_state.resolve_host`** (`service.rs:682` before fix): `.unwrap_or(false)` on a `rusqlite` query silently converted any DB error (locking, schema drift, I/O) into `false`, then fell through to the hostname fallback which also failed, resulting in a misleading `ServiceError::NotFound` with nothing in the logs.
- **JoinError ambiguity** (`service.rs:487` before fix): `spawn_blocking` cancellation during graceful shutdown emitted "db task panic" — would generate false-positive panic alerts in any alerting built on that log message.
- **Slow+error branch untested**: the `Err` arm of the `exec_ms > SLOW_DB_MS` block was fully untested; a refactor stripping the `error = %e` field would have passed all tests.
- **`SLOW_DB_MS` visibility**: made `pub(super)` so tests can reference it directly without hardcoding `500`, protecting against future threshold changes.

## Technical Decisions

- **`SLOW_DB_MS = 500`**: 500 ms is a reasonable threshold for SQLite operations on a homelab; fast enough to catch real slowdowns without constant noise. Made it a named const at module scope (not inside the function) so tests can reference it.
- **`warn` vs `error` for timeout/semaphore-closed**: used `warn` not `error` because these are operational conditions (transient saturation) not programmer bugs; the caller still receives a `ServiceError::Busy` and can surface it as appropriate.
- **`?` instead of `.unwrap_or(false)`**: the `map_err(|e| match e { ... other => other })` at `service.rs:701–708` already correctly passes through `ServiceError::Internal` variants that aren't `not_found` or `ambiguous_host`, so the propagated DB error surfaces as an internal error rather than a not-found — correct behavior.
- **`e.is_cancelled()` branch**: added without changing the `ServiceError::Internal` return type — only the log message differs. Preserves the caller contract while fixing operator confusion during shutdown.
- **Version bump to patch 0.35.1**: the changes are internal observability improvements (no new user-visible feature), so a patch bump is semantically correct even though the branch name says "feat/".

## Files Modified

| File | Purpose |
|------|---------|
| `src/app/service.rs` | Fixed `run_db`: early-return warn paths, SLOW_DB_MS const, warn-on-slow, `incident.search` label rename, `resolve_host` `.unwrap_or(false)` → `?`, JoinError is_cancelled branch |
| `src/app/service_tests.rs` | Added `run_db_emits_warn_on_slow_op`, `run_db_emits_warn_on_semaphore_closed`, `run_db_emits_warn_on_slow_op_with_error`; tightened existing warn-test assertions |
| `Cargo.toml` | Version bump 0.35.0 → 0.35.1 |
| `CHANGELOG.md` | Added `[0.35.1]` section describing all timing observability changes |

## Commands Executed

```bash
# Verification
rtk cargo test       # 1179 passed, 1 ignored
rtk cargo clippy     # No issues found

# Format (required by pre-commit hook)
cargo fmt

# Git
rtk git push         # feat/service-layer-timing → origin

# PR comment resolution
gh api -X POST repos/jmagar/syslog-mcp/pulls/57/reviews \
  -f body="Version bumped to 0.35.1 in Cargo.toml and CHANGELOG.md..." \
  -f event="COMMENT"
```

## Errors Encountered

- **Type inference compile error** on `run_db_emits_warn_on_slow_op_with_error`: `let _ = service.run_db(...)` did not provide enough info for the compiler to infer `T`. Fixed by annotating `let _: ServiceResult<()> = ...`.
- **Pre-commit hook format failures** (2 occurrences): `rustfmt` reformatted multiline closure calls and `anyhow::anyhow!()` calls. Fixed by running `cargo fmt` and re-staging before each commit.

## Behavior Changes (Before/After)

| Path | Before | After |
|------|--------|-------|
| `run_db` acquire timeout | Silent — returns `Busy` with no log event | Emits `tracing::warn!(op, permit_ms, "db acquire timeout")` before returning |
| `run_db` semaphore closed | Silent — returns `Busy` with no log event | Emits `tracing::warn!(op, permit_ms, "db semaphore closed")` before returning |
| `run_db` task panic | Emits "db task panic" for both panics AND graceful-shutdown cancellations | Panics emit "db task panic"; cancellations emit "db task cancelled" |
| `run_db` slow ops (>500ms) | Always logged at `debug` level | Logged at `warn` level — visible at production `RUST_LOG=info` |
| `correlate_state.resolve_host` DB error | Silently treated as "host not found" → `ServiceError::NotFound` with no log | Propagates as `ServiceError::Internal`, logged at `debug` (or `warn` if slow) |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `rtk cargo test` | 1179 passed | 1179 passed, 1 ignored | ✅ |
| `rtk cargo clippy` | No issues | No issues | ✅ |
| `cargo fmt --check` | No diff | No diff (after format pass) | ✅ |

## Risks and Rollback

- **`resolve_host` behavior change**: changing `.unwrap_or(false)` to `?` means a transient DB lock during host resolution now returns `ServiceError::Internal` instead of silently falling through to `NotFound`. This is correct, but callers that previously relied on the silent fallback behavior will now see a 500-class error instead of a 404. Rollback: revert `service.rs:682` to `.unwrap_or(false)`.
- **Slow-op warn noise**: the 500 ms threshold may generate frequent warns on queries that are acceptably slow in homelab context. If noise is excessive, raise `SLOW_DB_MS` (it's a named const, one-line change).

## Decisions Not Taken

- **Restore `elapsed_ms` to `rmcp_server.rs`**: the architecture review noted that removing INFO-level end-to-end timing from the MCP layer lost coverage for non-DB tools like `run_gemini_assess`. Decision: do not restore; add warn-on-slow in `run_db` instead. The Gemini subprocess case is a separate concern for a future PR.
- **Timeout-path test**: triggering the `tokio::time::timeout` expiry path requires a service with near-zero `acquire_timeout` and all permits held — the field is private with no test-visible setter. The effort is disproportionate; the semaphore-closed test covers the structurally similar `Ok(Err(_))` arm.
- **Task-panic test**: would require `spawn_blocking` to panic, achievable but adds complexity for low practical value. Deferred.

## Next Steps

**Follow-on work (not yet started):**
- Investigate and fix `ai_tool` data consistency bug: `syslog ai projects` shows both `"claude"` and `"claude-code"` because the `ai_tool` column sometimes contains the raw `app_name` value instead of the normalized value from `ai_tool_from_app()` (`src/syslog/enrichment.rs:251–258`). The `GROUP_CONCAT(DISTINCT ai_tool)` in `list_ai_projects()` (`src/db/queries.rs:874`) then surfaces both.
- Performance: `syslog ai blocks` takes ~3 minutes on 4.9M rows due to a full table scan with heavy GROUP BY and no covering index on `(ai_tool, ai_project, ai_session_id)`.
