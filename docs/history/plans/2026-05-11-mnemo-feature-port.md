# Plan: Port mnemo AI Session Features to cortex

## Objective

Bring the useful `mnemo` AI-session intelligence into `cortex` without breaking the existing syslog or AI-session contracts.

The current repo already has:

- AI metadata columns on `logs`: `ai_tool`, `ai_project`, `ai_session_id`, `ai_transcript_path`.
- A stable CLI surface: `syslog sessions`.
- A stable MCP surface: `action=sessions`.
- Flat log search that already returns AI metadata when present.
- OTLP parsing that captures `session.id` and `project.path` into AI metadata fields.
- Syslog enrichment that can infer AI metadata from Claude/Codex transcript log paths.

The port should extend these paths first. Add new modules only after duplication is real.

## Non-Goals

- Do not add a separate `sessions` table in the first pass.
- Do not rename or remove `syslog sessions`.
- Do not rename or remove MCP `action=sessions`.
- Do not add `recent_sessions` while it is identical to `sessions`.
- Do not add a generic `ai_metadata` JSON column until a concrete caller needs fields that cannot fit the existing columns.
- Do not add heavyweight ranking dependencies unless simple SQL/Rust scoring is not enough.

## Current Surfaces to Preserve

### CLI

Current implemented direct CLI commands are:

- `syslog search`
- `syslog tail`
- `syslog errors`
- `syslog hosts`
- `syslog sessions`
- `syslog correlate`
- `syslog stats`

`syslog sessions` currently lists AI transcript sessions grouped by project, tool, session id, and host. Keep this behavior compatible.

There is no implemented `syslog ai ...` namespace in `src/cli.rs` yet. The `ai` namespace below is planned work for this port.

### MCP

The MCP server exposes one tool, `syslog`, with action dispatch. Preserve `action=sessions` and keep its existing response shape unless a breaking change is explicitly documented.

Existing fields to preserve for session entries:

- `project`
- `tool`
- `session_id`
- `transcript_path`
- `hostname`
- `first_seen`
- `last_seen`
- `event_count`

## Data Model

Use the `logs` table as the source of truth for AI session records.

Keep the existing indexes:

- `idx_logs_ai_project_time`
- `idx_logs_ai_session`

Add indexes only when a specific new query plan needs them. Likely candidates after implementation profiling:

- `(ai_tool, ai_project, timestamp)`
- `(ai_session_id, timestamp)`
- `(ai_project, ai_session_id, timestamp)`

Do not introduce a SQL view until multiple query implementations are repeating the same grouped session subquery.

Add a small durable source/checkpoint table before root transcript indexing ships. Root indexing must not rely on `logs` metadata alone for idempotence.

Required checkpoint/source fields:

- Canonical source path.
- Source kind (`claude_project`, `codex_session`, explicit file, or similar).
- Stable file identity where available: size, mtime, and/or content hash.
- Last indexed offset or record identity if incremental JSONL indexing is supported.
- Last indexed timestamp and error summary.

Add row-level import identity, not only file-level checkpoints. A checkpoint can say where scanning stopped, but duplicate prevention must be enforced with a stable imported-record key such as source id plus byte offset, line number, transcript record id, or content hash. Add the required uniqueness constraint or unique index in the same migration as the checkpoint table.

SQLite migrations currently live inline in `src/db/pool.rs` with `schema_migrations`, not in a migrations directory. Add the checkpoint/import-identity tables as the next schema migration and keep any heavy backfill or index creation out of startup-critical paths unless there is operator-visible logging and a recovery/runbook note.

Checkpoint updates and log inserts must happen transactionally. Do not advance a checkpoint if parsing, storage checks, insert, or FTS maintenance fails partway through a chunk.

`syslog ai index` must be safe to rerun before it is considered complete. `syslog ai add --file` may support a force/reimport mode later, but the default behavior must avoid silent duplicate rows.

Define retention/storage semantics before implementation:

- Storage enforcement and retention can delete AI rows just like other logs.
- Decide whether a later rerun should restore purged transcript rows or treat already-processed source records as intentionally consumed.
- Document the choice in CLI help and docs. If the choice is "processed means processed," add an explicit force/reimport path before users need recovery.

## Privacy and Trust Boundaries

AI transcript indexing is opt-in. The server must not scan local transcript roots at startup as part of this port.

Transcript data is sensitive because it may include prompts, tool output, local paths, usernames, repo names, client names, and secrets. Before scanner implementation, define and test these rules:

