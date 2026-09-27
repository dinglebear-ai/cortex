# MCP implementation patterns

Use the current code as the implementation reference and [AGENTS.md](../../AGENTS.md) for invariants. Examples copied from earlier versions can omit authorization or dispatch metadata.

## Registry-driven adapters

Cortex exposes one public MCP tool with an `action` discriminator. `src/mcp/actions.rs::ACTION_SPECS` and its supporting modules own action names, handlers, scopes, costs, and input/flag metadata. Schema, help, and dispatch must agree with the registry. Do not create a second hardcoded list in a transport adapter.

MCP adapters decode and validate requests, enforce the applicable policy, call shared `CortexService` behavior in `src/app/`, and serialize bounded typed results. Keep timestamp normalization, defaults, query limits, correlation semantics, and blocking database execution at the shared service boundary rather than duplicating them in CLI, REST, and MCP.

## Persistence and pressure

Syslog parsing/listeners under `src/receiver/` feed the bounded writer in `src/ingest.rs`. Other evidence families have their own typed ingest paths; do not assume every source passes through the syslog queue. In-memory buffering is not durable spooling. Preserve explicit failure and loss accounting.

Logical database-size pressure can delete old evidence according to policy. Low free disk independently blocks writes and does not itself initiate deletion. Trigger/recovery thresholds provide hysteresis; err+ retention exemptions and pressure floors remain distinct. Read `src/db/maintenance.rs`, [CONFIG.md](../CONFIG.md), and the [retention contract](../contracts/retention-policy.md) before modifying these semantics.

Use parameterized SQL and bounded blocking helpers. Preserve connection/maintenance permit ownership, retry limits, cancellation, and transaction semantics. SQLite online backup is different from copying a live WAL database.

## Authentication and evidence

MCP, REST, admin REST, OAuth, and machine-ingest routes have different policies. Use the existing auth helpers and their tests rather than pasting a generic token-comparison snippet into a new route. Shared port ownership does not mean shared credentials. See [AUTH.md](AUTH.md) and [SECURITY.md](../SECURITY.md).

Treat transcript text and claimed hostnames as untrusted data. Retain evidence provenance, source identity, and observed/unknown coverage distinctions. A deterministic investigation must not trigger an LLM assessment without its separate CLI execution boundary.

## Bounded operational behavior

Use existing concurrency limits and supervision. Rate-limit or aggregate repeated failures instead of logging every overloaded event. A retry must account for possible committed effects; checkpoint updates and successful delivery receipts are not interchangeable. Verify limits and timing against the relevant sidecar tests rather than treating a number in this guide as an independent contract.

See [DEV.md](DEV.md), [TOOLS.md](TOOLS.md), [SCHEMA.md](SCHEMA.md), and [architecture.md](../architecture.md).
