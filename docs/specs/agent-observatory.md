---
title: "Cortex Agent Observatory Specification"
created: 2026-07-31
updated: 2026-07-31
---

# Cortex Agent Observatory specification

Status: proposed
Target release: post-3.11
Research: `docs/research/2026-07-31-agent-observatory.md`
Architecture: `docs/design/agent-observatory-architecture.md`
Wire and storage contract: `docs/contracts/agent-observatory.md`

The key words MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, and MAY are normative.

## 1. Product definition

Cortex Agent Observatory is an authenticated, live, repository-centered workspace for observing AI coding-agent sessions. It MUST unify durable evidence from transcripts, commands, shell history, Git, MCP, hooks, skills, LLM invocations, OTLP logs, traces, metrics, and host telemetry into one queryable run model and one chronological session view.

The observatory MUST be available through:

- Cortex REST API
- Cortex MCP action dispatch
- Cortex CLI where a text representation is useful
- a full Next.js App Router application under `/app/agents/`

## 2. User outcomes

An authenticated operator MUST be able to:

1. list every discovered local or forwarded repository
2. expand a repository into its current and removed worktrees
3. see branch, HEAD, dirty state, ahead/behind, lock/prune state, and freshness
4. see active, waiting, idle, stale, completed, failed, and abandoned runs
5. filter runs by repository, worktree, branch, tool, host, status, and time
6. open one run and review an ordered timeline
7. distinguish verified, claimed, correlated, and inferred attribution
8. inspect transcript, command, Git, MCP, hook, skill, LLM, log, span, metric, and host-health evidence
9. follow new events live without page refresh
10. reconnect without duplicate or missing events within the retained replay window
11. search a selected run without searching unrelated sessions
12. identify data freshness and missing provider signals
13. copy stable run, event, trace, span, worktree, and commit identifiers
14. use the complete workflow on desktop and mobile with keyboard and screen reader support

## 3. Scope

### 3.1 Required providers

The first production release MUST support:

- Claude Code transcript, command wrapper, hooks, and OTLP
- Codex transcript and OTLP
- Gemini CLI transcript and OTLP
- provider-neutral OTLP signals carrying a usable session identifier
- local and forwarded Cortex agent streams

### 3.2 Required Git topology

The observer MUST support:

- normal repositories
- linked Git worktrees
- bare repositories when discovered
- detached HEAD
- locked and prunable worktrees
- branch creation/deletion
- commits, fast-forward, rewind, rebase, and force-moved refs
- removed worktrees retained as historical observations

### 3.3 Non-goals

The first release MUST NOT execute commands, control sessions, edit files, retain full source diffs by default, replace a Git forge, or require a production Node.js service.

## 4. Run model

### 4.1 Run identity

A top-level run MUST be uniquely identified by host, canonical tool, and native session identifier using the versioned length-prefixed key defined in the contract.

A provider session that resumes with the same native identifier MUST map to the same run. A provider that emits a new native identifier with `session.previous_id` MUST create a new run and MAY link it to the previous run.

### 4.2 Actors and subagents

A run MAY contain one or more actors. An actor MUST have a provider-native actor identifier when available and MAY have an actor type. Actor events MUST remain queryable in the top-level run timeline. Subagent creation MUST NOT silently create an unrelated top-level run.

### 4.3 Worktree relations

A run MAY relate to multiple worktrees. Each relation MUST carry:

- first and last observation time
- evidence kind and source identity
- trust level
- confidence from 0.0 through 1.0
- current/primary indicator

A relation below 0.75 confidence MUST NOT become the primary worktree without corroborating evidence.

### 4.4 Lifecycle status

Allowed status values are:

- `starting`
- `active`
- `waiting`
- `idle`
- `stale`
- `completed`
- `failed`
- `abandoned`

Explicit terminal provider evidence MUST outrank timeout-derived states. Absence of one provider signal MUST NOT be considered terminal evidence.

The default status windows MUST be configurable and default to:

- active window: 15 seconds
- stale threshold: 300 seconds
- abandoned threshold: 86400 seconds

### 4.5 Freshness

Every run response MUST expose last activity, last projection, and freshness for transcript, command, Git, OTLP log, trace, metric, and provider lifecycle lanes. A lane with no evidence MUST be `not_observed`, not `stale`.