- Validate every requested path before scanning. Use `symlink_metadata()` when deciding whether the path itself is a symlink, and use `canonicalize()` only after the symlink policy is known. Do not follow a symlink outside an allowed root by accident.
- Reject or explicitly handle symlinks; do not follow symlink loops.
- Enforce file type and extension expectations for transcript inputs.
- Enforce max file size and max record size limits.
- Restrict default scans to known transcript roots.
- For explicit `--path`, reject broad home/root scans unless an intentional override flag is added.
- For explicit `--file`, parse only supported transcript/export formats.
- Sort directory entries deterministically before ingesting because filesystem iteration order is not stable.
- Count and report per-entry directory/read errors in JSON summaries without stopping the whole scan unless the root itself cannot be read.
- Run transcript messages through the existing secret-scrubbing/enrichment path before storage, or explicitly document and test why transcript indexing has different exposure semantics.
- Report skipped files and parse failures without dumping sensitive content.

Because transcript records are stored in `logs.message`, they become visible through existing raw log surfaces unless filtered. The implementation must make a deliberate compatibility decision before scanner work starts and document it:

- Either AI transcript rows are visible through existing `search`, `tail`, `context`, and `get` actions because `logs` is the source of truth, or
- raw log actions get an explicit filter/default policy for AI-originated rows.

Add a dedicated docs section for this policy. `get` can expose raw frames and `context` can expose nearby messages, so this cannot be documented only as a search/tail concern.

MCP responses that include `ai_project` and `ai_transcript_path` must preserve current compatibility, but docs must state that these fields expose local path information. Any redaction mode must be explicit and tested; do not silently change the existing response shape.

## Implementation Phases

### Phase 1: Contracts and Test Fixtures

Define the exact CLI and MCP contracts before adding behavior.

Tasks:

- Extend `src/app/models.rs` with new response/request types for AI-specific operations.
- Re-export new request/response types through `src/app/mod.rs` if existing service callers expect that pattern.
- Add focused unit fixtures in `src/db/queries_tests.rs` covering multiple tools, projects, sessions, timestamps, and hosts.
- Add baseline parser tests for the existing top-level `syslog sessions` command before adding the `ai` namespace.
- Add CLI parser tests in `src/cli_tests.rs` for the new `ai` namespace.
- Add MCP action tests in `src/mcp/tools_tests.rs` and scope tests in `src/mcp/rmcp_server_tests.rs`.
- Update docs only after names and response shapes are stable.
- Add registry parity tests proving every new MCP action is present in the dispatcher, schema action list, help text, read-scope mapping, and smoke coverage.
- Add privacy tests for transcript path handling and representative-content output.
- Add scale fixtures for grouped search, 5-hour buckets, distinct lists, duplicate indexing, and timeout-safe CLI behavior.

Proposed new CLI commands:

- `syslog ai search QUERY [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--json]`
- `syslog ai blocks [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--json]`
- `syslog ai context --project PATH [--tool TOOL] [--limit N] [--json]`
- `syslog ai tools [--project PATH] [--json]`
- `syslog ai projects [--tool TOOL] [--json]`

Proposed new MCP actions:

- `search_sessions`
- `usage_blocks`
- `project_context`
- `list_ai_tools`
- `list_ai_projects`

Keep `sessions` as the compatibility listing action.

### Phase 1A: Indexing Command Design

Make local transcript indexing a first-class explicit surface, not an incidental side effect of querying.

Chosen direction:

- Indexing is a CLI command surface.
- Indexing is not MCP-exposed in the first pass.
- Indexing is not only a background/library feature.

Commands:

- `syslog ai index [--path PATH] [--json]`
- `syslog ai add --file FILE [--json]`

Semantics:

- `syslog ai index` scans one or more roots and ingests discovered transcript files.
- `syslog ai index --path PATH` scans a specific root, project directory, or transcript directory.
- `syslog ai add --file FILE` ingests one explicit transcript/export file.
- Both commands should return counts for discovered files, ingested records, skipped duplicates, parse errors, and unsupported files.
- JSON output must also include read/permission errors, skipped symlinks, skipped unsafe paths, storage-blocked chunks, and checkpoint state changes.
- Both commands must be safe to rerun by default.
- Root indexing must use the checkpoint/source/import-identity tables described above before it can claim idempotence.
- If an explicit file cannot be deduped, the command must refuse by default or require an explicit force/reimport flag; it must not silently duplicate records.
- All paths must pass the privacy and trust-boundary rules above before scanning.

