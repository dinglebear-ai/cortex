> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-05-21 03:27:00 EST
repo: https://github.com/jmagar/syslog-mcp
branch: feat/surface-parity
head: 2025995
plan: docs/superpowers/plans/2026-05-21-surface-parity.md
agent: Claude (claude-sonnet-4-6)
working directory: /home/jmagar/workspace/syslog-mcp/.worktrees/surface-parity
worktree: /home/jmagar/workspace/syslog-mcp/.worktrees/surface-parity  2025995 [feat/surface-parity]
pr: "#40 — feat: surface parity — CLI + REST API for all MCP actions — https://github.com/jmagar/syslog-mcp/pull/40"
---

# Session: Surface Parity — CLI + REST API for All MCP Actions

## User Request

Investigate why the syslog-mcp tool surfaces were inconsistent — some features only reachable via MCP, others CLI-only — and build out a complete surface parity plan and implementation bringing CLI, REST API, and MCP to full feature parity.

## Session Overview

Audited all three tool surfaces (CLI, REST API, MCP) and produced a full feature matrix. Identified 10 MCP-only actions with no CLI or REST equivalents (critically: the entire error signature management surface). Wrote an implementation plan, dispatched a Rust implementation subagent to execute it in an isolated worktree, ran 4 parallel review agents (3 simplifier passes + security/correctness reviewer), applied fixes, and pushed to PR #40. Also fixed several unrelated ops issues discovered during the session: FreshRSS stack brought down, Chrome/browserless container removed, Apprise notifications enabled in the deployed server, error detection scanner enabled and configured.

## Sequence of Events

1. Checked recent errors via `syslog errors` — found FreshRSS PostgreSQL schema errors (recurring every 15min), Arcane auto-heal socket errors, Tracearr 401s, and Chrome/browserless logging stats at `err` level
2. Brought down FreshRSS stack on edgehost (`docker compose down`) — user no longer uses it
3. Fixed Chrome/browserless log level by adding `DEBUG=browserless:*,-browserless:server` env var, then user requested stopping the container entirely — brought it down
4. Investigated Arcane auto-heal errors — identified race between `docker-client-refresh` and `auto-heal` jobs (v1.19.4 bug, latest image, no upstream fix)
5. Investigated error signature management — discovered `unaddressed_errors`/`ack_error`/`unack_error` MCP actions exist but error scanner had never run (notifications disabled in deployed `.env`)
6. Enabled notifications and error detection in deployed `.env` with correct env vars; restarted container; confirmed scanner running at 60s interval
7. Performed full feature matrix audit across all three surfaces — identified 10 MCP-only gaps
8. Filed two beads issues: `syslog-mcp-clq5` (REST API gaps) and `syslog-mcp-neke` (CLI gaps)
9. Wrote implementation plan at `docs/superpowers/plans/2026-05-21-surface-parity.md`
10. Created worktree `feat/surface-parity`, dispatched Rust pro subagent to implement all 8 plan tasks
11. Subagent produced 6 commits: 907 tests passing, clippy clean, release build green
12. Dispatched 4 parallel review agents: 3 code simplifier passes + security/correctness review
13. Applied simplifier-1 improvements (parser helper functions, 108 ins / 148 del net reduction)
14. Fixed docs gap: `mcp-actions-current.md` missing 5 action specs — added §7 with full schemas
15. Pushed all changes to PR #40

## Key Findings

- **MCP-only actions with no CLI/REST**: `source_ips`, `timeline`, `patterns`, `ingest_rate`, `get`, `unaddressed_errors`, `ack_error`, `unack_error`, `notifications_recent`, `notifications_test` — all required SQLite direct queries to use outside an AI session
- **Error scanner never ran**: `error_scan_cursor` showed `last_log_id=0`; root cause was `SYSLOG_MCP_ERROR_DETECTION_ENABLED` missing from `/home/jmagar/.syslog-mcp/.env`. Notifications and error detection are separate subsystems requiring separate env vars
- **`docker restart` doesn't re-read `env_file`**: Required full `docker rm -f` + `docker compose up -d` to pick up new env vars — `restart` silently keeps old env
- **Default `scan_interval_secs=3600`**: Error scanner defaults to 1-hour cycles; overrode to 60s via `SYSLOG_MCP_ERROR_DETECTION_SCAN_INTERVAL_SECS=60` for operability
- **Ingest queue at 100% saturation**: 3 containers driving 63% of volume — prowlarr (~85/s), tracearr (~74/s), scrutiny (~45/s). Queue capacity 10k = ~31s buffer. Separate investigation queued
- **Security review clean**: Auth on all POST routes via `forced_policy=Mounted` wrapper; SSRF closed on `notifications_test` via `deny_unknown_fields` + server-side URL only; local `notify test` unconditionally bails

