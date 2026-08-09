> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-05-16 16:42:57 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 0f94929
plan: docs/superpowers/plans/2026-05-16-enrichment-framework-implementation.md
agent: Claude (claude-sonnet-4-6 / claude-opus-4-7)
session id: 11bb15c0-e1d6-4d45-b3f1-f240195bb6e7
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/11bb15c0-e1d6-4d45-b3f1-f240195bb6e7.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
pr: "#26 feat(enrich): enrichment framework — structured field extraction at ingest — https://github.com/jmagar/syslog-mcp/pull/26 (merged)"
---

## User Request

The user asked for an ELI5 explanation of how mnemo session search works, then asked to brainstorm how to put the homelab observability data to good use — covering all five value lanes (reliability, security, AI self-reflection, knowledge/RAG, notifications/digests).

## Session Overview

Extremely long session covering: full brainstorm → 6 design specs + 21 contracts → 12 follow-up beads → prerequisite task resolution → full Epic B (Enrichment Framework) implementation via subagent-driven development (19 tasks, 6 parsers) → PR #26 opened → 77 PR review threads addressed by parallel agents → critical bug fixed → migration renumbered → PR merged → repo cleaned up.

## Sequence of Events

1. **ELI5 of mnemo search** — explained `search_ai_sessions` FTS5 query + session grouping in `src/db/queries.rs:290–404`
2. **Brainstorm session** — identified 7 concrete pain points (OOM on devhost, silent Docker failures, /var/log fills, disk blackholes, IP contention, WSL DNS, subnet collisions); proposed agent-mode + WS + enrichment architecture
3. **Created 6 epics** in beads: Enrichment (B/1wjr P1), Agent mode (A/qgnx P2), API pollers (C/awvr P2), Probe registry (D/fue9 P2), Digest+notifications (E/h6dg P2), RAG over incidents (F/h6da P3); wired dependencies
4. **Dispatched 6 parallel research agents** → wrote 6 design specs (180K total) in `docs/superpowers/specs/`
5. **Iterated on specs** — resolved open questions (Apprise not gotify, `axon ask` collection scoping, host_metrics→metrics_gauge, token rotation, mark_incident_resolved, idx_logs_ts_tag); applied targeted spec patches
6. **Cross-cutting audit** — dispatched 3 parallel audit agents; found missing contracts, source_kind casing drift, metadata_json namespace ownership gaps
7. **Created 21 contracts** in `docs/contracts/` covering all boundaries (DB DDL, Rust traits, JSON-RPC, MCP actions, CLI, HTTP endpoints, config, retention, credentials, etc.)
8. **Created 12 follow-up beads** for audit findings; resolved 5 P1 prerequisites (source_kind casing, token rotation mechanism, host_metrics reconcile, mark_incident_resolved, idx_logs_ts_tag)
9. **Committed design epoch** as `8afdd93` (27 files, 12,553 insertions)
10. **Created worktrees** — `epic-b-enrichment-prereqs` for prereqs; `epic-b-impl` (.worktrees/) for implementation
11. **Subagent-driven development** — 19 tasks across 6 phases; dispatched implementer + spec reviewer + code reviewer per task; 608 tests passing at completion
12. **Opened PR #26** with 23 commits
13. **Addressed 77 review threads** via parallel agents; discovered critical dead-code bug in dispatcher (container_name path wrong)
14. **Rebased** to resolve migration version conflict (our #10 conflicted with main's #10–12; renumbered to #13)
15. **Merged PR #26** — squash merge into main
16. **Cleanup** — removed worktrees, deleted stale branches, pushed session logs

## Key Findings

- **Critical bug discovered in PR review**: `read_container_name()` in `src/enrich/dispatch.rs:153` looked at `docker.container_name` but `docker_ingest/parser.rs` stamps `container_name` at the metadata root — ALL container-based routing (swag, authelia, adguard, fail2ban via alias) was dead code. Fixed in `bc9625c`.
- **Migration collision**: Main's PR #25 used migration versions 10–12 (error signatures, notifications, dedup). Our enrichment migration had to be renumbered 10→13.
- **source_kind casing drift**: Three inconsistent spellings across the corpus (`syslog` vs `syslog-udp`, `docker_stream` vs `docker-stream`). Locked to kebab-case in `docs/contracts/source-kinds.md`.
- **metadata_json namespace ambiguity**: Three writers (parsers, pollers, agent) wrote to one column with no merge-order contract. Addressed in `docs/contracts/metadata-json-shape.md`.
- **execute_batch not atomic**: `rusqlite::execute_batch()` calls `sqlite3_exec` which auto-commits each statement. Migration 13 wrapped in `BEGIN IMMEDIATE; ... COMMIT;`.
- **authelia case bug**: `msg.contains("Unsuccessful")` (capital U) could never match lowercase Authelia messages. Fixed to `to_ascii_lowercase()` with check order reversed.

## Technical Decisions

- **Parser dispatch keyed on kebab-case source_kind** read from `metadata_json.source_kind` (not a separate DB column) — avoids schema migration, matches existing metadata_json pattern
- **`container_name` at metadata root** (not nested under `docker.`) — matches what `docker_ingest/parser.rs` actually stamps
- **`begin immediate` / `commit` wrapping migration 13** — makes the 5 ALTER TABLE + 4 CREATE INDEX + version INSERT atomic
- **Apprise over gotify** for notifications — multi-transport gateway, already deployed in homelab, no transport lock-in
- **`axon ask` with dedicated collection** (`syslog-mcp-incidents`) for RAG — `axon ask` has no payload filter, so isolation by collection is the only viable scoping mechanism
- **Parsers run in-band on writer hot path** (between AI scrub and SQL insert) — no new goroutine/spawn, reuses existing async writer task
- **`container_to_canonical` fold** — operator-renamed containers (`authelia-main`, `nginx-proxy`) fold to canonical parser keys via static match

## Files Modified

**New source files:**
- `src/enrich/mod.rs`, `parser.rs`, `parser_tests.rs` — Parser trait + types
- `src/enrich/dispatch.rs`, `dispatch_tests.rs` — EnrichmentPipeline with LRU debug
- `src/enrich/output.rs`, `output_tests.rs` — merge_output, record_error, stamp_source_kind
- `src/enrich/parsers/{kernel,docker_event,authelia,swag,adguard,fail2ban}.rs` + tests — 6 V1 parsers
- `tests/enrich_pipeline.rs` — 4 end-to-end integration tests
- `tests/fixtures/parsers/**` — golden fixtures for all 6 parsers

**Modified source files:**
- `src/db/pool.rs` — migration 13 (5 columns + 4 partial indexes)
- `src/db/models.rs` — 5 new fields on `LogBatchEntry`
- `src/db/ingest.rs` — INSERT wired with new columns
- `src/syslog/listener.rs` — `stamp_source_kind(SyslogUdp/SyslogTcp)` at listener
- `src/docker_ingest/parser.rs` — `stamp_source_kind(DockerStream/DockerEvent)`
- `src/otlp.rs` — `stamp_source_kind(Otlp)`
- `src/syslog/writer.rs` — `pipeline.dispatch(&mut e)` in `flush_batch`
- `src/ingest.rs` — `WriterContext.pipeline: Arc<EnrichmentPipeline>`
- `scripts/smoke-test.sh` — enrichment assertion (SWAG line → http_status=418)
- `Cargo.toml` — `thiserror = "1"`, `lru = "0.12"`

**Design artifacts:**
- `docs/superpowers/specs/` — 6 spec files (180K total)
- `docs/superpowers/plans/2026-05-16-enrichment-framework-implementation.md` — 19-task plan (100K)
- `docs/contracts/` — 21 contract files (parser-trait.rs, probe-trait.rs, db-additions.sql, current-schema.sql, agent-protocol.md, mcp-actions.md, mcp-actions-current.md, cli-surface.md, config-schema.md, credentials.md, retention-policy.md, runtime-lifecycle.md, data-layout.md, http-endpoints.md, forwarder-dropins.md, source-kinds.md, metadata-json-shape.md, severity-mappings.md, log-row-shape.md, notification-rules.schema.json, incident-card.md)

## Commands Executed

```bash
# Beads epic creation
bd create --type=epic --priority=1 --title="Enrichment framework..." # × 6 epics

# Worktree setup
git worktree add -b epic-b-implementation .worktrees/epic-b-impl worktree-epic-b-enrichment-prereqs

# Full test suite (post-implementation)
cargo test  # 608 passed → 665 passed after rebase with main

# PR management
gh pr create --title "feat(enrich): enrichment framework..." --base main --head epic-b-implementation
python3 .../fetch_comments.py --pr 26 -o /tmp/pr26.json  # 77 threads
python3 .../mark_resolved.py --all --input /tmp/pr26.json  # 77/77 resolved
gh pr merge 26 --squash --delete-branch  # merged

# Cleanup
git worktree remove .worktrees/epic-b-impl
git push origin --delete bd-work/error-detection-and-notifications
git branch -D epic-b-implementation worktree-epic-b-enrichment-prereqs
```

## Errors Encountered

- **Migration version collision** — PR #25 had already used versions 10–12 on main; our enrichment migration (10) conflicted during rebase. Resolved by renumbering to 13 and updating `pool_tests.rs` assertions.
- **Force push failed silently** — `rtk git push --force-with-lease` appeared to succeed but GitHub still showed old SHA. Fixed by explicit `git push origin epic-b-implementation --force-with-lease`.
- **Integration test fixture wrong path** — `swag_row_lands_with_http_status` failed after the container_name path fix; test used `{"docker":{"container_name":"swag"}}` (nested) but the fixed dispatcher reads `{"container_name":"swag"}` (flat). Fixed by updating the fixture helper in `tests/enrich_pipeline.rs:43`.
- **Trailing whitespace in session log** — pre-commit hook rejected `docs/sessions/2026-05-16-error-detection-and-notifications.md`; fixed with `sed -i 's/[[:space:]]*$//'`.

## Behavior Changes (Before/After)

| Surface | Before | After |
|---|---|---|
| `logs` table | 16 columns | 21 columns: +`http_status`, `auth_outcome`, `dns_blocked`, `event_action`, `parse_error` |
| Ingest pipeline | AI scrub only | AI scrub → parser dispatch → structured extraction |
| `syslog search` | FTS only | FTS + filter by `http_status`, `auth_outcome`, `dns_blocked`, `event_action` |
| SWAG 404 line | stored as raw text | `http_status=404`, `event_action="http_request"`, `swag.*` metadata |
| Authelia failure line | stored as raw text | `auth_outcome="failure"`, `authelia.username`, `authelia.src_ip` |
| AdGuard DNS block | stored as raw text | `dns_blocked=1`, `adguard.query`, `adguard.reason` |
| Container routing | dead code (wrong path) | live: `authelia-main` → `AutheliaParser` via `container_to_canonical` |
| Docker containers | no source_kind | stamped `source_kind="docker-stream"/"docker-event"` in `metadata_json` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `cargo test` (post-impl) | 608 pass | 608 pass | ✅ |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean | clean | ✅ |
| `cargo test` (post-rebase) | 665 pass | 665 pass | ✅ |
| `gh pr view 26 --json mergeable` | MERGEABLE | MERGEABLE | ✅ |
| PR review threads | 77 resolved | 77/77 resolved | ✅ |
| `git worktree list` (post-cleanup) | main only | main only | ✅ |
| `git branch -a` (post-cleanup) | main + remote bd-work | main only | ✅ |

## Risks and Rollback

- **Migration 13 is irreversible on a live DB** — the 5 `ALTER TABLE ADD COLUMN` statements cannot be cleanly reversed; rollback would require recreating the DB from backup or manually dropping the columns (SQLite doesn't support `DROP COLUMN` below v3.35).
- **Parser failures never drop data** — `parse_error` column captures failures; raw rows are always written. No data loss risk from bad parser logic.
- **Container routing fix** — the `container_name` path change (`docker.container_name` → `container_name`) is now correct but would break any code that stamped the nested path. Only `docker_ingest/parser.rs` stamps it; confirmed flat.

## Decisions Not Taken

- **Separate `metrics` ingest channel** — considered a full metrics table separate from logs; rejected in favour of storing via parser output in `metadata_json` and the contracts' `metrics_gauge` / `probe_results` tables (Epic D).
- **Parser chaining** — considered allowing multiple parsers per row (e.g., SWAG + HTTP-status rule); rejected for V1; one parser per row via dispatch.
- **`host_metrics` as separate table** — spec A originally pre-created it; audit found it duplicated Epic D's `metrics_gauge`; resolved by removing it from migration 13 entirely.
- **gotify as notification transport** — replaced with Apprise gateway for multi-transport fan-out.

## References

- `docs/contracts/source-kinds.md` — canonical source_kind enumeration (locked to kebab-case)
- `docs/contracts/metadata-json-shape.md` — namespace registry + merge-order
- `docs/contracts/db-additions.sql` — all new DDL (Epic A–F)
- `docs/superpowers/specs/2026-05-16-enrichment-framework-design.md` — implementation spec
- `axon://schema/mcp-tool` — verified `axon ask` has no payload filter (only `collection`, `since`, `before`)
- PR #25: error detection + push notifications (migrations 10–12 on main)
- PR #26: enrichment framework (migration 13) — merged

## Open Questions

- **`thiserror` 1.x vs 2.x** — code reviewer noted that `thiserror 2.0.18` is already a transitive dep; using `"1"` causes dual versions. Can bump to `"2"` (API-compatible for `#[derive(Error)]`) in a follow-up to dedupe the tree.
- **Backfill of 4.9M existing rows** — explicit non-goal for V1; `syslog backfill --since` deferred to V1.1. Operators have no structured fields for pre-migration logs.
- **AdGuard 7-day retention hardcode** — `runtime.rs:55-59` hardcodes 7-day retention for `adguard-*` tags; tracked as bead `syslog-mcp-0dmn` for V1.1 promotion to config.
- **`alert_state` 30-day GC hardcoded** — tracked as `syslog-mcp-d4tk` for V1.1.

## Next Steps

**Unblocked epics** (all blocked by Epic B, now clear):
- `syslog-mcp-awvr` (C) — API pollers: UniFi + AdGuard query log
- `syslog-mcp-h6dg` (E) — Digest + push notifications via Apprise
- `syslog-mcp-h6da` (F) — RAG over incidents (axon, syslog-mcp-incidents collection)

**Epic A still blocked by prerequisite tasks:**
- `syslog-mcp-7xvo` — token rotation mechanism (DONE: pinned to HelloResult inline)
- `syslog-mcp-qgnx` (A) — Agent mode: WebSocket + JSON-RPC 2.0 (ready to plan)
- `syslog-mcp-fue9` (D) — Probe registry (blocked by A)

**Standalone P2 tasks** (not blocking any epic):
- `syslog-mcp-vy59` — lab-auth allowed_emails not honored
- `syslog-mcp-v8nk` — implement distinct exit codes (1/2/3)
- `syslog-mcp-m7iu` — compose_doctor error envelope alignment
- `syslog-mcp-o7yf` — SYSLOG_MCP_SHUTDOWN_TIMEOUT_SECS knob

**Recommended next session**: pick up `syslog-mcp-awvr` (API pollers) — highest ROI per spec, unblocked, no new architecture needed.