Default roots:

- `~/.claude/projects`
- `~/.codex/sessions`

Do not hide indexing behind server startup in this port. A future background scanner can reuse the same library path after the CLI behavior exists and is tested.

### Phase 1B: OTLP AI Tool Mapping

Move trustworthy OTLP `ai_tool` extraction before or alongside the first new AI query surfaces.

Reason:

- Existing session queries require non-empty `ai_tool`.
- OTLP can already provide `session.id` and `project.path`.
- If `ai_tool` remains empty, OTLP-originated AI records stay invisible to `sessions`, `search_sessions`, and AI inventories.

Tasks:

- Treat `session.id` as an accepted but opt-in/development OTel semantic attribute, and treat `ai_tool`, `project.path`, and transcript path attributes as local trusted-emitter contracts rather than standard authentication signals.
- Derive `ai_tool` from explicit trusted resource/log attributes when available.
- Define the accepted attribute keys and precedence order. Prefer explicit local keys over inferred producer names.
- Treat `service.name` as useful producer metadata, not proof that a record came from Claude/Codex/Gemini.
- Define which ingress paths are trusted enough for AI attribute mapping. If `/v1/logs` can be reached without a token in loopback/dev or via an upstream gateway, document that OTLP AI fields are producer-supplied and must not be used for authorization decisions.
- Normalize known tool values (`claude`, `codex`, `gemini`) and keep unknown values as `None` unless a trusted mapping exists.
- Enforce length caps on tool, project, session id, and transcript path attributes before indexing.
- Add tests in `src/otlp_tests.rs` for Claude, Codex, Gemini, unknown tools, oversized values, spoofed/noisy attributes, unauthenticated/dev ingress assumptions, and gateway/upstream producer headers if those affect trust.

### Phase 2: DB Analytics

Refactor in place first. Implement analytics in `src/db/queries.rs` before adding new DB modules.

New query functions:

- `search_ai_sessions`
- `get_ai_usage_blocks`
- `get_ai_project_context`
- `list_ai_tools`
- `list_ai_projects`

`search_ai_sessions` should:

- Require an FTS query and reuse `validate_fts_query`.
- Join `logs_fts` to `logs`. Never return directly from `logs_fts`, because delete paths can leave phantom FTS rows until merge/rebuild.
- Restrict to rows with non-empty `ai_project`, `ai_tool`, and `ai_session_id`.
- Group results by `ai_project`, `ai_tool`, `ai_session_id`, and probably `hostname`.
- Return the best snippet/log row for each session.
- Include `first_seen`, `last_seen`, `event_count`, and match count.
- Include candidate count/truncation metadata so callers know when broad matches were capped.
- Enforce an internal candidate cap before grouping/ranking broad FTS results.
- Prefer bounded `from`/`to`, `project`, or `tool` filters for broad queries and document the defaults.
- Capture `EXPLAIN QUERY PLAN` evidence in tests or review notes before adding indexes.
- Do not assert exact `EXPLAIN QUERY PLAN` strings in tests; SQLite wording is not a stable contract. Assert broad plan properties or keep the plan as review evidence.
- Rank by a simple deterministic score before copying mnemo complexity:
  - FTS/BM25 score if available from SQLite. Remember that SQLite FTS5 `bm25()` uses lower scores as better matches.
  - Recency or temporal decay.
  - Density bonus based on matches per session.
- Add search tests/docs for phrase-quoting hyphenated terms and model/path-like tokens because FTS5 bareword parsing treats punctuation such as `-` specially.

`get_ai_usage_blocks` should:

- Bucket AI events into 5-hour windows.
- Return project/tool/session counts per block.
- Include total events and active session count.
- Anchor buckets to UTC epoch.
- Document inclusive/exclusive boundaries.
- Explicitly choose whether buckets, first_seen, and last_seen are based on syslog `timestamp` or `received_at`. Existing session behavior leans on stored event timestamps; do not mix fields accidentally.
- Use epoch math for 5-hour windows instead of copying calendar `strftime` grouping from minute/hour/day timeline code.
- Use bounded default windows, such as recent 7 or 30 days, or require explicit `from`/`to` for unbounded historical scans.
- Keep bucket boundaries deterministic and tested with boundary timestamps.

`get_ai_project_context` should:

