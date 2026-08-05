---
title: "Agent Observatory Contract"
created: 2026-07-31
updated: 2026-08-01
---

# Agent Observatory contract

Status: proposed
Contract version: 1
Schema baseline: Cortex 43
Planned schema versions: 44 through 47

Companion artifacts:

- `agent-observatory.sql`
- `agent-observatory.schema.json`
- `agent-observatory.openapi.json`
- `agent-observatory-types.rs`
- `agent-observatory-types.ts`

## 1. Compatibility rules

1. JSON fields are snake_case.
2. Unknown request fields are rejected.
3. Response additions are backward compatible within contract version 1.
4. Removing or changing a field, enum value, identity format, cursor format, or event meaning requires a new contract version.
5. SQLite integer IDs are serialized as decimal strings.
6. Timestamps are UTC RFC3339 with fractional seconds when available.
7. Hex IDs are lowercase.
8. List limits are clamped to server caps and responses report truncation.
9. All `/api/*` routes require the existing bearer token, including loopback.
10. The stream never accepts a token in URL parameters.

## 2. Stable identities

### 2.1 Run key

```text
v1|<host-length>:<host>|<tool-length>:<tool>|<session-length>:<native-session-id>
```

Example:

```text
v1|6:dookie|6:claude|36:00112233-4455-6677-8899-aabbccddeeff
```

Input components are trimmed, Unicode-preserving, and length-counted in UTF-8 bytes. Empty host, tool, or session is invalid. Canonical tools are lower-case `claude`, `codex`, `gemini`, or `unknown:<normalized>`.

### 2.2 Repository key

```text
v1|<host-length>:<host>|<common-dir-length>:<canonical-common-git-dir>
```

### 2.3 Worktree key

```text
v1|<host-length>:<host>|<path-length>:<canonical-worktree-path>
```

### 2.4 Actor key

```text
v1|<run-key-length>:<run-key>|<actor-id-length>:<native-actor-id>
```

### 2.5 Projected event key

```text
v1:<source-kind>:<source-primary-key>:<projection-variant>
```

Source kinds and variants are ASCII lower snake case. Event keys are deterministic and max 1024 bytes.

### 2.6 Metric point key

The metric point key is SHA-256 hex over the canonical tuple:

```text
resource fingerprint, scope, metric name, instrument kind,
start timestamp, point timestamp, sorted attributes, value, exemplar IDs
```

This makes repeated OTLP export idempotent without assuming producer point IDs.

## 3. Enumerations

### Run status

`starting | active | waiting | idle | stale | completed | failed | abandoned`

### Trust level

`verified | claimed | correlated | inferred | refuted`

### Event kind

`lifecycle | transcript | command | shell_history | git_status | git_head | git_commit | file_operation | mcp | hook | skill | llm | otlp_log | otlp_span | otlp_metric | heartbeat | error | provider_event`

### Stream event

`run.created | run.updated | run.status | run.event | worktree.updated | repository.updated | telemetry.updated | observatory.reset`

### Freshness state

`fresh | delayed | stale | not_observed | error`

## 4. Lifecycle reducer contract

Inputs:

- current time
- explicit lifecycle events
- latest substantive activity
- latest waiting/open-span evidence
- optional process-live evidence
- configured windows

Precedence:

1. explicit failed end -> failed
2. explicit successful end -> completed
3. current open wait -> waiting
4. no substantive event yet -> starting
5. activity age <= active window -> active
6. activity age <= stale threshold -> idle
7. activity age < abandoned threshold -> stale
8. age >= abandoned threshold and process not live -> abandoned
9. age >= abandoned threshold and process evidence unavailable -> stale

The reducer returns status, reason code, observed_at, and whether the materialized row changed.

Required reason codes include:

- `explicit_success`
- `explicit_failure`
- `permission_wait`
- `tool_wait`
- `recent_activity`
- `idle_timeout`
- `stale_timeout`
- `abandoned_timeout`
- `no_activity_yet`

## 5. Attribution contract

An attribution candidate contains:

```json
{
  "run_key": "...",
  "worktree_id": "42",
  "kind": "hook_cwd",
  "source": "hook_events:9182",
  "trust": "verified",
  "confidence": 1.0,
  "observed_at": "2026-07-31T23:00:00.000Z"
}
```

