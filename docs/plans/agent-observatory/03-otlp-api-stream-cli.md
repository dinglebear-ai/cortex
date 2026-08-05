# Phase 3: OTLP traces/metrics, REST, stream, MCP, and CLI

Prerequisite: Phase 2 green.
Phase gates: G4 OTLP and G5 API/stream.

## AO-041 Extract shared OTLP normalization

- **Deliverable:** one helper extracts host/service/tool/session/project and bounded attributes for logs, spans, and metric points.
- **Files:** `src/otlp/normalization.rs`, refactor `src/otlp/entries.rs`, tests.
- **RED:** precedence fixtures for `session.id`, `session_id`, `gen_ai.conversation.id`, and project keys fail.
- **GREEN:** move logic without changing current log output.
- **Proof:** existing OTLP log tests stay byte/field compatible and new precedence tests pass.
- **Gate:** unknown attributes retained in bounded metadata.
- **References:** contract §9.4.

## AO-042 Decode one OTLP trace span

- **Deliverable:** convert one `ExportTraceServiceRequest` span to normalized DB input.
- **Files:** `src/otlp/traces.rs`, tests.
- **RED:** fixture expects IDs, parent, times, duration, status, service, session, resource/scope JSON.
- **GREEN:** implement one valid span path.
- **Proof:** exact normalized fixture passes and invalid zero/length IDs reject safely.
- **Gate:** no HTTP handler yet.
- **References:** `opentelemetry_proto` types, spec §8.2.

## AO-043 Preserve span events and links

- **Deliverable:** bounded serialization of all event/link fields and dropped counts.
- **Files:** trace converter/tests.
- **RED:** multi-event/link fixture loses attributes or order.
- **GREEN:** normalize to bounded JSON arrays with trace/span IDs in links.
- **Proof:** fixture round-trip matches expected JSON and cap truncation reports diagnostics.
- **Gate:** no prompt/tool content bypasses privacy filter.
- **References:** OTel trace data model.

## AO-044 Persist trace batches idempotently

- **Deliverable:** DB batch insert returns accepted/duplicates/rejected counts.
- **Files:** `src/db/otlp_traces.rs`, tests.
- **RED:** duplicate export creates two rows or incorrect count.
- **GREEN:** transaction plus `ON CONFLICT(trace_id,span_id)` policy.
- **Proof:** export twice leaves one row and reports duplicate without rejection.
- **Gate:** storage-budget block is partial, not canonical-ingest failure.
- **References:** existing `insert_logs_batch` patterns.

## AO-045 Mount functional /v1/traces

- **Deliverable:** authenticated protobuf endpoint returns `ExportTraceServiceResponse` and partial success.
- **Files:** `src/otlp.rs`, trace handler/tests.
- **RED:** current route returns 404; auth/decode/body/cap tests fail.
- **GREEN:** follow logs handler auth, body limit, `spawn_blocking`, persistence, response encoding.
- **Proof:** valid request 200, no/invalid auth 401, malformed protobuf 400, oversized 413, partial invalid span reports rejected count.
- **Gate:** logs route regression suite unchanged.
- **References:** current `logs_handler`, contract §9.1-9.2.

## AO-046 Normalize gauge and sum points

- **Deliverable:** number point conversion including temporality/monotonic/start/time/exemplars.
- **Files:** `src/otlp/metrics.rs`, tests.
- **RED:** integer/double gauge and cumulative/delta sum fixtures fail.
- **GREEN:** emit generic `MetricPointInput` and deterministic point key.
- **Proof:** duplicate fixture yields same point key; sorted attributes make input order irrelevant.
- **Gate:** no f64 NaN JSON corruption; represent safely.
- **References:** metric point key contract.

## AO-047 Normalize histogram points

- **Deliverable:** histogram count/sum/min/max/bounds/buckets/exemplars stored losslessly within caps.
- **Files:** metrics/tests.
- **RED:** histogram fixture drops a bucket or bound.
- **GREEN:** bounded `value_json` shape.
- **Proof:** expected buckets/attributes and cap diagnostics pass.
- **Gate:** mismatched bucket/bound input rejected as point-level partial error.
- **References:** OTel metric protobuf.

## AO-048 Normalize exponential histogram and summary

- **Deliverable:** positive/negative buckets, scale/zero fields, and summary quantiles.
- **Files:** metrics/tests.
- **RED:** official-shape fixtures fail.
- **GREEN:** complete remaining instrument variants.
- **Proof:** all five instrument kinds serialize with stable point keys.
- **Gate:** unknown future data kind becomes rejected point, not panic.
- **References:** spec §8.3.

