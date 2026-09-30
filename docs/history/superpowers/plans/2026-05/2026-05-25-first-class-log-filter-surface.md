# First-Class Log Filter Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an explicit filter-only log retrieval surface across CLI, REST, and MCP while preserving existing `search` behavior.

**Architecture:** Contract first, then one shared app-layer filter model. `filter` is a queryless wrapper over the existing non-FTS `search_logs` path; alias validation and mapping are centralized before DB SQL construction so CLI, REST, and MCP cannot drift.

**Tech Stack:** Rust, SQLite/rusqlite, Axum REST, rmcp MCP, existing hand-written CLI parser.

---

## Locked Decisions

- `source_ip` remains exact identity. Prefix/range behavior belongs to derived aliases, not `source_ip`.
- `filter` rejects FTS query text everywhere: CLI positionals, REST `query`, and MCP `query`.
- MCP `action=filter` must use strict typed deserialization, not loose `string_arg` extraction.
- V1 avoids broad `json_extract(metadata_json, '$.source_kind')` scans. JSON-only source kinds are reject/time-bound and any expression index is a follow-up bead.
- Alias mapping lives in service/db code, not CLI/REST/MCP.

## File Map

- Create: `docs/contracts/log-filter-surface.md` — contract, cost matrix, examples, out-of-scope items.
- Modify: `src/app/models.rs` — add `LogFilterFields` and `FilterLogsRequest`; reuse fields in `SearchLogsRequest` if practical.
- Modify: `src/app/service.rs` — add `filter_logs`; centralize request-to-`SearchParams` mapping and validation.
- Modify: `src/db/models.rs` — add derived filter fields needed by SQL, such as source ranges and event action.
- Modify: `src/db/queries.rs` — apply derived filters in `append_filters`; keep `search_logs` non-FTS branch reused.
- Modify: `src/db/queries_tests.rs`, `src/app/service_tests.rs` — behavioral and query-plan tests.
- Modify: `src/api.rs`, `src/api_tests.rs` — add `/api/filter` with strict query deserialization.
- Modify: `src/mcp/actions.rs`, `src/mcp/schemas.rs`, `src/mcp/tools.rs`, `src/mcp/tools_tests.rs` — add strict MCP `action=filter`.
- Modify: `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/parse_logs.rs`, `src/cli/run.rs`, `src/cli/dispatch.rs`, `src/cli/http_client.rs`, `src/cli/dispatch_tests.rs`, `src/cli/parse_logs_tests.rs` — add `syslog filter`.
- Modify: `src/main.rs` — recognize `filter` as a DB-backed CLI command.
- Modify: `README.md`, `docs/contracts/mcp-actions-current.md`, `scripts/smoke-test.sh`, `CHANGELOG.md` — docs/smoke/version notes.

### Task 1: Contract

**Files:**
- Create: `docs/contracts/log-filter-surface.md`

- [ ] **Step 1: Write the contract**

Create the contract with these sections:

```markdown
# Log Filter Surface Contract

## Purpose
`filter` retrieves logs by structured fields without FTS. `search` remains the FTS-capable action and keeps its backwards-compatible no-query filter behavior.

## Request Fields
Document: hostname, source_ip, severity, severity_min, app_name, facility, exclude_facility, process_id, from, to, received_from, received_to, limit, source_kind, tool, project, session_id, container, docker_host, stream, event_action.

## Cost Matrix
Cheap/indexed: exact source_ip; hostname + time; severity + time; app_name + received_at; event_action + time; full AI tuple with hostname.
Moderate/bounded: container via source_ip range with docker_host; app_name without received time; facility; process_id; project+tool+session without hostname; broad systemd/app filters.
Reject/defer unless time-bounded: JSON-only source_kind; session_id alone; container without host/source prefix; arbitrary metadata JSON.

## Trust Boundaries
hostname is sender-claimed. source_ip is exact source identity. syslog:read can expose logs, scrubbed transcript text/paths, command history metadata, and container metadata.

## Out Of Scope
Arbitrary metadata JSON filters, wildcard source_ip matching, schema migration for source_kind, and rich container metadata filters.
```

- [ ] **Step 2: Verify docs lint is not needed**

Run: `test -f docs/contracts/log-filter-surface.md`
Expected: exit 0.

### Task 2: Shared Filter Model And DB Mapping

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app/service.rs`
- Modify: `src/db/models.rs`
- Modify: `src/db/queries.rs`
- Test: `src/db/queries_tests.rs`
- Test: `src/app/service_tests.rs`

- [ ] **Step 1: Add failing tests**

Add focused tests for:

```rust
filter_logs_rejects_queryless_source_kind_without_bounds
filter_logs_rejects_session_id_without_tool_project_or_time_bounds
filter_logs_maps_agent_command_source_kind
filter_logs_maps_shell_history_source_kind
filter_logs_maps_transcript_tool_project_session
filter_logs_filters_docker_event_action
filter_logs_plan_does_not_use_logs_fts
```

Run each focused test separately because this repo accepts one cargo filter at a time:

```bash
cargo test filter_logs_rejects_queryless_source_kind_without_bounds
cargo test filter_logs_plan_does_not_use_logs_fts
```

Expected: fail before implementation.

- [ ] **Step 2: Implement model**

Add `LogFilterFields` and `FilterLogsRequest` in `src/app/models.rs` with `#[serde(deny_unknown_fields)]`. Include the contract fields and keep `query` absent from `FilterLogsRequest`.