Required evidence defaults:

| Kind | Trust | Confidence |
| --- | --- | ---: |
| `hook_cwd` | verified | 1.00 |
| `otlp_session_path` | verified | 0.98 |
| `agent_command_cwd` | verified | 0.98 |
| `transcript_project_path` | verified | 0.95 |
| `lifecycle_host_process` | verified | 0.95 |
| `atuin_cwd_window` | claimed | 0.85 |
| `unique_active_host_cwd` | correlated | 0.75 |
| `timestamp_proximity` | inferred | <= 0.50 |

Refuted evidence remains stored and prevents the same source from selecting the relation until new stronger evidence arrives.

Primary worktree selection orders by non-refuted, confidence descending, trust rank, last seen descending, then worktree ID. A candidate below 0.75 cannot be primary.

## 6. Pagination

List endpoints accept:

- `limit`: default 50, minimum 1, maximum 200
- `cursor`: opaque base64url JSON signed/validated by structure, not a database offset

The cursor encodes sort time, stable ID, filters fingerprint, and direction. A cursor used with different filters returns `400 cursor_filter_mismatch`.

Events default to descending order. `order=asc` is supported for transcript-style reading. Cursor ordering must remain stable while newer events arrive.

Every page returns:

```json
{
  "pagination": {
    "limit": 50,
    "next_cursor": "...",
    "truncated": false
  },
  "as_of": "2026-07-31T23:00:00.000Z",
  "stream_cursor": "19381"
}
```

## 7. REST endpoints

### 7.1 GET /api/repositories

Filters:

- `host`
- `query` over display name and canonical path
- `active_runs_only`
- `include_removed`
- `since`, `until`
- pagination

Response: repository summaries plus page envelope.

### 7.2 GET /api/repositories/{id}

Returns one repository, active and removed worktree counts, last reconcile status, and optional worktrees when `include=worktrees`.

Errors: `404 repository_not_found`.

### 7.3 GET /api/repositories/{id}/worktrees

Filters: branch, dirty, active_runs_only, include_removed, pagination.

### 7.4 GET /api/repositories/{id}/runs

Same run filters as the global run list, implicitly constrained to any current or historical relation to this repository.

### 7.5 GET /api/agent-runs

Filters:

- `repository_id`
- `worktree_id`
- `branch`
- repeated `status`
- repeated `tool`
- `host`
- `query` over run/session/project summary fields
- `since`, `until`
- `active_only`
- pagination

Default sort: `last_activity_at DESC, id DESC`.

### 7.6 GET /api/agent-runs/{run_key}

Returns the run summary plus actors, current/historical worktree relations, event-kind inventory, signal freshness, exact commit summary, and latest stream cursor.

Run keys are path-segment percent encoded. Errors: `404 run_not_found`.

### 7.7 GET /api/agent-runs/{run_key}/events

Filters:

- repeated `kind`
- `severity_min`
- `actor_key`
- `trace_id`
- `query`
- `since`, `until`
- `order=asc|desc`
- `include=payload`
- pagination

Default limit 100, maximum 500. Payload inclusion obeys content/privacy configuration and response byte caps.

### 7.8 GET /api/agent-runs/{run_key}/telemetry

Query:

- `signal=spans|metrics|all`
- `trace_id`
- `metric_name`
- `since`, `until`
- independent span and metric cursors
- independent limits, max 500 each

Response returns span summaries, metric points, aggregate summary, signal freshness, and independent pagination.

### 7.9 GET /api/agent-runs/stream

Headers:

- `Authorization: Bearer <token>` required
- `Accept: text/event-stream` required or implied
- `Last-Event-ID: <decimal outbox id>` optional

Query:

- `after=<decimal id>` optional non-secret fallback
- optional repository/run/worktree filters

Response headers:

```text
Content-Type: text/event-stream
Cache-Control: no-cache, no-store
Connection: keep-alive
X-Accel-Buffering: no
X-Content-Type-Options: nosniff
```

The server emits `retry: 3000` once, 15-second comment keepalives, and records:

```text
id: 19381
event: run.event
data: {"id":"19381",...}

```