## AO-049 Persist metric batches idempotently

- **Deliverable:** DB insert with accepted/duplicate/rejected/storage-blocked counts.
- **Files:** `src/db/otlp_metrics.rs`, tests.
- **RED:** duplicate point key creates another row.
- **GREEN:** bounded transaction/upsert policy.
- **Proof:** repeated batch row count stable and indexes answer run/name queries.
- **Gate:** no unbounded dimensions/index creation.
- **References:** SQL contract migration 47.

## AO-050 Mount functional /v1/metrics

- **Deliverable:** authenticated OTLP metric endpoint with partial-success response.
- **Files:** `src/otlp.rs`, handler/tests.
- **RED:** route 404 and auth/body/decode/cap tests fail.
- **GREEN:** mirror traces handler with metrics types and limits.
- **Proof:** all instrument fixtures accepted; malformed/oversized/auth cases exact.
- **Gate:** request with >point cap rejects excess deterministically.
- **References:** contract §9.3.

## AO-051 Add Claude OTLP fixture

- **Deliverable:** fixture models documented Claude resource/events/traces/metrics and projects to one run.
- **Files:** `tests/fixtures/otlp/claude*`, integration test.
- **RED:** run lacks expected session/tool/project and trace/tool relation.
- **GREEN:** provider normalization additions only if required by official fields.
- **Proof:** expected run/events/spans/metrics, privacy-disabled content absent.
- **Gate:** fixture provenance URL/date documented.
- **References:** Claude monitoring docs in research ledger.

## AO-052 Add Codex OTLP fixture

- **Deliverable:** fixture from current config/schema conventions and static span attributes.
- **Files:** Codex fixture/integration test.
- **RED:** service/tool/session fields fail to associate.
- **GREEN:** narrow Codex adapter or generic mapping.
- **Proof:** projected signal freshness correctly marks missing signals `not_observed`.
- **Gate:** absence of a signal does not make run idle/ended.
- **References:** Codex config schema sources.

## AO-053 Add Gemini OTLP fixture

- **Deliverable:** Gemini log/trace/metric fixture including `session.id`, conversation ID, agent/run/tool metadata.
- **Files:** Gemini fixture/integration test.
- **RED:** one or both session identifiers lost.
- **GREEN:** preserve both original attributes and select precedence per contract.
- **Proof:** one run, correct tool, expected events/points; prompt detail absent by default.
- **Gate:** rewritten transcript plus OTLP deduplicates conceptual activity only where exact source keys say so; distinct evidence remains distinct.
- **References:** Gemini telemetry docs.

## AO-054 Implement opaque cursor codec

- **Deliverable:** stable cursor containing sort value, ID, direction, and filter fingerprint.
- **Files:** `src/app/cursor.rs` or observatory model module, tests.
- **RED:** tampered, malformed, changed-filter, ascending/descending fixtures fail.
- **GREEN:** URL-safe encoding using existing serde/base64 capability or a tiny internal encoder if already available; no new crate without ADR.
- **Proof:** round-trip and concurrent-insert stability tests pass.
- **Gate:** cursor contains no token or sensitive payload.
- **References:** contract §6.

## AO-055 Add repository/worktree query service

- **Deliverable:** paginated list/detail methods with filters and stream cursor.
- **Files:** DB queries, `src/app/models/agent_observatory.rs`, service/tests.
- **RED:** query/cap/cursor/not-found tests fail.
- **GREEN:** parameterized SQL, hard caps, string ID serialization.
- **Proof:** EXPLAIN plan fixtures use expected indexes; inserts after first page do not duplicate page traversal.
- **Gate:** no handler SQL.
- **References:** current app service pattern.

## AO-056 Add agent-run list/detail service

- **Deliverable:** global/repository-constrained run list and full run detail.
- **Files:** DB/service models/tests.
- **RED:** status/tool/host/worktree/branch/time filters and ambiguity evidence absent.
- **GREEN:** implement queries and model conversion.
- **Proof:** contract-shaped JSON fixture matches Rust serialization.
- **Gate:** list payload contains summaries only.
- **References:** JSON Schema `AgentRun`.

## AO-057 Add event and telemetry query service

- **Deliverable:** stable event pagination/search and independent span/metric pages.
- **Files:** DB/service/tests.
- **RED:** asc/desc, event-kind, trace, query, payload cap, span and metric cursor cases fail.
- **GREEN:** bounded queries and response-size guard.
- **Proof:** 10,000-event fixture pages without duplicates/gaps and returns capped payload error when requested.
- **Gate:** FTS use follows existing safe query path; payload opt-in obeys privacy.
- **References:** contract §7.7-7.8.

