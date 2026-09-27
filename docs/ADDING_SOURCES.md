# Adding an ingestion source

Cortex has several ingestion families, not one universal plug-in trait. Choose the existing boundary for the evidence being added; a network transport is not a source-format provider. [Architecture](architecture.md) maps the families to current modules.

## AI transcript formats

Start in `src/scanner/providers.rs`. `Provider` and `PROVIDERS` define canonical names, aliases, source kinds, adapter versions, privacy policy, checkpoint metadata, and evidence-lane support. The current provider names are Claude, Codex, Gemini, and Antigravity. Format parsing stays in provider-local scanner modules. `src/scanner/providers/paths.rs` owns safe discovery roots and path classification.

A new format needs a descriptor, safe discovery rules, its parser, and integration into the scanner dispatch. Extend existing code rather than adding separate provider lists in health, forwarding, or graph consumers. Check both the local scanner/watch path (`src/scanner*`, `src/ai_watch*`, `src/cli/sessions_watch.rs`) and host-agent forwarding (`src/agent/ai_transcript.rs`, `src/ai_transcript_ingest*`).

### Preserve evidence semantics

`ProviderLane` distinguishes session metadata, transcript, tool calls, MCP events, skills, hooks, and usage. `AdapterSupport` is a static capability statement: `supported`, `partial`, or `unsupported`. `Coverage` is receipt-backed runtime evidence: `observed`, `partial`, `not_observed`, or `failed`. Supporting a format does not prove this installation ingested it. No observed records must not become a claim of successful zero-result collection.

`CheckpointPolicy` records a locator, revision, and content fingerprint. A canonical filesystem path is mutable, not an immutable source identity. Preserve replay/rewrite detection, privacy transformations, bounded payloads, and committed-checkpoint behavior. Do not advance a delivery checkpoint before durable server acknowledgement. Review [agent-protocol.md](contracts/agent-protocol.md) and the applicable ingest contract before changing the wire shape.

### Tests and delivery checks

Test aliases and safe roots, representative and malformed records, unsupported lanes, truncation and rewrites, repeat/replay behavior, redaction, and source identity. Add sidecar fixtures near the parser and forwarding tests in `src/agent/`; test ingest normalization and persisted coverage too. Verify that session search, extracted events, and graph/Observatory projections consume the same evidence rather than synthetic success markers.

## Other source families

| Evidence | Existing seam |
| --- | --- |
| Network syslog | `src/receiver/` parsing/listeners and `src/ingest.rs` writer |
| Host Docker logs | Host-local agent in `src/agent/`; legacy central pull remains in `src/docker_ingest/` |
| Managed files | `src/filetail*`, agent file-tail forwarding and `src/agent_file_tail_ingest.rs` |
| OTLP logs/metrics/traces | `src/otlp/`, typed metric/trace storage in `src/db/` |
| Heartbeats | `src/heartbeat*`, host agent and heartbeat storage |
| Shell/agent commands | `src/agent/`, `src/shell_history_ingest.rs`, `src/agent_command_ingest.rs` |
| Inventory/configuration | `src/inventory/`, normalized inventory cache, graph projection |

Do not force metrics, inventory/config snapshots, or arbitrary logs into the transcript provider enum. Reuse each family's validation, authorization, size limits, and persistence boundary. Share normalized evidence downstream through the service layer. Agent HTTP delivery and any future WebSocket delivery are transport choices; neither should be a provider name.

For every new source, update the current configuration/setup reference, its contract and tests, and supported coverage claims. Keep a proposed adapter explicitly proposed until its real ingest and replay tests pass.