The JSON data is a `StreamEnvelope` from the JSON Schema. The SSE `id` and envelope `id` must match.

Replay defaults/caps:

- retained outbox: 24 hours, configurable up to 7 days
- max replay rows: 1000
- per-client channel: 256 notifications
- max stream clients: 256
- max serialized event: 64 KiB

Reset reasons:

- `cursor_expired`
- `cursor_unknown`
- `replay_limit_exceeded`
- `subscriber_lagged`
- `projection_version_changed`

### 7.10 GET /api/agent-observatory/status

Returns feature flags, schema/projection versions, projector cursors/lag, Git watcher counts/errors/overflow, stream clients/outbox bounds, OTLP signal counts/freshness, embedded web revision, Aurora revision, and warnings.

### 7.11 Admin operations

Planned admin-only routes:

- `POST /api/agent-observatory/reconcile`
- `POST /api/agent-observatory/backfill`
- `GET /api/agent-observatory/backfill/{job_id}`

Request bodies reject unknown fields. Reconcile supports repository ID or bounded all. Backfill supports source and cursor range. Both are single-flight per operation class and audit caller/action/result.

## 8. Error envelope

All endpoints use:

```json
{
  "error": "machine_code",
  "message": "safe operator-facing detail",
  "request_id": "optional correlation id",
  "details": {}
}
```

Required status/code mappings:

| HTTP | Code |
| ---: | --- |
| 400 | `invalid_query`, `invalid_cursor`, `cursor_filter_mismatch` |
| 401 | `unauthorized` |
| 403 | `admin_required` |
| 404 | `repository_not_found`, `worktree_not_found`, `run_not_found`, `event_not_found` |
| 409 | `operation_in_progress` |
| 413 | `request_too_large`, `response_too_large` |
| 429 | `stream_client_limit` |
| 500 | `internal_error` |
| 503 | `observatory_disabled`, `projector_unavailable` |

Internal filesystem paths or raw SQL errors must not appear in unauthenticated/error logs. Authenticated detail endpoints may return configured paths as data.

## 9. OTLP endpoint contract

### 9.1 Authentication and protocol

`/v1/logs`, `/v1/traces`, and `/v1/metrics` share the existing OTLP auth policy. The initial implementation supports OTLP/HTTP protobuf. Unsupported content type returns 415. Decode happens in `spawn_blocking`.

Defaults:

- logs body: existing limit
- traces body: 8 MiB
- metrics body: 8 MiB
- max spans/request: 5000
- max metric points/request: 10000
- max resource attributes: 128
- max record/point attributes: 256
- max serialized bounded metadata field: 256 KiB

### 9.2 Trace response

Return `ExportTraceServiceResponse`. Invalid individual spans increment `rejected_spans` and safe `error_message`. A wholly invalid protobuf returns 400.

### 9.3 Metric response

Return `ExportMetricsServiceResponse`. Invalid individual points increment `rejected_data_points`. Duplicate points are accepted idempotently and do not count as rejected.

### 9.4 Provider normalization

Session ID precedence:

1. record/span/point `session.id`
2. record/span/point `session_id`
3. record/span/point `gen_ai.conversation.id`
4. resource values in the same order

Project path precedence:

1. `project.path`
2. `codebase.root_path`
3. `session.cwd`

Tool precedence:

1. explicit Cortex/provider tool attribute
2. `gen_ai.agent.name`
3. known `service.name`
4. unknown normalized service

## 10. Configuration contract

Proposed TOML:

```toml
[agent_observatory]
enabled = true
projector_poll_ms = 500
projector_page_rows = 500
projector_page_bytes = 4194304
active_window_secs = 15
stale_after_secs = 300
abandoned_after_secs = 86400

[agent_observatory.git]
enabled = true
roots = ["~/workspace"]
max_depth = 3
max_repositories = 120
reconcile_interval_secs = 60
debounce_ms = 500
command_timeout_ms = 5000
max_commits_per_transition = 500
store_changed_paths = true
store_author_name = true
store_author_email_hash = false

[agent_observatory.stream]
outbox_retention_secs = 86400
replay_limit = 1000
client_queue = 256
max_clients = 256
keepalive_secs = 15
max_event_bytes = 65536

[agent_observatory.privacy]
include_prompt_content = false
include_tool_content = false
include_command_content = true
include_paths = true
include_user_identity = false
hash_email = true

[agent_observatory.retention]
events_days = 90
spans_days = 30
metrics_days = 30
repository_observations_days = 90
removed_worktrees_days = 365
```