## Technical Decisions

- **Pure plumbing approach**: Service layer already had all business logic; added only routing/dispatch code, no new SQL or service methods
- **`deny_unknown_fields` on all new API query/body structs**: Prevents future fields from silently passing through without validation, consistent with existing pattern
- **`actor = "cli"` / `actor = "api"` for ack operations**: No per-user identity in bearer-token auth path; placeholder strings provide audit trail differentiation between surfaces
- **`notify test` HTTP-only in CLI**: Apprise config lives in the server process; local mode would require reading config files that may differ from the deployed server's config — correct to force `--http`
- **Simplifier helpers in `src/cli.rs`**: `match_value()` and `parse_i64_flag()` added as scoped helpers for the 9 new parsers; existing parsers left untouched to avoid convention drift

## Files Modified

| File | Purpose |
|------|---------|
| `src/api.rs` | 10 new REST routes + handlers + `ApiState.notifications_config` field |
| `src/api_tests.rs` | Updated `ApiState` constructor call sites (2 locations) |
| `src/main.rs` | Thread `notifications_config` into `ApiState` constructor |
| `src/cli.rs` | 9 new CLI commands with arg structs, parsers, print formatters, `match_value`/`parse_i64_flag` helpers |
| `src/cli/dispatch.rs` | 9 new `run_*` dispatch functions; idiomatic hash-slice fix |
| `src/cli/dispatch_tests.rs` | 8 new snapshot tests for `into_request()` conversions |
| `src/cli/http_client.rs` | 10 new HTTP client methods for `--http` CLI mode |
| `tests/test_live.sh` | `phase_surface_parity_rest` + `phase_cli_parity` smoke phases |
| `CLAUDE.md` | Updated CLI command table with 9 new commands |
| `docs/contracts/cli-surface.md` | §8 Surface Parity Additions section |
| `docs/contracts/http-endpoints.md` | 10 new REST route entries |
| `docs/contracts/mcp-actions-current.md` | §7 with full specs for 5 previously undocumented MCP actions |
| `docs/superpowers/plans/2026-05-21-surface-parity.md` | Implementation plan |

## Commands Executed

```bash
# Confirmed error scanner running
docker logs syslog-mcp 2>&1 | grep "error_scan"
# → 06:37:42 INFO error_scan: cycle complete rows_processed=7203

# Final verification in worktree
cargo test  # → 907 passed, 1 ignored
cargo clippy --all-targets -- -D warnings  # → No issues found
cargo build --release  # → succeeded

# Pushed to PR
git push -u origin feat/surface-parity
gh pr create --title "feat: surface parity..."
```

## Errors Encountered

- **`docker restart` doesn't re-read `env_file`**: New env vars not picked up by `docker compose restart`. Fixed by `docker rm -f syslog-mcp && docker compose up -d`.
- **Error scanner 1-hour default interval**: Monitor timed out 3 times waiting for signatures. Fixed by adding `SYSLOG_MCP_ERROR_DETECTION_SCAN_INTERVAL_SECS=60`.
- **PR #27 merge conflict**: `src/cli.rs` and `src/main.rs` conflicted between config-cli and http-mode branches. Config command dispatch stranded inside `CliMode::Debug` impl. Fixed manually — kept both sides.
- **Simplifier agent plan deviation**: `IngestRateResponse` uses `buckets.per_sec_1m` not `logs_per_sec`; `UnaddressedErrorsRequest.include_acknowledged` is `Option<bool>` not `bool`. Subagent checked actual models and adjusted.

## Behavior Changes (Before/After)

