---
date: 2026-05-27 21:32:17 EST
repo: git@github.com:jmagar/syslog-mcp.git
branch: feat/api-route-parity-completion
head: cad1349
plan: docs/superpowers/plans/2026-05-27-api-route-parity-completion.md
agent: Claude (Opus 4.7, 1M context)
working directory: /home/jmagar/workspace/syslog-mcp/.worktrees/api-route-parity
worktree: /home/jmagar/workspace/syslog-mcp/.worktrees/api-route-parity
pr: 56 — feat: complete CLI/API/MCP route parity (3 new routes + 3 unblocked) — https://github.com/jmagar/syslog-mcp/pull/56
---

## User Request

Two-step request: first, audit CLI/API/MCP feature parity in `syslog-mcp`; then use `superpowers:writing-plans` to write a plan to implement all remaining routes relevant for the API; then execute that plan through the `/work-it` workflow in an isolated worktree.

## Session Overview

Performed a three-surface (CLI / HTTP API / MCP) parity audit, discovered five real query gaps, wrote a 7-task TDD plan, and executed it in a `.worktrees/` checkout. The implementation added three new HTTP routes (`/api/host-state`, `/api/context`, `/api/fleet-state`), registered `fleet_state` as a first-class MCP action, added three `HttpClient` wrappers, and removed three stale `--http` `bail!` guards. After PR #56 opened, three independent review waves ran (4 lavra-style reviewers, 3 simplifier passes, 3 pr-review-toolkit roles) producing 9 actionable findings — all fixed in the same worktree. The Codex bot review then flagged the documented `/api/context` follow-up, so that was also resolved by reshaping `SyslogService::context` to return `ServiceError::InvalidInput`/`NotFound`. Final state: 1175 tests (+10 from baseline), zero lint findings, PR open and pushed.

## Sequence of Events

1. **Parity audit** — Dispatched a general-purpose agent to enumerate `ACTION_SPECS` in `src/mcp/actions.rs`, HTTP routes in `src/api.rs`, and CLI verbs in `src/cli/parse*.rs`. Produced a 41-action matrix.
2. **Audit correction** — Discovered while writing the plan that the audit was wrong on `search_sessions` (it IS on HTTP as `/api/ai/search`) and missed `fleet_state` (service method exists, no MCP/HTTP exposure).
3. **Plan authoring** — Wrote `docs/superpowers/plans/2026-05-27-api-route-parity-completion.md` with 7 TDD tasks covering the 3 missing routes + 3 orphaned-route CLI wire-up.
4. **Worktree creation** — `git worktree add -b feat/api-route-parity-completion .worktrees/api-route-parity HEAD`.
5. **Implementation agent** — Executed the full 7-task plan via `superpowers:executing-plans`. One deviation: `/api/context` returned 500 instead of 400/404 because `service.context()` used bare `anyhow!`; agent pinned tests with `TODO(follow-up)` markers per the plan's explicit guidance.
6. **PR created** — `gh pr create` opened PR #56 immediately after plan completion (per work-it protocol — kicks off external reviewers).
7. **Wave 1 review** — 4 reviewers in parallel: architecture-strategist (clean), security-sentinel (1 medium: `since` timestamp validation), silent-failure-hunter (1 medium: missing `http_or_cancel` wrapper), code-reviewer (clean). Fix agent applied all 3 in 3 commits.
8. **Wave 2 — 3 simplifier passes** — Collapsed two `Query` wrappers (−34 LOC), trimmed redundant test name suffixes, fixed an inline-formatting drift in `smoke-test.sh`.
9. **Wave 3 review** — 3 pr-review-toolkit roles: pr-test-analyzer (4 coverage gaps), comment-analyzer (stale `41 actions` count), type-design-analyzer (drop `ContextQuery`, reclassify `fleet_state` cost). Fix agent applied all 8 in 4 commits.
10. **PR comment resolution** — Codex bot flagged the documented `/api/context` 500 issue. Decided to fix in-PR rather than defer; agent reshaped service errors to `ServiceError::InvalidInput`/`NotFound`, flipped pinned tests from 500 to 400/404. Posted resolution reply on PR.

## Key Findings