Environment overrides use `CORTEX_AGENT_OBSERVATORY_...` with names documented in `docs/contracts/config-schema.md`. Durations/counts reject zero when zero would disable safety. A dedicated `enabled=false` controls disablement.

### 10.1 Remote transcript-forwarding environment rename

Current name: `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`
Deprecated alias: `CORTEX_AGENT_AI_TRANSCRIPTS`

The value controls only the heartbeat agent's remote HTTP forwarding loop for AI transcript records. It does not control `cortex sessions watch`, transcript parsing, or direct local SQLite ingestion.

| New name | Deprecated alias | Effective value | Required diagnostic |
| --- | --- | --- | --- |
| unset | unset | `false` | none |
| set | unset | parsed new value | none |
| unset | set | parsed legacy value | `legacy_ai_transcripts_env` warning once at startup |
| set | same parsed value | new value | `legacy_ai_transcripts_env` warning once |
| set | conflicting parsed value | new value | `conflicting_ai_transcript_forward_env` warning once |

Generated env files, setup output, deployment templates, and documentation examples emit only `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`. The legacy alias may appear only in the compatibility resolver, compatibility tests, migration documentation, and the deprecation changelog.

Doctor migration behavior under explicit `--fix --yes`:

1. legacy-only: atomically rename the key and preserve the parsed value
2. both equal: remove the legacy key
3. both conflicting: report an error and make no change
4. neither: make no change

File ownership and restrictive permissions must be preserved. A second doctor run is idempotent. The alias remains accepted through Cortex 3.x and is removed in Cortex 4.0 only when parser, warning, doctor migration, tests, docs, and occurrence allowlist are updated together.

### 10.2 Cortex 4.0 removal checklist

When removing the deprecated `CORTEX_AGENT_AI_TRANSCRIPTS` alias in Cortex 4.0, all of the following must be deleted together in a single coordinated change:

1. **Parser compatibility code** in `src/heartbeat_agent.rs`:
   - Remove `AI_TRANSCRIPT_FORWARD_LEGACY_ENV` constant
   - Remove legacy variable resolution logic
   - Remove deprecation and conflict warning emission

2. **Doctor migration** in `src/setup/doctor.rs` and tests:
   - Remove `check_transcript_forward_env_migration()` function
   - Remove `migrate_legacy_only()` and `migrate_both_equal()` helpers
   - Remove all migration tests from `doctor_tests.rs`

3. **Compatibility tests** in `src/heartbeat_agent_tests.rs`:
   - Remove all test cases referencing the legacy variable name
   - Remove `EnvGuard` setup for `CORTEX_AGENT_AI_TRANSCRIPTS`

4. **Deployment regression tests** in `src/agent_deploy_tests.rs`:
   - Remove fixtures containing the legacy variable
   - Remove assertions checking for the legacy name

5. **Documentation updates**:
   - Update `docs/contracts/agent-observatory.md` section 10.1 to remove deprecated alias table
   - Update or remove this section 10.2 removal checklist
   - Update CHANGELOG.md with breaking change notice
   - Remove legacy variable references from plan and research docs (or archive them)

6. **Validation script**:
   - Remove or update `scripts/validate-transcript-forward-env-rename.sh` to check for absence of legacy variable

7. **Setup generation** (if any legacy references remain in comments or examples):
   - Audit `src/setup/heartbeat_agent.rs` for any lingering references
   - Update any help text or examples

Verification after removal:
- `scripts/validate-transcript-forward-env-rename.sh` must report the legacy variable is absent
- All tests must pass without any `CORTEX_AGENT_AI_TRANSCRIPTS` references
- `grep -R "CORTEX_AGENT_AI_TRANSCRIPTS"` should return only this contract section (for historical reference)

## 11. CLI contract

Planned commands:

```text
cortex agents list [filters] [--json]
cortex agents show RUN_KEY [--json]
cortex agents events RUN_KEY [filters] [--json]
cortex agents repositories [filters] [--json]
cortex agents status [--json]
cortex agents watch [filters] [--json]
```