| Feature | Before | After |
|---------|--------|-------|
| Error signature list | SQLite direct query only | `syslog sig list` / `GET /api/errors/unaddressed` |
| Acknowledge error | MCP session only | `syslog sig ack HASH` / `POST /api/errors/ack` |
| Source IPs | MCP session only | `syslog source-ips` / `GET /api/source-ips` |
| Timeline | MCP session only | `syslog timeline` / `GET /api/timeline` |
| Patterns | MCP session only | `syslog patterns` / `GET /api/patterns` |
| Ingest rate | MCP session only | `syslog ingest-rate` / `GET /api/ingest-rate` |
| Test notification | MCP session only | `syslog --http notify test` / `POST /api/notifications/test` |
| Notifications history | MCP session only | `syslog notify recent` / `GET /api/notifications/recent` |
| Error detection | Disabled (scanner never ran) | Enabled, 60s interval |
| Apprise notifications | Disabled | Enabled, URL `http://198.51.100.2:8766` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test` | 907 passed | 907 passed, 1 ignored | ✅ |
| `cargo clippy --all-targets -- -D warnings` | No issues | No issues | ✅ |
| `cargo build --release` | success | success | ✅ |
| Security review: auth on POST routes | Enforced via `forced_policy=Mounted` | Confirmed | ✅ |
| Security review: SSRF on `notifications_test` | URLs from server config only | Confirmed — `deny_unknown_fields` blocks caller URL | ✅ |

## Risks and Rollback

- **`ApiState` constructor arity change**: Adding `notifications_config` as 8th arg is a compile-time break on any out-of-tree code constructing `ApiState` directly. Internal only; no public API concern.
- **`mcp-actions-current.md` stability claim**: §6 states "All 29 actions listed in §2 are stable". The 5 new actions in §7 are not yet listed in §2's summary table — a follow-up should update the §2 table and bump the count.
- **Rollback**: `git revert` the 8 commits on this branch; no schema migrations, no data mutations.

## Decisions Not Taken

- **`/api/get` CLI command**: Not added — fetching a single log by ID requires knowing the ID upfront, which is only useful in scripting. REST-only is sufficient.
- **`notify test` local mode firing Apprise**: Rejected — apprise config lives in the server process and the local binary may have different/no config. Intentional `bail!`.
- **Restructuring `docs/contracts/cli-surface.md`**: Added §8 rather than retrofitting into epic-specific tables to avoid disrupting existing contract structure.

## Open Questions

- **`mcp-actions-current.md` §2 summary table**: Still shows 29 actions; needs update to 34 after this PR merges
- **Arcane auto-heal bug**: Confirmed v1.19.4 upstream bug (docker-client-refresh races auto-heal context). No upstream fix. Error signature acknowledge pending — scanner still catching up through 2.8M historical logs
- **Ingest queue saturation**: Queue at 100%, prowlarr/tracearr/scrutiny driving 63% of volume. Config fix recommended (`write_channel_capacity=100000`, `batch_size=500`) — not yet applied, awaiting user decision

## Next Steps

**Unfinished (started, not completed):**
- Arcane auto-heal error signature: scanner is running at 60s intervals but needs to catch up through historical logs before the signature appears — then `syslog sig ack <hash>` to suppress

**Follow-on tasks:**
- Apply ingest queue config fix: `write_channel_capacity=100000`, `batch_size=500`, `reconnect_initial_ms=5000` in `/home/jmagar/.syslog-mcp/.env`
- Reduce log levels for prowlarr, tracearr, scrutiny on nashost to `Warn`
- Update `mcp-actions-current.md` §2 table to include 5 new actions (count: 29 → 34)
- Add live smoke coverage for mutating POST routes (`/api/errors/ack`, `/api/errors/unack`, `/api/notifications/test`)
- Close beads issues `syslog-mcp-clq5` and `syslog-mcp-neke` once PR #40 merges

## References

- PR #40: https://github.com/jmagar/syslog-mcp/pull/40
- Beads issue syslog-mcp-clq5: REST API error signature gaps
- Beads issue syslog-mcp-neke: CLI error signature gaps
- Ingest queue investigation report: dispatched subagent (agent ac9c4d51483f1a545)
