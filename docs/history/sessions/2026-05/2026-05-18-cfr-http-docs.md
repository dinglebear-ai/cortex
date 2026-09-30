---
date: 2026-05-18 00:44:19 EDT
repo: https://github.com/jmagar/syslog-mcp
branch: work/cfr-http-docs
head: 7062643
plan: /home/jmagar/workspace/syslog-mcp/06-all-issues.md
working directory: /home/jmagar/workspace/syslog-mcp/.worktrees/cfr-http-docs
worktree: /home/jmagar/workspace/syslog-mcp/.worktrees/cfr-http-docs 7062643 [work/cfr-http-docs]
---

# CFR HTTP, OTLP, and README Docs

## User Request

Use the `work-it` workflow in an isolated worktree for Agent 5's CFR-017,
CFR-018, and CFR-019 assignment from `/home/jmagar/workspace/syslog-mcp/06-all-issues.md`.

## Session Overview

- Created worktree `.worktrees/cfr-http-docs` on branch `work/cfr-http-docs`.
- Replaced MCP and non-MCP API CORS `allow_headers(Any)` usage with explicit header allowlists.
- Made deferred OTLP `/v1/traces` require the same bearer-token helper as `/v1/logs` and `/v1/metrics`.
- Updated README action inventory for unaddressed error and notification actions.
- Bumped release metadata to `0.25.4` and added a changelog entry.

## Sequence of Events

- Inspected main checkout state and read the consolidated issue register.
- Created the requested worktree and reviewed route/API/OTLP/doc surfaces.
- Implemented scoped CORS, OTLP, README, and test changes.
- Ran focused tests first, then full verification gates.
- Found the repo was already at `0.25.3` without a changelog entry, then bumped this branch to `0.25.4` per branch-push rules.
- Checked available review tooling; the generic Lab `Agent` entry was present, but no runnable agent types were configured.

## Key Findings

- MCP CORS previously allowed any request header in `src/mcp/routes.rs`; required browser/RMCP headers are `Accept`, `Authorization`, `Content-Type`, `mcp-protocol-version`, and `mcp-session-id`.
- Non-MCP API routes are GET-only and require only `Accept` and `Authorization` for browser callers.
- `/v1/metrics` already shared OTLP auth behavior with `/v1/logs`; `/v1/traces` was the only deferred endpoint returning 404 before checking auth.
- `docs/mcp/TOOLS.md` already listed the newer admin/notification actions; the stale surface was the README top-level inventory.

## Technical Decisions

- Kept changes out of `src/mcp/tools.rs` because another agent owns mutating action identity.
- Preserved the deferred 404 contract for authorized `/v1/traces` callers instead of adding ingest behavior.
- Added CORS preflight tests that assert allowed headers are concrete and do not reflect arbitrary request headers.
- Used `RUSTC_WRAPPER=` for Rust verification after `sccache` failed while zipping compiler outputs.

## Files Modified

- `src/mcp/routes.rs` - explicit MCP CORS request-header allowlist.
- `src/mcp/routes_tests.rs` - MCP CORS preflight regression coverage.
- `src/api.rs` - explicit non-MCP API CORS request-header allowlist.
- `src/api_tests.rs` - API CORS preflight regression coverage.
- `src/otlp.rs` - `/v1/traces` now checks OTLP bearer auth before deferred 404.
- `src/otlp_tests.rs` - `/v1/traces` unauthorized and authorized-deferred tests.
- `README.md` - updated action inventory and installer version example.
- `CHANGELOG.md` - added `0.25.4` release notes and comparison link.
- `Cargo.toml`, `Cargo.lock`, `server.json` - bumped to `0.25.4`.
- `docs/sessions/2026-05-18-cfr-http-docs.md` - this session note.

## Commands Executed

| Command | Result |
| --- | --- |
| `git worktree add -b work/cfr-http-docs .worktrees/cfr-http-docs HEAD` | Created worktree at `7062643`. |
| `cargo fmt` | Passed. |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test mcp::routes` | Passed, 32 route tests. |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test api::tests` | Passed, 12 API tests. |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test otlp::tests` | Passed, 22 OTLP tests. |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test` | Passed after version bump. |
| `cargo fmt --check` | Passed after version bump. |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo clippy -- -D warnings` | Passed after version bump. |
| `bash scripts/check-version-sync.sh` | Passed: all 2 checked files at `v0.25.4`. |

## Errors Encountered

- Initial parallel `cargo test` jobs failed in `sccache` with `Allocation error : not enough memory` while zipping compiler outputs. Rerunning with `RUSTC_WRAPPER=` and `CARGO_BUILD_JOBS=2` resolved it.
- `git status` inside the sandbox failed because Git LFS attempted to write under the shared `.git/lfs/tmp` path. Escalated `git status` succeeded.
- Lab exposed a generic `Agent` tool, but calls failed with `Agent type 'general-purpose' not found. Available agents:`; named review-agent waves were unavailable in this runtime.

## Behavior Changes

- Browser CORS preflight responses for MCP/API no longer allow arbitrary request headers.
- Unauthorized `/v1/traces` requests now return unauthorized when `SYSLOG_MCP_TOKEN` is configured.
- Authorized `/v1/traces` requests still return the deferred `traces_not_supported` 404 response.
- README top-level MCP action inventory now includes unaddressed error and notification actions.

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test mcp::routes` | MCP route tests pass | 32 passed | pass |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test api::tests` | API route tests pass | 12 passed | pass |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test otlp::tests` | OTLP tests pass | 22 passed | pass |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo test` | Full suite passes | 583 lib tests, 48 main tests, integration tests, and doc tests passed | pass |
| `cargo fmt --check` | Formatting clean | no output, exit 0 | pass |
| `RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 cargo clippy -- -D warnings` | Lints pass | finished successfully | pass |
| `bash scripts/check-version-sync.sh` | Version metadata synchronized | OK at `v0.25.4` | pass |

## Risks and Rollback

- CORS allowlists can block browser clients that depend on an undocumented custom request header. Roll back by restoring `allow_headers(Any)` or adding the specific missing header with a test.
- `/v1/traces` now performs auth before its deferred 404, which is a stricter behavior for token-protected deployments. Roll back by restoring the no-argument handler only if intentionally public deferred 404s are desired.

## Decisions Not Taken

- Did not modify `src/mcp/tools.rs` because this assignment did not require action dispatch changes and another agent owns mutating action identity.
- Did not generate README inventory from schema metadata because CFR-019 requested a low-risk inventory update and schema docs already contained the new actions.

## Open Questions

- The changelog had no `0.25.3` entry even though Cargo metadata started at `0.25.3`; this branch adds `0.25.4` but does not backfill `0.25.3`.

## Next Steps

- Create and push the PR for `work/cfr-http-docs`.
- Fetch PR comments after the PR exists and resolve any actionable review feedback.