## 5. Event model

### 5.1 Event kinds

The durable timeline MUST support at least:

- lifecycle
- transcript
- command
- shell_history
- git_status
- git_head
- git_commit
- file_operation
- mcp
- hook
- skill
- llm
- otlp_log
- otlp_span
- otlp_metric
- heartbeat
- error

Unknown future provider events MUST be retained as `provider_event` with bounded metadata rather than dropped.

### 5.2 Event identity and idempotency

Every projected event MUST have a deterministic versioned key unique across its source and projection variant. Reprocessing the same source row MUST NOT create another event, increment run counts, or produce another stream notification.

### 5.3 Event ordering

Events MUST be returned in deterministic order. Equal provider timestamps MUST use stable source precedence, source primary key, and durable event ID as tie-breakers. Provider sequence fields MUST be preserved.

### 5.4 Payload policy

List responses MUST return summary payloads. Full bounded payloads MUST require an event detail request or explicit `include=payload` permission. Content MUST pass Cortex scrubbing before persistence and again before response serialization where configured.

## 6. Projection

### 6.1 Source independence

Existing ingestion paths MUST remain functional when the observatory is disabled or degraded. Observatory projection MUST consume durable committed source rows and MUST NOT be required for canonical ingest success.

### 6.2 Cursor transaction

For each source page, event/run/worktree materialization and source cursor advancement MUST commit in one SQLite transaction. A failed transaction MUST leave the cursor unchanged.

### 6.3 Bounded work

Projection MUST enforce configurable caps for rows, decoded bytes, transaction duration, and retries. Defaults MUST prevent one oversized session from monopolizing the SQLite writer.

### 6.4 Backfill

A backfill operation MUST be resumable, idempotent, progress-reporting, cancelable, and safe while live ingestion continues. Progress MUST expose source, last cursor, total-known rows where available, processed rows, emitted events, duplicates, errors, and elapsed time.

### 6.5 Health

Projector health MUST expose enabled/running state, per-source cursor, source max ID, lag rows, last success, last error, retry count, and current batch.

## 7. Repository observation

### 7.1 Discovery

Configured project roots MUST default to the current inventory roots. Discovery MUST be depth and count bounded, reject symlink traversal, and canonicalize paths.

### 7.2 Parsing

Worktree enumeration MUST use `git worktree list --porcelain -z`. Status MUST use `git status --porcelain=v2 --branch -z`. Human-oriented Git output MUST NOT be parsed.

### 7.3 Watching

The observer MUST watch Git control paths and project-root creation points, not every source file. Watch events MUST be debounced and coalesced by repository. Overflow MUST schedule a bounded reconcile. A periodic reconcile MUST repair missed events.

### 7.4 Commit import

HEAD transitions MUST produce exact commit metadata from Git objects. Commit import MUST cap traversal and detect non-fast-forward transitions. Historical commit evidence MUST NOT be deleted after rebase or reset. Reachability MAY change.

### 7.5 Privacy

By default Cortex MAY store commit SHA, parent SHAs, subject, timestamps, author display name, and changed-file summary. It MUST NOT store full diffs, blobs, patch text, or author email in plaintext by default.

### 7.6 Safety

Git commands MUST receive canonical paths discovered by Cortex. API callers MUST NOT supply arbitrary command arguments or filesystem paths to Git execution endpoints.

## 8. OTLP

### 8.1 Protocol

Cortex MUST implement OTLP/HTTP protobuf:

- `POST /v1/logs`
- `POST /v1/traces`
- `POST /v1/metrics`

The new endpoints MUST use the existing OTLP authentication policy, request size limits, source provenance, and blocking decode isolation.

### 8.2 Traces

Trace ingest MUST support all standard OTLP spans, events, links, status, resource, and instrumentation scope fields. Duplicate `(trace_id, span_id)` exports MUST be idempotent. Invalid spans MUST use OTLP partial-success semantics where possible.

### 8.3 Metrics

Metric ingest MUST support gauge, sum, histogram, exponential histogram, and summary point forms. Point values, buckets, quantiles, exemplars, and attributes MUST remain lossless within documented caps.

### 8.4 Normalization

Cortex MUST recognize:

- `session.id`
- `session_id`
- `gen_ai.conversation.id`
- `project.path`
- `codebase.root_path`
- `session.cwd`
- service and host attributes

