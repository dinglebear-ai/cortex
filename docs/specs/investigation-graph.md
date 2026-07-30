---
title: "Investigation Graph Spec"
created: 2026-06-02
updated: 2026-07-30
---

# Investigation Graph Spec

## Purpose

The investigation graph is a derived read model over Cortex telemetry. It
connects operational logs, Docker identity, AI session metadata, host heartbeat
identity, and error signature summaries into typed entities and relationships
that can be queried by humans, shell scripts, MCP clients, and future narrative
features.

The graph answers questions like:

- What host, service, container, source, project, or session is this connected to?
- Why does Cortex think those entities are connected?
- Which source rows support the connection?
- Is this connection verified, claimed, or inferred?
- Is the projection fresh enough to trust for this investigation?

The graph does not replace raw logs, heartbeats, transcripts, Docker ingest, or
error signatures. Those source tables remain authoritative. Graph rows are
rebuildable summaries with evidence pointers back to source data.

## Goals

- Build an explorable entity/relationship surface from existing ingested data.
- Preserve source-of-truth boundaries: raw source rows remain authoritative.
- Attach evidence to every relationship.
- Make trust level explicit for every entity and relationship.
- Keep graph lookup bounded for agent and CLI use.
- Expose graph projection status so stale or failed graph answers are visible.
- Support deterministic explanation mode over graph evidence.

## Non-Goals

- No LLM inference during graph projection.
- No embedding/vector similarity for graph edges.
- No trigger-maintained writes in the ingest hot path.
- No causal claims from same-window timing alone.
- No first-class incident, command, file, user, port, HTTP route, mount, config,
  or environment-variable nodes in the current version.
- No automatic incremental graph maintenance in the current version.
- No graph UI in the current version.

## Projection Lifecycle

The graph is built with:

```bash
cortex graph rebuild --json
```

The rebuild:

1. Scans source tables in bounded chunks.
2. Writes entities, aliases, relationships, and evidence to staging tables.
3. Publishes in-flight progress through `graph_projection_meta`.
4. Performs a final serialized swap into public graph tables.
5. Updates projection status, source watermark, counts, runtime, and chunk count.

Operators inspect projection state with:

```bash
cortex graph status --json
```

Lookup commands do not implicitly rebuild the graph. They report projection
metadata so callers can decide whether results are ready, stale, degraded, or
failed.

## Current Sources

The current projection reads:

| Source | Purpose |
| --- | --- |
| `logs` | Source identifiers, claimed hostnames, app names, Docker metadata, AI metadata |
| `host_heartbeats_latest` | Verified heartbeat host identities and aliases |
| `error_signatures` | Error signature entities and app/host signature links |

The latest verified production-scale seed ran against the configured Cortex DB
and produced:

```text
source_row_count: 3,713,029
entity_count: 28,402
relationship_count: 23,369
evidence_count: 54,365
source_watermark: logs:3750550;heartbeats:5738;signatures:3028
```

These numbers are evidence from one live rebuild, not a contractual minimum.

## Current Entity Types

| Entity Type | Source | Key Shape | Trust |
| --- | --- | --- | --- |
| `source_ip` | `logs.source_ip` | normalized source identifier | `verified` |
| `host` | `logs.hostname`, Docker host metadata, heartbeat latest, error signature sample host | normalized hostname | `claimed` or `verified` depending on source |
| `app` | `logs.app_name`, error signature sample app | normalized app name | `inferred` |
| `container` | Docker source string or Docker metadata | `docker_host:container_id_or_name` | `verified` |
| `service` | Docker Compose metadata | `docker_host:compose_project:compose_service` | `inferred` |
| `ai_project` | `logs.ai_project` | normalized project path/name | `verified` |
| `ai_session` | AI project/tool/session metadata | `ai_project:ai_tool:ai_session_id` | `verified` |
| `error_signature` | `error_signatures` | `signature_hash:normalizer_version` | `inferred` |

## Current Relationships

