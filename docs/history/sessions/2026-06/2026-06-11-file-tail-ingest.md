---
date: 2026-06-11 21:37:45 EDT
repo: git@github.com:jmagar/cortex.git
branch: codex/file-tail-ingest
head: 4b17a3e
pr: https://github.com/jmagar/cortex/pull/73
working directory: /home/jmagar/workspace/cortex/.worktrees/file-tail-ingest
worktree: /home/jmagar/workspace/cortex/.worktrees/file-tail-ingest
beads: syslog-mcp-6y96m
---

# Managed file-tail ingest

## User Request
Create and work the plan `2026-06-11-file-tail-ingest.md`: keep the special log-file ingestion functionality that previously came from syslog forwarders, but expose it as a managed Cortex action available through CLI, REST API, and MCP.

## Session Overview
Implemented managed file-tail ingestion on branch `codex/file-tail-ingest` and opened PR #73. Cortex can now persist file-tail source definitions, supervise enabled sources, ingest appended lines through the same log path as other sources, and manage those sources through `cortex file-tail`, `POST /api/file-tails`, and the admin MCP action `file_tails`.

The final review pass hardened the feature around admin authorization, path policy, checkpoint correctness, partial-line handling, rotation, MCP schema typing, smoke/live-test behavior, and docs parity.

## Sequence of Events
1. Created isolated worktree `/home/jmagar/workspace/cortex/.worktrees/file-tail-ingest` on branch `codex/file-tail-ingest`.
2. Claimed bead `syslog-mcp-6y96m` for managed file-tail ingestion.
3. Implemented the initial registry, supervisor, runtime wiring, CLI/API/MCP action, docs, config, and version bump to 1.20.0.
4. Opened PR #73: `feat: add managed file-tail ingest`.
5. Ran multiple review waves focused on data integrity, security, performance, API contracts, test simplification, and PR comments.
6. Landed review hardening commit `4b17a3e` after local full-gate verification and pre-push verification.
7. Dispatched two follow-up review agents over PR #73, split between runtime/data-integrity and API/CLI/MCP/docs concerns.
8. Folded their fixes into the branch: durable DB-write acknowledgement before file-tail checkpoints, safe opened-file identity checks, read-only list/status behavior, partial-EOF ingestion before rotation, admin/CORS/schema hardening, smoke-test admin coverage, and narrower production path defaults.

## Key Changes
| area | change |
|---|---|
| Registry | `file-tails.json` persisted via `FileTailRegistry`, with add/list/status/remove/enable/disable operations and missing-id errors. |
| Supervisor | `FileTailSupervisor` tails enabled sources, checkpoints progress, reloads registry state between retries, handles copytruncate and rename-create rotation, and buffers partial EOF records until newline. |
| Security | REST management requires normal API bearer auth plus `X-Cortex-Admin-Token`; file paths must be absolute, existing, regular files, non-symlinks, and inside configured allow roots. |
| Metadata | File-tail rows use `source_kind=file-tail`, `source=file-tail:<id>`, and metadata limited to `file_tail_id`, `tag`, and `path_basename`. |
| Interfaces | Added `cortex file-tail` CLI, `/api/file-tails`, and admin MCP action `file_tails`. |
| Contracts | Updated MCP schemas, action counts, source-kind docs, filter aliases, API docs, CLI docs, config docs, README, CLAUDE.md, and smoke/live test scripts. |

## Review Findings Addressed
- Checkpoint updates are registry-authoritative, so retries do not replay from stale in-memory source copies.
- File-tail checkpoints now advance only after the batch writer successfully commits the row to SQLite; retryable write failures keep the durable ack pending.
- The batch writer flushes durable file-tail entries immediately so checkpoint durability does not throttle ingestion to one line per flush interval.
- Partial lines at EOF are held until newline instead of being prematurely ingested.
- Unterminated partial lines are ingested before rotation/truncation and leave status context for operators.
- Rotation and truncation reopen paths through the same path-policy checks used on initial open.
- Opened file identity is validated after `O_NOFOLLOW` open so symlink swaps or path races are rejected.
- Production defaults now allow only `/file-tail-root` unless `CORTEX_FILE_TAIL_ALLOWED_ROOTS` is set; tests retain tempdir roots.
- Missing files during reopen now surface an error instead of silently marking the source healthy.
- `list` and `status` are read-only and no longer reconcile/spawn supervisor tasks.
- Disable/remove stop active tailing through supervisor reconciliation tests.
- Configured file-tail hostnames are normalized/validated and source identity components are sanitized.
- MCP schema now constrains `get.id` to integer and `file_tails.id` to string through action-specific JSON Schema conditionals.
- MCP schema now also constrains `file_tails.op` to the supported operation enum and requires per-operation fields.
- REST tests cover missing and wrong admin token cases.
- Blank admin tokens are rejected and `X-Cortex-Admin-Token` is accepted by CORS preflight.
- Smoke/live MCP scripts skip admin-only `file_tails` only when the token cannot have admin scope.
- Optional admin-token smoke tests add, append, query, and remove a live file-tail source under `/file-tail-root`.
- The stale `[Unreleased]` changelog compare target now starts from `v1.20.0`.

