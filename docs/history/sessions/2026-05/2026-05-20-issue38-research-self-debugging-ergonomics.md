---
date: 2026-05-20 18:11:55 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 4f17080
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 279efd31-dd01-4670-b226-f8106bf52433
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/279efd31-dd01-4670-b226-f8106bf52433.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
---

## User Request

Review the newest GitHub issue posted by jmagar (issue #38), run lavra-research on everything in the issue, and update the issue with findings.

## Session Overview

Fetched GitHub issue #38 ("Make syslog self-debugging ergonomic for AI watcher and service failures"), dispatched 5 parallel research agents covering all 6 requested improvements, collected and synthesized findings, and posted a comprehensive research comment to the issue.

No code was written this session — research and documentation only.

## Sequence of Events

1. Invoked `lavra:lavra-research` skill to establish research workflow
2. Fetched issue #38 via `gh issue list --author jmagar --limit 5 --json`
3. Oriented on codebase by grepping `src/mcp/tools.rs` and `src/cli.rs` for relevant symbols
4. Dispatched 5 parallel research agents (all background):
   - **Codebase explorer** — current state of AI watch status, search filters, timeline, doctor, DB schema
   - **Facility/RFC standards** — syslog facility codes, `--exclude-facility` naming, `received_from`/`received_to` design
   - **Incident timeline UX** — CLI design patterns, time parsing libraries, multi-source row format
   - **Journald proxying** — Rust crates for journal reading, shell-out patterns, field semantics
   - **Schema drift detection** — stale process detection, health check patterns, SQLite migration drift
5. Collected agent results as notifications arrived (codebase first, then timeline, schema drift, facility, journald)
6. Wrote synthesized findings to `/tmp/issue38-comment.md` (shell quoting issues required file-based approach)
7. Posted comment via `gh issue comment 38 --body-file /tmp/issue38-comment.md`

## Key Findings

- **`facility` filter already wired end-to-end** (`SearchLogsRequest`, `service.rs`, MCP) — `src/app/models.rs:638`. Only `exclude_facility` and `received_from`/`received_to` are actually missing.
- **`ExecMainStartTimestamp` already fetched but unused** — `src/cli.rs:1311` calls `systemctl_user_output(&["show", "-p", "ExecMainStartTimestamp", ...])` but never stores or compares the value. Schema drift detection is ~2h of wiring.
- **Bug: `FACILITIES[15] = "clock"` should be `"clockd"`** — `src/syslog/parser.rs`. `syslog_loose` 0.21 emits `"clockd"` for facility code 15; parser uses its own array so DB stores `"clock"`. Silent zero-result bug for `--facility=clockd` queries.
- **`journald-query` 0.1.1 broken for user services** — `Query::unit()` matches `_SYSTEMD_UNIT` which is `user@1000.service` for all user services. Correct field is `_SYSTEMD_USER_UNIT`. Shell out to `journalctl` instead.
- **rsyslog drop filter already suppresses watcher logs** — `/etc/rsyslog.d/99-syslog-mcp-forward.conf` drops `$programname == "syslog"`, which includes `syslog-ai-watch.service`. Simplest journal ingest is a one-line filter exemption.
- **`tool_timeline` returns bucket counts, not rows** — `src/mcp/tools.rs:56`, `src/app/models.rs:883`. Naming a CLI command `syslog timeline` would collide with this MCP action. Use `syslog incident`.
- **Most likely root cause of the specific incident**: watcher/server DB path divergence. `RuntimeCore::load_query_only()` opens the DB but does NOT run migrations — only `init_pool()` does. If paths diverged, the watcher would query a DB that only had migrations 1-4 and would never see `transcript_sources`.
- **`read_schema_version()` in `src/api.rs:200`** already exists — needs extraction to shared fn for reuse in CLI status path.
- **`transcript_sources.file_mtime` (INTEGER)** and **`transcript_parse_errors.seen_at`** tables exist today and provide all the data needed for failure tracking without schema changes.

## Technical Decisions

- **Shell out over native crate for journald** — `journald-query` 0.1.1 has `!Send + !Sync` Journal type (requires `spawn_blocking`), wrong field for user units, and needs `libsystemd-dev` at build time. Shell-out via `tokio::process::Command` with `kill_on_drop(true)` is cleaner, correct, and has Vector as production-tested prior art.
- **`syslog incident` not `syslog timeline`** — the existing `timeline` MCP action returns `TimelinePoint { bucket, count }` (aggregates), not rows. A CLI command named `syslog timeline` producing individual rows would be semantically inconsistent with the MCP action.
- **`--exclude-facility` naming** — matches `journalctl --exclude-identifier` precedent and the `--exclude-X` convention across rsync, grep, and GNU tooling. MCP param: `exclude_facility` (snake_case, consistent with existing `exclude_ai` in `SearchParams`).
- **`received_from`/`received_to` distinct from `from`/`to`** — `from`/`to` filter `l.timestamp` (RFC header message time); `received_from`/`received_to` filter `l.received_at` (ingestion time). Both are needed and answer different questions.
- **NULL guard for facility exclusion** — `NULL NOT IN (...)` evaluates to NULL (falsy) in SQLite, silently dropping Docker logs and malformed PRI-header entries. Must use `(l.facility IS NULL OR l.facility NOT IN (...))`.
- **`humantime` crate for CLI time parsing only** — strict `parse_required_timestamp` in `src/app/time.rs` stays RFC3339. Relative parsing (`-10m`, `5m ago`, space-separated ISO) is a CLI-layer translation, never reaching the service boundary.
- **Symmetric window for `syslog incident`** — `--around`/`--minutes` maps 1:1 to `correlate`'s `reference_time`/`window_minutes`. The incident command is `correlate` plus multi-source fan-in.

## Files Modified

None — research-only session. No code changes.

## Commands Executed

```bash
# Fetch newest issue
gh issue list --author jmagar --limit 5 --json number,title,createdAt,body,url

# Codebase orientation
rtk git log --oneline -10
grep -n "search_logs|facility|watch.status|ai_watch" src/mcp/tools.rs src/cli.rs

# Post research comment
gh issue comment 38 --body-file /tmp/issue38-comment.md
# → https://github.com/jmagar/syslog-mcp/issues/38#issuecomment-4494261021
```

## Errors Encountered

**Shell quoting failure on `gh issue comment --body`**: The comment body contained backtick code blocks and special characters that caused zsh parse errors when passed inline. Resolved by writing the body to `/tmp/issue38-comment.md` and using `--body-file`.

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Issue #38 | Open with problem description only | Open with detailed research comment covering all 6 areas |

No code behavior changes — research session only.

## References

- Issue #38: https://github.com/jmagar/syslog-mcp/issues/38
- Research comment: https://github.com/jmagar/syslog-mcp/issues/38#issuecomment-4494261021
- `syslog_loose` 0.21 source: `~/.cargo/registry/src/.../syslog_loose-0.21.0/src/pri.rs`
- Vector VRL journald source: `src/sources/journald.rs` in the Vector repo (shell-out + cursor persistence reference)
- `humantime` docs.rs: `parse_rfc3339_weak`, `parse_duration`
- `journald-query` 0.1.1 crates.io (2025-09): only maintained reader crate but broken for user units

## Open Questions

- Is the facility-15 `"clock"` vs `"clockd"` mismatch worth a DB migration, or just document it as a divergence?
- Should `syslog service logs` only support `syslog-ai-watch.service` initially, or be generic for any user unit?
- Rsyslog drop-filter approach (zero Rust) vs `JournalTailTask` in `ai_watch.rs` (~100 lines) — which fits the roadmap better?
- Does `ai-watch-coord` doctor phase need to run on every `watch-status` call or remain doctor-only?

## Next Steps

**Unfinished (started but not completed):** None — research fully completed and posted.

**Follow-on tasks (not yet started), in priority order:**

1. **Store `ExecMainStartTimestamp` in `AiWatchStatusReport`** — `src/cli.rs:1311`. Parse with `"%a %Y-%m-%d %H:%M:%S %Z"`, store in struct, compare against `db_last_migration_at`. (~2h)
2. **Add `schema_drift_detected` + `schema_drift_migrations` to `watch-status --json`** — extend `read_schema_version()` in `src/api.rs:200` to also return `MAX(applied_at)`, extract to shared fn. (~3h)
3. **Add `last_successful_ingest_at`, `recent_failure_count`, `affected_paths`** — SQL against `transcript_sources` and `transcript_parse_errors` tables. (~2h)
4. **Add schema drift phase to `syslog doctor`** — FAIL when `db_last_migration_at > process_start_time`; emit concrete `systemctl --user restart` fix command. (~2h)
5. **Add `--exclude-facility` and `received_from`/`received_to` to search** — `facility` already wired; these are additive. Fix `FACILITIES[15]` bug (`"clock"` → `"clockd"`) in `src/syslog/parser.rs`. (~3h)
6. **`syslog incident` command** — add `humantime` crate to CLI layer, write `parse_cli_time()`, build multi-source fan-in on top of `correlate` internals. (~1 day)
7. **Journal ingest** — either modify rsyslog drop filter (30m, zero Rust) or `JournalTailTask` in `ai_watch.rs` with cursor persistence (~1 day).