| Relationship | Rule | Reason | Trust | Confidence |
| --- | --- | --- | --- | --- |
| `source_ip observed_as host` | A log row has both `source_ip` and `hostname` | `syslog_claimed_hostname` | `claimed` | `0.6` |
| `app emitted_by host` | A log row has both `app_name` and `hostname` | `log_app_name` | `inferred` | `0.5` |
| `container runs_on host` | Docker source/metadata identifies host and container | `docker_container_id` | `verified` | `0.9` |
| `container runs_on service` | Docker Compose metadata identifies service/project | `docker_service_label` | `inferred` | `0.7` |
| `ai_session worked_on ai_project` | AI metadata identifies session and project | `ai_session_project` | `verified` | `0.9` |
| `app matches_signature error_signature` | Error signature summary has a sample app | `error_signature_match` | `inferred` | `0.7` |
| `host matches_signature error_signature` | Error signature summary has a sample host | `error_signature_match` | `inferred` | `0.5` |

## Evidence Model

Every relationship has one or more evidence rows. Evidence rows point back to
source data and include:

- source kind,
- source id,
- source log id when available,
- source heartbeat id when available,
- source signature hash when available,
- observed timestamp,
- reason code,
- reason text,
- confidence delta,
- trust level,
- safe excerpt,
- metadata path,
- aggregated evidence count.

Evidence is intentionally bounded and deduplicated. Repeated observations of
the same relationship increment `evidence_count` instead of persisting one
unbounded row per raw event forever.

Example:

```text
relationship: container tootie/<container_id> runs_on service tootie/sabnzbd/sabnzbd
reason: docker_service_label
trust: inferred
evidence source: logs row with source_ip=docker://tootie/sabnzbd/stdout
metadata path: metadata_json.compose_service
safe excerpt: tootie/sabnzbd/sabnzbd
```

## Query Surfaces

Current direct CLI:

```bash
cortex graph status [--json]
cortex graph rebuild [--json]
cortex entity <entity-type> <key> [--limit N] [--json]
cortex entity <entity-type:key> [--json]
cortex entity --alias-type TYPE --alias-key KEY [--limit N] [--json]
cortex graph around <entity-type> <key> [--limit N] [--json]
cortex graph around <entity-type:key> [--limit N] [--json]
cortex graph explain <entity-type> <key> [--depth N] [--beam-width N] [--max-chains N] [--json]
cortex graph explain <entity-type:key> [--json]
cortex graph evidence <evidence-id> [--payload-budget BYTES] [--json]
```

Shared service responses are also used by MCP and REST graph surfaces.

Relationship JSON includes the compatibility ids (`src_entity_id`,
`dst_entity_id`) plus optional compact `src_entity` and `dst_entity` summaries
when the endpoints are available. Evidence lookup returns the safe evidence row,
owning relationship, endpoint summaries, projection metadata, and a bounded
`source_log_summary` for log-derived evidence. `source_log_summary` contains
only scalar display fields and a redacted message preview; it never contains the
raw syslog frame or full `metadata_json`.

## Current Limitations

- Heartbeats currently verify host identity/aliases but do not yet create state
  edges for pressure, reboot, disk, memory, or network signals.
- Error signature links are summary-level connections, not per-log causal
  chains.
- Full-rebuild runtime on multi-million-row databases is minutes-scale; an
  incremental watermark-based delta path is the default at high row counts.
- `docker_network` and `reverse_proxy_config` topology is projected from the
  homelab inventory snapshot only (the docker log rows carry no network or
  reverse-proxy membership); `compose_config` is projected from both the
  inventory snapshot and docker compose labels on log rows.

## Future Work

- Add first-class graph nodes for incidents, commands, ports, HTTP routes, and
  operational findings.
- Add heartbeat state edges such as `host has_pressure memory_pressure`.
- Add bounded time-window correlation as query-time context, not persisted
  causal edges.
- Add incremental rebuild or targeted refresh once full projection semantics are
  stable.