Original attributes MUST remain available in bounded metadata.

### 8.5 Privacy and cardinality

Prompt, tool, file, user, account, and email fields MUST follow explicit configuration. Session-level metric data MUST NOT be promoted to an unbounded SQLite column/index or time-series dimension.

## 9. Streaming

### 9.1 Authentication

The stream MUST be under the existing authenticated `/api` router. The web client MUST use `fetch()` with Authorization. Tokens MUST NOT be accepted through the query string.

### 9.2 Replay

The stream MUST accept `Last-Event-ID` and MAY accept a non-secret `after` cursor. It MUST replay retained durable events before live subscription and MUST not duplicate replayed/live boundary events.

### 9.3 Reset

When a cursor is expired, unknown, or beyond the replay cap, the server MUST emit `observatory.reset` with a machine-readable reason and latest cursor. The client MUST refetch snapshots before continuing.

### 9.4 Backpressure

Per-client queues, replay count, total clients, event payload size, and keepalive interval MUST be bounded. A lagging client MUST be disconnected or reset without blocking publishers or other clients.

### 9.5 Stream payload

Stream payloads MUST be compact invalidations or summaries. Large transcript, span, metric, or command payloads MUST be fetched from paginated detail endpoints.

## 10. REST API

The release MUST provide:

- `GET /api/repositories`
- `GET /api/repositories/{id}`
- `GET /api/repositories/{id}/worktrees`
- `GET /api/repositories/{id}/runs`
- `GET /api/agent-runs`
- `GET /api/agent-runs/{run_key}`
- `GET /api/agent-runs/{run_key}/events`
- `GET /api/agent-runs/{run_key}/telemetry`
- `GET /api/agent-runs/stream`
- `GET /api/agent-observatory/status`

Admin-only implementation MAY provide force-reconcile and backfill endpoints. Such endpoints MUST use explicit admin authorization, audit logs, dry-run where meaningful, single-flight protection, and hard bounds.

All list endpoints MUST use cursor pagination and hard server caps. Timestamps MUST be RFC3339 UTC. IDs MUST serialize as strings where JavaScript integer precision could be exceeded.

## 11. MCP and CLI

The single Cortex MCP tool registry MUST add read actions equivalent to:

- `agent_runs`
- `agent_run`
- `agent_run_events`
- `repository_runs`
- `agent_observatory_status`

MCP schemas MUST be generated from the authoritative action registry and documented action count must be updated by repository tooling.

CLI commands MUST provide useful text/JSON views for list, show, events, repositories, and status. Live watch MAY be added only if it reuses the same stream contract and honors terminal cancellation.

## 12. Web application

### 12.1 Runtime model

The production application MUST be a Next.js 16 App Router static export embedded in the Cortex binary. It MUST NOT require `next start`, cookies, server actions, middleware/proxy, or route handlers at runtime.

### 12.2 Routes

Required routes:

- `/app/` product landing/navigation
- `/app/agents/` observatory
- `/app/investigate/` existing investigation workflow migrated to Next/Aurora

Deep links MUST return the correct exported HTML and survive reload.

### 12.3 Authentication

The bearer token MUST remain in memory. Clearing or reloading the page MUST remove it. The app MUST NOT write the token to Web Storage, IndexedDB, cookies, URL, analytics, console, or error telemetry.

### 12.4 Aurora

All visible UI primitives and composed blocks MUST come from Aurora pinned to the reviewed full Git commit. The app MUST maintain `web/aurora.lock.json`, an explicit component-use inventory, and a CI audit that rejects parallel bespoke primitives.

### 12.5 Main workspace

The agents route MUST provide:

- repository/worktree navigation
- active/recent run list
- selected run header with tool, host, status, branch, HEAD, freshness, and trust
- filter/search controls
- virtualized unified timeline
- transcript, commands, Git, telemetry, MCP/hooks/skills, and raw evidence tabs or facets
- responsive mobile drawer navigation
- empty, loading, reconnecting, partial, reset, unauthorized, and error states

### 12.6 Live behavior

The page MUST fetch an initial snapshot, connect with the returned/latest cursor, apply compact stream events, and refetch only affected detail where possible. It MUST pause append-follow when the operator scrolls away and clearly offer return-to-live.

