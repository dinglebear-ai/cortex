---
title: "Architecture overview"
created: 2026-04-04
updated: 2026-09-27
---

# Architecture overview

The maintained architecture is [docs/architecture.md](../architecture.md). The root [AGENTS.md](../../AGENTS.md) maps source ownership and contribution invariants. Keep detailed module and data-flow documentation there rather than maintaining a second syslog-only diagram here.

Cortex combines network logs, host-local collection, OTLP telemetry, transcript scanning, inventory, and evidence-derived investigation. HTTP MCP, REST, CLI, and the browser workspace share service behavior where applicable; local-only paths retain explicit ownership.

Syslog uses UDP/TCP 1514 and the shared HTTP listener uses port 3100 by default. A shared listener does not imply shared credentials. `CortexService` owns common query behavior; source adapters and typed persistence own their ingestion boundaries. Inventory and graph/Observatory projections are not independent event-history authorities.

For extension work, read [ADDING_SOURCES.md](../ADDING_SOURCES.md). For operational boundaries, read [SECURITY.md](../SECURITY.md), [CONFIG.md](../CONFIG.md), and [SETUP.md](../SETUP.md).