- Summarize a single project path.
- Include tools used, sessions, hostnames, first/last seen, event volume, and recent representative entries.
- Define whether representative entries are metadata-only, snippets, or full messages before implementation.
- Filter by AI project/tool/session identity. Do not reuse existing `context_around` blindly, because it is host/time oriented and can mix unrelated rows.
- Avoid N+1 queries. Use fixed aggregate queries plus one bounded window-function query for representative rows.
- Enforce response limits for representative rows and snippet length.

`list_ai_tools` and `list_ai_projects` should:

- Return distinct values with counts and first/last seen.
- Support optional cross-filtering.
- Hard-cap result sizes consistently with existing list actions.
- Normalize and length-cap tool/project values before they can create high-cardinality inventories.
- Use bounded default windows where possible, or document that the action scans all retained AI rows.

Do not create `src/db/ai.rs` during this phase unless `src/db/queries.rs` has working behavior and the resulting duplication or file size creates a concrete maintenance problem.

Placement rule:

- Start `search_ai_sessions` beside existing `list_ai_sessions` in `src/db/queries.rs`.
- Put broader non-basic analytics (`usage_blocks`, `project_context`, and inventory aggregates) in the existing `src/db/analytics.rs` once they are more than thin variants of `list_ai_sessions`.
- If grouped-session SQL starts to diverge or repeat, extract shared helpers before adding another copy.

### Phase 3: Service Layer Wiring

Expose the DB work through `src/app/service.rs`. Keep the first pass in the existing service layer.

Tasks:

- Add request/response models in `src/app/models.rs`.
- Update `src/app/mod.rs` exports for new public app models if needed.
- Add service methods that mirror the DB functions.
- Keep all CLI and MCP surfaces calling service methods, not DB functions directly.
- Avoid an `AiService` extraction until the service layer has duplicated enough AI-specific logic to justify it.
- Do not create `src/app/ai.rs` during this phase unless the new behavior already works and extraction clearly reduces duplication.
- Preserve blocking DB pool isolation and avoid per-session service calls for aggregate endpoints.
- Return truncation/cap metadata through service responses for expensive AI analytics.

### Phase 4: MCP Actions

Update the MCP action dispatcher in `src/mcp/tools.rs`.

Tasks:

- Add the five new actions to `tool_syslog`.
- Add schemas and descriptions in `src/mcp/schemas.rs`.
- Add help text in `src/mcp/tools.rs`.
- Add read-scope mappings in `src/mcp/rmcp_server.rs`.
- Extend `scripts/smoke-test.sh` with structure checks for each new action.
- Add or update tests that fail if `CORTEX_ACTIONS`, `tool_syslog`, help text, and `READ_ONLY_ACTIONS` drift.

Compatibility requirements:

- Existing `action=sessions` continues to work.
- Existing `action=search` remains flat log search.
- New `search_sessions` is session-ranked search, not a replacement for flat search.
- New MCP actions must work under mounted auth, not only loopback development mode.
- Unknown or unmapped MCP actions must remain fail-closed.

### Phase 5: CLI AI Namespace

Update `src/cli.rs` and the top-level command dispatch/usage in `src/main.rs`.

Tasks:

- Add `CliCommand::Ai(AiCommand)`.
- Add `ai` to the top-level direct CLI command whitelist in `src/main.rs`.
- Update `src/main.rs` usage/help text so `syslog ai ...` is discoverable and not routed as an unknown command.
- Keep top-level `sessions` unchanged.
- Route `ai search`, `ai blocks`, `ai context`, `ai tools`, and `ai projects` through the service methods.
- Add `ai index` and `ai add` command parsing according to Phase 1A.
- Keep JSON output available for all new commands.
- Keep flag names consistent with MCP argument names where practical.
- Reject ambiguous broad indexing commands with clear messages before doing any file IO.

Human-readable output should be concise and table-like, matching the current CLI style.

### Phase 6: Local Transcript Scanner

Implement the indexing behavior designed in Phase 1A after the query and API surfaces are tested.

Current repo has `src/ingest.rs`, but that file is async channel/writer plumbing, not local file parsing. Do not put scanner parsing there by default. Start with a focused scanner module or submodule, and keep it decoupled from runtime listener/channel setup.

Tasks:

- Scan known transcript roots:
  - `~/.claude/projects`
  - `~/.codex/sessions`