### 12.7 Accessibility

Every workflow MUST be keyboard operable. Focus MUST remain predictable during live updates. New events MUST NOT steal focus. Status MUST not rely on color alone. Dynamic announcements MUST be rate-limited and use appropriate live-region politeness. Desktop and mobile views MUST pass axe with no serious or critical findings.

### 12.8 Performance

Long timelines MUST be virtualized. Heavy transcript/graph/telemetry panes SHOULD be dynamically imported. The app MUST meet the budgets in the architecture document and publish bundle analysis in CI.

## 13. Static asset serving

The build MUST generate a deterministic manifest of every exported file, route, MIME type, cache policy, ETag, and inline-script hash. The Rust build MUST fail with an actionable message when the export or manifest is missing or stale.

HTML MUST use no-store. Content-hashed assets MUST use immutable caching. Source maps MUST NOT ship. Path traversal and unknown assets MUST return 404.

The Content Security Policy MUST use exact script hashes and MUST NOT include `script-src 'unsafe-inline'`. Browser tests MUST fail on CSP violations.

## 14. Configuration

New configuration MUST be documented in TOML/env contracts and include safe defaults for:

- observatory enablement
- projector polling/page/byte caps
- status windows
- Git roots, discovery bounds, reconcile interval, command timeout, commit cap
- stream replay retention, replay cap, clients, queue, keepalive
- OTLP trace and metric body/attribute/point caps
- content/privacy controls
- retention periods

Invalid or unsafe configuration MUST fail startup or disable only the affected optional subsystem with an explicit health error, as defined in the config contract.

The heartbeat-agent environment switch for remote AI transcript forwarding MUST be named `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`. `CORTEX_AGENT_AI_TRANSCRIPTS` MUST be treated only as a deprecated compatibility alias and MUST NOT appear in generated configuration. When the new name is present it MUST be authoritative. Legacy-only use MUST remain functional with one deprecation warning; conflicting dual values MUST select the new value and emit one conflict warning. Neither variable may alter the independent local `cortex sessions watch` service.

Doctor MUST detect the deprecated alias. Under explicit `--fix --yes`, it MUST atomically rename legacy-only configuration and remove a redundant equal legacy entry. It MUST refuse to rewrite conflicting values. The alias MUST remain supported for the remainder of Cortex 3.x and is scheduled for removal in Cortex 4.0 with a dedicated release gate.

## 15. Storage and maintenance

New tables MUST use additive migrations after schema 43. Migrations MUST be idempotent and tested from a schema-43 fixture and a fresh database.

Retention MUST remove expired outbox, event, span, metric, observation, and orphaned relation rows in bounded batches. Database budget enforcement, WAL checkpoint, backup, vacuum, integrity checks, stats, and diagnostics MUST include new storage.

## 16. Observability of the observatory

Cortex MUST report:

- projector lag and failures
- Git reconcile duration/errors/overflow
- stream clients/replays/resets/lag disconnects
- OTLP trace/metric requests, accepted/rejected/duplicate records, decode time
- API latency and cap/truncation fields
- web asset revision and Aurora revision

No metric dimension may include raw session ID by default in aggregate metrics.

## 17. Compatibility and rollout

All existing API, MCP, CLI, ingest, and investigation behavior MUST remain compatible until explicitly deprecated. The old investigation app MAY coexist during rollout. Removal requires route parity, acceptance tests, and documented rollback.

Feature flags MUST permit shipping additive schema before enabling projector, Git observer, trace/metric ingest, and UI independently.

## 18. Definition of done

The feature is production ready only when every task gate in `docs/plans/2026-07-31-agent-observatory-implementation.md` is green and the final proof bundle includes:

- clean fresh and upgrade migration evidence
- deterministic replay/backfill/crash recovery
- real Git topology fixtures
- provider OTLP fixtures
- transcript-forward environment rename, alias precedence, doctor migration, and local watcher independence evidence
- authenticated stream replay/reset/load evidence
- full Rust, TypeScript, unit, integration, browser, accessibility, CSP, and release gates
- storage/retention/backup/integrity evidence
- updated public, operator, configuration, API, CLI, MCP, architecture, and security documentation
- a clean worktree and reproducible production binary whose embedded UI reports the reviewed source revisions