## Verification Evidence
| command | result |
|---|---|
| `cargo test file_tail::supervisor_tests::supervisor_ingests_appended_line_and_updates_checkpoint --lib` | pass |
| `cargo test otlp::tests::auth --lib` | pass |
| `cargo test file_tail --all-targets` | pass |
| `bash -n scripts/smoke-test.sh tests/test_live.sh tests/mcporter/test-tools.sh` | pass |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo test` | pass: 1178 lib + 332 main + integration/doc tests clean, with 2 ignored network/perf tests |
| `bash scripts/check-version-sync.sh` | pass |
| `bash scripts/check-rust-module-size.sh --limit 500 src/cli.rs src/cli` | pass |
| `cargo deny check` | pass, with existing wildcard git-source warning for `lab-auth` |
| `git push` pre-push hook | pending for the review-follow-up commit |

## PR State
- PR: https://github.com/jmagar/cortex/pull/73
- Head after implementation/review fixes: current review-follow-up commit after `4b17a3e`
- Local verification: green.
- GitHub checks need to restart after the review-follow-up push.

## Remaining Notes
- CodeRabbit/PR Review Toolkit comments from the previous head were used as the acceptance bar for the follow-up agent pass.
- The `FileTailStatus.running` boolean was left as-is to avoid a broader API shape churn; `last_error` now carries retry detail for operators.
- `CORTEX_FILE_TAIL_ALLOWED_ROOTS` can broaden or further constrain the default `/file-tail-root` mount set.

## Beads Activity
- `syslog-mcp-6y96m` was claimed and implemented.
- `syslog-mcp-6y96m` was reopened for the review follow-up.
- Follow-up bead `syslog-mcp-7j9hn` was opened and closed by the API/CLI/docs agent during review remediation.

## Next Steps
- Wait for PR #73 CI and CodeRabbit to finish on the final branch head.
- Merge PR #73 once checks are green.
- Deploy with mounted log roots for any host that should manage file-tail sources through Cortex rather than rsyslog imfile.

## 2026-06-12 Second Review Pass

Dispatched the PR review toolkit again after PR #73 head `e54d033` and addressed the fresh findings from CodeRabbit plus the reviewer/test-analyzer agents.

### Additional Fixes

- Fixed the file-tail model compile break introduced while making `hostname` required.
- Required `hostname` across CLI, REST, MCP schema, models, docs, and tests so file-tail rows are never silently attributed to the Cortex container/local host.
- Rejected duplicate `file_tails op=add` requests before `upsert`, preserving existing checkpoints instead of resetting them.
- Kept query-only/stdio runtimes registry-readable but mutation-disabled, preventing local CLI or stdio MCP sessions from spawning competing tailers outside the long-running server.
- Changed the HTTP client file-tail admin POST path to avoid 503 retry replay and added a wiremock regression test.
- Added explicit committed-mutation error messages for reconcile/refresh failures and a regression test proving registry state is preserved.
- Added `last_read_at`, `last_checkpoint_at`, and `blocked_on_writer_since` to file-tail status and surfaced file-tail blocked count in the general `status` action.
- Hardened resume/rotation behavior: stale checkpoint identity now starts replacement files at offset 0; same-inode copytruncate/regrow is detected by prefix fingerprint; rename-create rotation waits through a short EOF grace window before switching away from the old file descriptor.
- Canonicalized configured allowed roots and added direct path-policy tests for env roots and symlinked roots.
- Fixed the Docker live smoke harness by chmodding the host smoke dir/file and asserting container readability before the file-tail smoke.
- Tightened admin-scope predicates in `scripts/smoke-test.sh`, `tests/test_live.sh`, and `tests/mcporter/test-tools.sh`.
- Bumped version to `1.20.1` with changelog notes for the review-hardening patch.

### Second-Pass Verification

| command | result |
|---|---|
| `bash -n scripts/smoke-test.sh tests/test_live.sh tests/mcporter/test-tools.sh` | pass |
| `cargo test file_tail --lib --quiet` | pass: 45 tests |
| `cargo test file_tails --lib --quiet` | pass: 13 tests |
| `cargo test http_client::tests::file_tails_post_does_not_retry_503 --quiet` | pass |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo test --locked` | pass: 1189 lib + 334 main + integration/doc tests clean, with 1 ignored network-dependent test |
| `CORTEX_TOKEN=codex-live-smoke-token bash tests/test_live.sh --mode docker --token codex-live-smoke-token` | pass: 122 passed, 0 failed, 4 skipped |