- [ ] **Step 3: Implement service validation**

Add `SyslogService::filter_logs(req)` and a shared conversion helper that produces `SearchParams { query: None, ... }`. Validation rules:

- `source_ip` is exact.
- `source_kind=agent-command` maps to source prefix/range or facility/app convention for agent commands.
- `source_kind=shell-history` maps to shell-history convention.
- `tool/project/session_id` map to AI columns.
- `session_id` alone requires time bounds or returns invalid input.
- JSON-only `source_kind` values require `from` and `to` or return invalid input.

- [ ] **Step 4: Implement DB predicates**

Extend `SearchParams` only as needed for parameterized derived filters. Prefer exact/range predicates over `LIKE '%...'`. Keep the non-FTS branch in `search_logs` as the execution path.

- [ ] **Step 5: Verify focused tests**

Run:

```bash
cargo test filter_logs
cargo test search_logs
```

Expected: pass.

### Task 3: CLI, REST, And MCP Surfaces

**Files:**
- Modify: `src/api.rs`, `src/api_tests.rs`
- Modify: `src/mcp/actions.rs`, `src/mcp/schemas.rs`, `src/mcp/tools.rs`, `src/mcp/tools_tests.rs`
- Modify: `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/parse_logs.rs`, `src/cli/run.rs`, `src/cli/dispatch.rs`, `src/cli/http_client.rs`, `src/cli/dispatch_tests.rs`, `src/cli/parse_logs_tests.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add failing surface tests**

Add tests for:

```rust
parse_filter_rejects_positional_query
filter_args_into_request_snapshot
run_filter_http_sends_exactly_one_request
api_filter_rejects_query_param
mcp_filter_rejects_unknown_fields
mcp_action_enum_contains_filter
```

- [ ] **Step 2: Add CLI command**

Add `CliCommand::Filter(FilterArgs)`, `parse_filter`, `run_filter`, and `HttpClient::filter`. `parse_filter` accepts only flags and rejects any positional token.

- [ ] **Step 3: Add REST route**

Add `GET /api/filter`, deserialize into the strict filter query/request, and call `service.filter_logs`.

- [ ] **Step 4: Add MCP action**

Add `filter` to `ACTION_SPECS` with read scope and cheap/moderate cost. In `tool_syslog`, deserialize args into `FilterLogsRequest` strictly after removing/validating `action`, then call `service.filter_logs`.

- [ ] **Step 5: Verify surfaces**

Run:

```bash
cargo test parse_filter
cargo test run_filter
cargo test api_filter
cargo test mcp_filter
```

Expected: pass.

### Task 4: Docs, Smoke, Version

**Files:**
- Modify: `README.md`
- Modify: `docs/contracts/mcp-actions-current.md`
- Modify: `scripts/smoke-test.sh`
- Modify: `CHANGELOG.md`
- Modify version-bearing files according to repo rules.

- [ ] **Step 1: Document examples**

Add examples for:

```bash
syslog filter --since 2026-05-24T20:00:00Z --until 2026-05-24T21:00:00Z
syslog filter --source-kind docker-stream --container swag --stream stdout --since ... --until ...
syslog filter --source-kind docker-event --event-action die --since ... --until ...
syslog filter --tool claude --project /home/jmagar/workspace/syslog-mcp --since ... --until ...
syslog filter --source-kind agent-command --since ... --until ...
syslog filter --source-kind shell-history --since ... --until ...
syslog filter --app systemd --since ... --until ...
```

- [ ] **Step 2: Add smoke coverage**

Extend smoke so one test calls `action=filter` and one equivalent no-query `action=search`, asserting both return the same response shape. Do not require Docker ingest.

- [ ] **Step 3: Bump version and changelog**

Use the repo script:

```bash
scripts/bump-version.sh patch
```

Expected: version-bearing files and `CHANGELOG.md` are updated consistently.

- [ ] **Step 4: Run final gates**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
./scripts/check-version-sync.sh
git diff --check
```

Expected: all pass.

## Self-Review

- Spec coverage: contract, DB/app mapping, CLI/REST/MCP, docs/smoke/version are covered.
- Placeholder scan: no TBD/TODO placeholders.
- Type consistency: `LogFilterFields`, `FilterLogsRequest`, and `filter_logs` are the planned shared names across tasks.