## AO-058 Mount repository and run REST routes

- **Deliverable:** read-only routes from OpenAPI through `CortexService`.
- **Files:** `src/api/agent_observatory.rs`, `src/api.rs`, tests.
- **RED:** route/auth/unknown-query/404 tests fail.
- **GREEN:** thin handlers, strict query models, error envelope.
- **Proof:** OpenAPI operation paths each have positive and negative API tests.
- **Gate:** all routes under existing bearer middleware.
- **References:** OpenAPI contract.

## AO-059 Implement durable outbox replay query

- **Deliverable:** fetch after cursor with filters, expiry detection, latest/oldest bounds.
- **Files:** DB outbox methods/tests.
- **RED:** cursor at boundary duplicates or skips a row; expired cursor not detected.
- **GREEN:** ascending ID query and filter-safe semantics.
- **Proof:** property fixture over random page boundaries returns each retained ID exactly once.
- **Gate:** replay max enforced by caller and reported.
- **References:** contract §7.9.

## AO-060 Implement authenticated SSE handler

- **Deliverable:** replay then live subscribe with retry directive, keepalive, cancellation, and exact IDs.
- **Files:** `src/agent_observatory/stream.rs`, API route/tests.
- **RED:** missing auth, replay/live boundary, disconnect cleanup, keepalive tests fail.
- **GREEN:** Axum SSE using existing futures/Tokio broadcast.
- **Proof:** stream frames parse; SSE id equals envelope id; abort drops subscriber count.
- **Gate:** URL token rejected/ignored and never logged.
- **References:** architecture durable stream and OpenAPI.

## AO-061 Implement stream reset and backpressure behavior

- **Deliverable:** expired/unknown/replay-cap/lag/version reset envelopes and max-client enforcement.
- **Files:** stream/DB tests.
- **RED:** lagged subscriber blocks publisher or silently misses data.
- **GREEN:** bounded channel, reset/disconnect logic, client semaphore.
- **Proof:** synthetic 10,000-event burst with slow client leaves fast client complete and slow client receives reset.
- **Gate:** no unbounded task/queue growth.
- **References:** contract reset reasons.

## AO-062 Add observatory status and admin reconcile/backfill routes

- **Deliverable:** status response plus admin-only single-flight jobs.
- **Files:** service/API/job tests.
- **RED:** non-admin can start operation; duplicate operation starts twice; status lacks lag/revision.
- **GREEN:** reuse admin validation and jobs primitives; thin operation commands.
- **Proof:** 403/409/dry-run/progress/cancel/result tests pass.
- **Gate:** operations bounded and audited.
- **References:** OpenAPI admin routes, existing jobs implementation.

## AO-063 Add MCP actions

- **Deliverable:** five read actions in authoritative registry, schema generation, dispatcher, tests.
- **Files:** `src/mcp/actions.rs`, schemas/tools/tests, generated docs.
- **RED:** action registry lookup and argument validation fail.
- **GREEN:** call same service methods as REST; apply MCP caps.
- **Proof:** action list count updated by generator; positive/unknown-field/cap tests pass.
- **Gate:** no duplicate MCP server/tool surface.
- **References:** contract §12 and current action registry.

## AO-064 Add CLI agents commands and watch

- **Deliverable:** list/show/events/repositories/status/watch in local and HTTP modes.
- **Files:** CLI args/parse/dispatch/output/http client/tests/help.
- **RED:** parse, URL encoding, JSON output, Ctrl-C watch tests fail.
- **GREEN:** reuse service/client models and stream parser; table output bounded.
- **Proof:** CLI golden tests and a real local-server watch smoke emit each event once then exit 0 on cancellation.
- **Gate:** help, contracts, and legacy commands remain compatible.
- **References:** contract §11 and current grouped CLI patterns.

## Phase 3 gate

```bash
cargo test otlp --lib
cargo test api --lib
cargo test mcp --lib
cargo test cli --lib
cargo test --test agent_observatory_api
cargo test --test agent_observatory_stream
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Required proof:

- official-shape provider fixtures accepted and projected
- OTLP duplicate/partial/auth/body/cap behavior exact
- all OpenAPI read paths implemented and bearer-protected
- cursor pagination stable under concurrent insert
- stream replay/live boundary has no gap or duplicate
- slow client cannot block fast clients or publisher
- REST/MCP/CLI JSON enum and field names match contract fixtures
