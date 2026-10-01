---
title: "Cortex architecture"
created: 2026-05-18
updated: 2026-09-30
---

# Cortex architecture

Cortex combines a Rust service, host-local collection agents, a query CLI/MCP surface, deployment tooling, and a browser investigation workspace. It stores canonical evidence in SQLite and builds derived search, inventory, graph, and Agent Observatory views. See [AGENTS.md](../AGENTS.md) for contributor instructions.

## Data flow

```text
Syslog UDP/TCP ────────────────> receiver/parser ─────┐
Host-local agent HTTP batches -> ingest adapters ────┤
OTLP HTTP/protobuf ────────────> OTLP adapters ───────┤
Local transcript/file sources -> scanner/watch ──────┤
Inventory collectors ─────────> normalized cache ────┤
                                                    v
                              validated, bounded persistence
                                                    |
                          SQLite evidence + derived projections
                                                    ^
                                                    |
                  CortexService + shared per-process query limits
                     ^                 ^                   ^
                   /mcp          /api/* and /api/v1/*    local CLI/stdio
                                       ^
                              HTTP CLI / browser /app
```

The diagram groups source families; they do not all use the same table, queue, or transaction. Logs, heartbeats, metrics, traces, ingest receipts, and graph/Observatory projections retain their own typed storage and contracts. A projection is rebuildable evidence-derived state, not a second authoritative event history.

## Module ownership

| Area | Implementation |
| --- | --- |
| Process entry / runtime | `src/main.rs`, `src/runtime.rs`, `src/runtime/` |
| Shared service | `src/app.rs`, `src/app/`; `CortexService`, validation, query and maintenance limits |
| SQLite | `src/db.rs`, `src/db/`; pool, 63 sequential migrations, FTS5, evidence, projections, retention |
| Syslog | `src/receiver.rs`, `src/receiver/`, `src/ingest.rs` |
| Host collection | `src/agent.rs`, `src/agent/`, `src/heartbeat_agent.rs` |
| Forwarded ingest | `src/ai_transcript_ingest*`, `src/agent_command_ingest.rs`, `src/agent_file_tail_ingest.rs`, `src/shell_history_ingest.rs`, `src/syslog_forward_ingest*` |
| Transcript scanning | `src/scanner.rs`, `src/scanner/`, `src/ai_watch*`, `src/cli/sessions_watch.rs` |
| Managed file sources | `src/filetail.rs`, `src/filetail/` and agent file-tail forwarding |
| OTLP | `src/otlp.rs`, `src/otlp/`, `src/db/otlp_metrics*`, `src/db/otlp_traces*` |
| Inventory / graph | `src/inventory*`, `src/db/graph*`, `src/runtime/inventory_refresh*`, `src/runtime/graph_refresh.rs` |
| Agent Observatory | `src/agent_observatory*`, `src/app/agent_observatory*`, `src/db/agent_observatory*`, `src/git_observer*` |
| Public adapters | `src/mcp*`, `src/api*`, `src/cli*`, `src/surfaces*` |
| Browser | `web/`, static export served by `src/web_app.rs` |
| Notifications / LLM assessment | `src/notifications*`, `src/app/llm_runner*`, `src/*assessment*` |
| Setup and updates | `src/compose*`, `src/setup*`, `src/deploy*`, `src/doctor.rs`, `src/update.rs` |

## Sources versus transports

The preferred Docker log path is the **host-local cortex agent**, reading the local Docker socket. `src/docker_ingest/` and `CORTEX_DOCKER_*` retain **legacy central pull** compatibility for explicit remote Docker Engine HTTP endpoints. They are not the default fleet collection architecture.

Agents forward supported evidence over HTTP routes such as `/v1/ai-transcripts`, `/v1/agent-commands`, `/v1/shell-history`, `/v1/file-tails`, and `/v1/syslog-forward`. Do not describe a proposed WebSocket transport as implemented merely because the system streams data.

For Claude and Codex transcript JSONL, the host agent extracts bounded tool-call identity and outcome fields before scrubbing the display message. Claude runtime hook attachments also contribute bounded event names and outcomes. The `/v1/ai-transcripts` receiver commits those normalized event rows with the canonical log and receipt in one transaction. Older agents may still forward only display text, so an empty normalized event table cannot by itself establish that no tool or hook activity occurred. The forwarding coverage remains partial where the source format cannot expose nested calls or runtime hooks.

Transcript source metadata is centralized in `src/scanner/providers.rs`; parsing remains provider-local. Static adapter support differs from receipt-backed observed coverage. Mutable file paths are locators, not immutable source identities. Read [ADDING_SOURCES.md](ADDING_SOURCES.md) and [agent-protocol.md](contracts/agent-protocol.md) before extending these boundaries.