- Support explicit `--path` and `--file` inputs.
- Parse JSONL transcript files incrementally.
- Normalize transcript records into the existing `LogEntry`/batch insert path.
- Populate `ai_tool`, `ai_project`, `ai_session_id`, and `ai_transcript_path`.
- Avoid duplicate ingestion with the required checkpoint/source/import-identity tables and uniqueness constraints.
- Respect storage-budget/write-block semantics. Direct CLI mode does not run the runtime maintenance tasks, so the scanner must preflight storage and enforce the same budget before each chunk. If ingestion is blocked, report it and do not advance checkpoints.
- Chunk large imports and report progress in JSON summaries without logging transcript content.
- Keep chunk sizes below retained-writer/memory caps and make partial failures retryable.
- Add duplicate-run tests proving a second index pass does not increase row or FTS counts.
- Add failure tests proving checkpoints do not advance on partial insert, storage-block, or parse failure.

### Phase 7: Ranking and Result Quality

Refine ranking after bounded query behavior and compatibility are in place.

Tasks:

- Start with deterministic ranking that is easy to test.
- Include deterministic tie-breakers.
- Add BM25, temporal decay, and density scoring only after baseline ranking is measurable and stable.
- Document whether ranking is intended to match mnemo exactly or is cortex-specific.
- Add ranking tests that account for FTS5 BM25 polarity, where lower values rank better.

### Phase 8: Docs and Smoke Coverage

Update docs in the same change set as the final public-surface implementation.

Docs to update:

- `README.md`
- `docs/CLI.md`
- `docs/mcp/TOOLS.md`
- `docs/mcp/SCHEMA.md`
- `docs/mcp/TESTS.md`
- `docs/expansion.md` if it still describes the feature as future work.
- `plugins/syslog/skills/cortex/SKILL.md` if it references the session/action surface.

Docs must include:

- The explicit raw transcript visibility policy for `search`, `tail`, `context`, and `get`.
- The indexing command surface, default roots, unsafe path rules, and rerun/reimport semantics.
- The OTLP trust model: AI attributes are accepted from trusted emitters and are not authentication or authorization signals.
- FTS query syntax notes for transcript terms, including phrase quoting for hyphenated values.

Verification:

- `cargo fmt`
- `cargo test`
- `cargo clippy`
- `bash scripts/smoke-test.sh` with a running server when MCP behavior changes.
- `bash scripts/check-version-sync.sh` if the branch is being prepared for push.
- Seeded scale tests for grouped FTS caps, usage-block defaults, project-context representative limits, duplicate indexing, and path rejection.
- MCP mounted-auth tests for every new action.
- Live smoke coverage in `scripts/smoke-test.sh`, `tests/test_live.sh`, and `tests/mcporter/test-tools.sh` should validate response structure and key fields for new actions, not only check that action names appear in help text.

## Acceptance Criteria

- Existing `syslog sessions` output remains compatible.
- Existing MCP `action=sessions` output remains compatible.
- `search_sessions` returns grouped/ranked session results that are distinct from flat `search`.
- `usage_blocks` returns deterministic 5-hour buckets.
- `project_context` summarizes one project without requiring a separate sessions table.
- `list_ai_tools` and `list_ai_projects` return counts and first/last seen timestamps.
- The CLI `ai` namespace exposes the new features without crowding the raw syslog command namespace.
- Local transcript indexing can ingest a single file and a root path into existing storage.
- Re-running local transcript indexing does not duplicate log rows or FTS rows.
- Checkpoint/import identity is transactionally updated only after successful inserts.
- Retention/storage behavior for previously indexed transcript rows is documented and tested.
- Broad or unsafe indexing paths are rejected before scanning.
- Transcript privacy behavior is documented and tested.
- OTLP AI-originated records populate `ai_tool` when reliable attributes are present.
- OTLP AI-originated records reject or ignore oversized, spoofed, unknown, or untrusted attributes according to the documented trust contract.
- New AI analytics expose cap/truncation metadata and bounded defaults.
- `project_context` avoids N+1 query patterns and has bounded representative output.
- New MCP actions pass dispatcher/schema/help/read-scope parity tests and mounted-auth tests.
- Tests cover DB queries, service methods, CLI parsing/output, top-level `main.rs` command dispatch, MCP dispatch/schemas, OTLP mapping, FTS quirks, filesystem path handling, and checkpoint failure behavior.

## Deferred Decisions

- Whether to add `ai_metadata` JSON.
- Whether to extract `src/db/ai.rs` or `src/app/ai.rs`.
- Whether to add a SQL view for reusable grouped session summaries.
- Whether ranking should exactly match mnemo or remain a simpler cortex-specific scoring model.