- `/api/ai/search` (already in `src/api.rs:247`) is the existing route for `search_sessions` — the audit missed this and counted it as a gap.
- `SyslogService::fleet_state` exists at `src/app/service.rs:527` (added in commit `afd77e4`) but was never registered as an MCP action or exposed on HTTP — most egregious gap of the audit.
- `src/cli/dispatch_ai.rs:389-423` had three `bail!("...currently runs locally only; omit --http.")` guards that were obsolete: the matching HTTP routes existed at `src/api.rs:238, 240, 239` but no `HttpClient` wrapper had been added.
- `SyslogService::host_state` at `src/app/service.rs:505` forwarded `req.since` raw to SQL `sampled_at >= ?2`, bypassing `parse_optional_timestamp` validation that every other timestamp-bearing service method uses.
- `SyslogService::fleet_state` at `src/app/service.rs:534-538` issues N+1 DB calls (one `heartbeat_metric_snapshot` per host) — `Cost::Moderate` classification was dishonest; reclassified to `Expensive`.
- The schema-coverage fence test (`src/mcp/tools_tests.rs:226-248`) auto-covers new actions through its `ACTION_SPECS` iteration — no payload-mapping change was needed for `fleet_state` once added to `ACTION_SPECS`.
- `docs/INVENTORY.md` + `docs/mcp/SCHEMA.md` + `docs/mcp/TOOLS.md` + `docs/mcp/TESTS.md` + `plugins/syslog/skills/cortex/SKILL.md` + 3 smoke-test scripts all need a row per registered action (enforced by `public_action_references_cover_schema_registry` fence) — caught only at `just test`, not at `cargo check`.

## Technical Decisions

- **No CLI verb additions for `host_state` / `context` / `fleet_state`.** Scoped the PR strictly to "API routes." Aurora UI consumes these directly; CLI verbs land later.
- **In-PR fix of the Codex P2 instead of follow-up.** The fix was 15 lines using the existing `host_state` sentinel-error pattern; deferring would leave the route returning 500s for known client errors.
- **Kept `ContextQuery` initially, then dropped it after type-design review.** Reasoning: `ContextRequest` didn't have `deny_unknown_fields`. Type reviewer pointed out the canonical type should carry the invariant; adding `deny_unknown_fields` to `ContextRequest` itself eliminated the wrapper.
- **Pre-existing `UnaddressedErrorsQuery` missing `deny_unknown_fields` fixed.** Per work-it protocol the worktree "owns" pre-existing issues; one-line attribute add was cheap defense-in-depth.
- **Plan format change deviation: extra commit `a8fbb8e`.** Task 3 plan step said `cargo check` suffices for MCP action registration, but the registry-coverage fence test only fires at `just test`. Implementation agent caught this and split the doc/script bookkeeping into its own commit before Task 6.

## Files Modified

| File | Purpose |
|------|---------|
| `src/api.rs` | Added 3 new GET handlers + `deny_unknown_fields` on `UnaddressedErrorsQuery`; later collapsed Query wrappers |
| `src/api_tests.rs` | +12 tests covering happy path, 400/404, bearer-required, `deny_unknown_fields` for new routes |
| `src/app/service.rs` | `host_state` validates `since` via `parse_optional_timestamp`; `context` returns `InvalidInput`/`NotFound` instead of `anyhow!` |
| `src/app/models.rs` | Added `deny_unknown_fields` to `ContextRequest` |
| `src/mcp/actions.rs` | Registered `fleet_state` ActionSpec (Read scope, Expensive cost) |
| `src/mcp/tools.rs` | Added `tool_fleet_state` dispatch arm + handler + help-text block |
| `src/mcp/tools_tests.rs` | Added `fleet_state_action_*` MCP dispatch tests |
| `src/cli/http_client.rs` | Added 3 wrapper methods (similar_incidents, ask_history, incident_context) |
| `src/cli/http_client_tests.rs` | NEW sidecar — wiremock round-trip tests for the 3 wrappers |
| `src/cli/dispatch_ai.rs` | Removed 3 stale `bail!` guards; wrapped HTTP arms in `http_or_cancel` |
| `src/app.rs` | Exported new request types needed by api.rs |
| `docs/mcp/SCHEMA.md` | Bumped action count 41 → 42; added `fleet_state` row |
| `docs/INVENTORY.md`, `docs/mcp/TOOLS.md`, `docs/mcp/TESTS.md`, `plugins/syslog/skills/cortex/SKILL.md` | Added `fleet_state` row (fence requirement) |
| `scripts/smoke-test.sh`, `tests/test_live.sh`, `tests/mcporter/test-tools.sh` | Added `fleet_state` action to inventory lists |
| `docs/superpowers/plans/2026-05-27-api-route-parity-completion.md` | The implementation plan itself |

## Commands Executed

- `git worktree add -b feat/api-route-parity-completion .worktrees/api-route-parity HEAD` — created isolated workspace
- `just test` (run after every fix wave) — 873 → 1175 tests passing throughout
- `just lint` (clippy `-D warnings`) — clean throughout
- `rtk gh pr create --title ... --body ...` — opened PR #56
- `rtk gh api repos/jmagar/syslog-mcp/pulls/56/comments` — fetched Codex inline review
- `rtk gh pr comment 56 --body ...` — replied to Codex with resolution evidence

## Errors Encountered