## Query paths and ownership

HTTP MCP runs the single action-dispatch `cortex` tool. `ACTION_SPECS` in `src/mcp/actions.rs` owns action names, scopes, flags, and handlers. The REST API (98 method/path bindings) and CLI have additional/local-only surfaces; `src/surfaces/` and their adapters define those contracts. The REST denominator is guarded by `documented_rest_route_count_matches_router_registrations`; update the registry, reference, and test together.

An installed CLI commonly uses HTTP settings written into the managed environment. Explicit flags and `CORTEX_USE_HTTP` select routing in `src/cli/run.rs`; local-only commands retain their own rules. Direct SQLite consumers are not automatically governed by another process's in-memory service limits. Do not assume every CLI call reaches the container.

`cortex mcp` is local query-only stdio and does not start the ingestion listeners or maintenance scheduler. Local transcript index/watch paths remain available, while remote agents can forward evidence without sharing the server's filesystem or database. `cortex compose doctor` resolves and checks live Compose/listener ownership before lifecycle changes.

## HTTP and trust boundaries

The shared listener defaults to port 3100 and serves MCP, REST, OTLP, host ingest, health, and `/app`. Syslog uses UDP/TCP 1514. Sharing the HTTP port does not make token policies interchangeable: MCP/machine ingest, REST, and privileged REST have separate configuration. OTLP logs have a 4 MiB body cap; metrics and traces have 8 MiB caps. Verify auth behavior in the relevant route code and [SECURITY.md](SECURITY.md).

Hostname/transcript content is untrusted. Persist provenance and coverage gaps, enforce schema and size limits, and acknowledge delivery only according to the applicable durable ingest contract. LLM-assisted assessments are explicit CLI operations behind the shared LLM runner; deterministic MCP investigations must not silently start one.

Forwarded transcript and syslog records retain their raw authentication-principal hostname, transport peer, hostname claim and trust metadata. A separate indexed projection attributes a record to a safe device hostname only when its server-stamped forwarding provenance agrees with the raw row and transport lane. Devices sharing a credential or NAT address can therefore have separate log views. The peer address alone does not identify a device, and a projected hostname remains a claim rather than an authentication identity. Invalid or missing claims stay under the forwarding principal. The inventory preserves full raw principal totals alongside claimed-device views, whose counts overlap. Claimed names do not inherit heartbeat IDs; only an unambiguous direct-host entry can attach one. Metadata size and field limits preserve the server-stamped provenance while discarding excess optional evidence.

Migration 63 installs this derived host-attribution schema without rewriting raw logs, receipts or session identities. New records update attribution in their ingestion transaction. A small indexed catalog of proven forwarding-principal names keeps their identity and alias namespace separate from device names even after retention deletes their last attributed row. Colliding claims remain preserved as evidence but cannot alter source selectors, device counts, display identity or stream bounds. Historical attribution runs as resumable maintenance work in batches of at most 1,000 primary-key rows; each batch commits its projection and cursor together. The upgrade high-water mark bounds historical work, and restart resumes from the durable cursor. The same bounded pass counts retained raw rows, tracks concurrent inserts/deletes transactionally, and repairs legacy host-counter inflation at completion using indexed receipt-time endpoints. Inventory and device filters become complete as this backfill progresses; old records whose provenance was already discarded remain unattributed. See [host identity and filters](api.md#host-identity-and-filters) for the read contract.

## Runtime maintenance

`RuntimeCore::spawn_maintenance_tasks` wires retention, storage enforcement, error detection, inventory work, projection refresh, session/timeline rollups, planner optimization, and managed file tails. Listener and agent streams have their own supervision/retry behavior. Check the implementation and [CONFIG.md](CONFIG.md) for each cadence rather than assuming all background work shares one interval.

Inventory cache refresh and inventory graph projection are separate controls. `CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED` is opt-in, and `CORTEX_GRAPH_REFRESH_INTERVAL_SECS=0` disables scheduled graph refresh. CLI-driven rebuilds remain distinct.

Retention aging, logical DB-size cleanup, and low-free-disk write blocking are separate policies. The ingest queue is not a durable spool. SQLite uses WAL; take online backups or coordinate all writers for an offline copy. See the [retention contract](contracts/retention-policy.md), [storage configuration](CONFIG.md), and current backup runbooks.

## Implementation versus design history

The repo contains Agent Observatory backend/storage/contract and browser implementation. Dated design and implementation plans describe intent at the time they were written; their task counts are not a statement that all planned scope is complete. Evaluate specific capabilities against source, tests, and current contracts.