`watch` uses the REST stream in HTTP mode or the same in-process stream service in local mode. Ctrl-C exits 0. JSON watch emits one compact JSON object per line.

## 12. MCP action contract

Actions are read-only in version 1:

| Action | Required input | Output |
| --- | --- | --- |
| `agent_runs` | optional filters/page | run list response |
| `agent_run` | run_key | run detail |
| `agent_run_events` | run_key plus filters/page | event page |
| `repository_runs` | repository_id plus filters/page | run list response |
| `agent_observatory_status` | none | status response |

All action argument schemas deny unknown fields. MCP list caps may be lower than REST and must report truncation.

## 13. Storage contract

The canonical DDL fixture is `agent-observatory.sql`. Production migration split:

- 44: repositories, worktrees, repository observations, exact commits
- 45: runs, actors, run/worktree and run/commit evidence, events, cursors, outbox
- 46: OTLP spans
- 47: OTLP metric points

Migrations are additive and use `INSERT OR IGNORE INTO schema_migrations`. Every migration test verifies fresh creation, schema-43 upgrade, repeated open, required indexes, foreign-key check, and integrity check.

The feature never requires destructive rollback. Disabling supervisors leaves additive tables unused.

## 14. Retention and storage budget

Purge order in bounded batches:

1. expired stream outbox
2. expired metric points
3. expired spans
4. expired run events whose source detail has expired
5. expired repository observations
6. expired removed-worktree relations and rows when unreferenced
7. orphan cleanup after parent retention

Terminal run summaries and exact commit metadata may outlive event detail under configured retention. Counts and freshness must state when detail was retained less than summary.

Storage budget diagnostics must report bytes or row estimates by new table group. Write blocking of observatory detail must not block canonical log ingest; it marks projection/OTLP partial and reports health.

## 15. Web asset contract

`web/out/cortex-assets.json` contains:

```json
{
  "schema": 1,
  "source_revision": "...",
  "aurora_revision": "19e87c...",
  "next_version": "16.2.11",
  "generated_at": "...",
  "assets": [
    {
      "request_path": "/app/agents/",
      "file_path": "agents/index.html",
      "content_type": "text/html; charset=utf-8",
      "cache": "no-store",
      "etag": "sha256-...",
      "inline_script_hashes": ["sha256-..."]
    }
  ]
}
```

The manifest is deterministic except `generated_at`, which is excluded from reproducibility comparison or derived from `SOURCE_DATE_EPOCH`. Paths are normalized forward-slash relative paths and cannot contain `..`.

## 16. Aurora lock contract

`web/aurora.lock.json` records:

- schema version
- repository and full source SHA
- registry URL template
- installation timestamp
- top-level requested items
- resolved transitive items
- installed file paths and SHA-256 hashes before local adaptation
- acknowledged downstream modifications

CI verifies the source SHA is full length, every Aurora import resolves to a vendored file, every locked file exists, no mutable Aurora URL appears in production source, and no feature component imports a non-Aurora parallel primitive.

## 17. Security headers

HTML responses include route-specific CSP. Required minimum:

```text
default-src 'self';
script-src 'self' <exact inline hashes>;
style-src 'self';
style-src-attr 'unsafe-inline';
connect-src 'self';
font-src 'self';
img-src 'self' data:;
object-src 'none';
base-uri 'none';
form-action 'none';
frame-ancestors 'none';
worker-src 'self';
manifest-src 'self'
```

If browser support requires a fallback style policy, it may use `style-src 'self' 'unsafe-inline'` only after a documented security review. Scripts never use `unsafe-inline` or `unsafe-eval`.

## 18. Proof obligations

A contract change is complete only when:

- SQL fixture applies to empty SQLite and passes foreign-key/integrity checks
- JSON Schema and OpenAPI parse and cross-reference
- Rust and TypeScript declarations compile in their target projects
- fixtures serialize identically across Rust and TypeScript expectations
- REST, MCP, CLI JSON, and SSE use the same field names/enums
- unknown-field, cap, redaction, and auth negative tests pass
- documentation indexes and current endpoint/action inventories are updated at implementation time