- **Implementation agent's first cd was lost between Bash invocations.** Caught by `git status` showing `## main` instead of the branch. Fixed by including `cd <worktree>` at the start of every multi-step Bash chain.
- **Original push command in coordinator session returned with empty output, looked successful but local was `ahead 3`.** Re-pushed from the worktree path explicitly; second push succeeded with `ok` confirmation.
- **`http_client_tests.rs` first attempt failed on `crate::app::...` imports.** `http_client.rs` belongs to the `syslog` binary which consumes `syslog_mcp` as a library; switched to `syslog_mcp::app::...` paths.
- **Commit 3 (wave-3 fixes) initially failed `lefthook format` pre-commit.** Reformatted and retried successfully — no `--no-verify` shortcut used.

## Behavior Changes (Before/After)

- `GET /api/host-state` — was 404 (route absent); now 200 with bounded heartbeat state, 400 if no host_id/hostname or invalid `since`, 404 if host unknown.
- `GET /api/context` — was 404 (route absent); now 200 with pivot-window log context, 400 if no pivot, 404 if log_id unknown. (Initially 500 for those cases — fixed in `cad1349`.)
- `GET /api/fleet-state` — was 404 (route absent); now 200 with fleet snapshot + pressure flags + summary counts.
- `syslog ai similar-incidents --http`, `syslog ai ask-history --http`, `syslog ai incident-context --http` — was `bail!("...currently runs locally only; omit --http.")`; now reaches the matching HTTP route and supports Ctrl-C cancellation.
- MCP action catalog grew from 41 → 42 actions; agents now see `fleet_state` in tool schemas.
- `HostStateRequest.since` — was forwarded raw to SQL `sampled_at >=`; now validated as RFC3339 at the service boundary.

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `just test` | all suites green, 1175 tests | 1175 passed, 1 ignored, 0 failed | pass |
| `just lint` | clippy `-D warnings` clean | clean | pass |
| `rtk cargo check --lib` | no errors or warnings | clean | pass |
| `rtk gh pr view 56` | open, ahead of main | open, 14+ commits ahead | pass |
| GitHub Actions CI | green | pending at session save (monitor running) | in-flight |

## Risks and Rollback

- **Surface widening risk** — three new public HTTP endpoints. Each is bearer-gated via the forced `AuthPolicy::Mounted` layer in `router()`. Rollback: revert the three handler commits (`41e18bf`, `ebd949d`, `4785171`) — pure additions, no schema/migration impact.
- **`fleet_state` is `Expensive` cost** but exposed broadly. Agents that planned around the previous `Moderate` rating (if any) may need recalibration. Reclassification commit (`3278c2c`) is one-line and trivially revertable.
- **`SyslogService::context` error reshape** changes wire-level status codes from 500 to 400/404 for two client-input cases. This is a behaviour change downstream consumers may observe — but a 500-on-bad-input was a bug, so no rollback expected.

## Decisions Not Taken

- **Did not refactor `HostStateRequest` into a `HostLookup` enum** to enforce the "exactly one of host_id / hostname" invariant at the type level. Type reviewer flagged this but explicitly deferred — refactor breaks URL-encoded query ergonomics and ripples through CLI + MCP construction sites; out of scope for a parity PR.
- **Did not file a bead for the original context follow-up.** Resolved it in the PR instead.
- **Did not add a live CLI smoke test for the 3 unblocked `--http` paths.** Architecture reviewer suggested it; unit-level `wiremock` coverage in `http_client_tests.rs` was deemed sufficient — live tests gated on infra availability.

## References

- Plan: `docs/superpowers/plans/2026-05-27-api-route-parity-completion.md`
- PR: https://github.com/jmagar/syslog-mcp/pull/56
- Original commit that added the `fleet_state` service method without exposure: `afd77e4`
- The 2026-05-22 surface-parity gap closure that landed 12 routes but left these orphaned: `289d571` (this PR removes the resulting `bail!` guards)
- Codex review comment ID: `chatgpt-codex-connector` P2 on `src/api.rs:622`

## Open Questions

- Should `host_state` / `context` / `fleet_state` get CLI verbs in a follow-up, or do operator surfaces consuming them via HTTP make that unnecessary?
- Is `correlate` (currently `Cost::Moderate`) similarly mis-classified given that `fleet_state` was? Worth a sweep of the cost table.

## Next Steps

**Started but not completed in this session:**
- CI checks on PR #56 are still pending (`Tests`, `Clippy`, `cargo-deny`, `Pre-publish CI gate`, CodeRabbit). Monitor running until they settle.

**Follow-on tasks not yet started:**
- `HostLookup` enum refactor for `HostStateRequest` (deferred per type-design reviewer).
- Audit the rest of the `Cost` classifications in `src/mcp/actions.rs` for honesty (started with `fleet_state`, didn't sweep others).
- Add CLI verbs (`syslog host-state`, `syslog context`, `syslog fleet-state`) that tunnel via `--http` to the new routes.
