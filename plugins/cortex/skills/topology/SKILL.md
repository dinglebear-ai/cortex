---
name: topology
description: Answer homelab topology and cross-host correlation questions using cortex's graph-backed inventory — what services run on a host, what depends on what, fleet-wide heartbeat state, correlating events across hosts around a point in time, and graph entity/neighborhood/explanation queries. Use whenever the user asks "what's running on <host>", "what depends on <service>", "fleet health", "what else happened around this time", "correlate this across hosts", or wants an evidence-backed explanation of how two things in the homelab are connected.
---

# Cortex Topology & Correlation

Answer "what's connected to what" and "what else was happening" questions
across the fleet, backed by cortex's inventory graph and heartbeat state.
This is distinct from `incidents` (error/alert triage) and
`troubleshoot` (fixing a specific broken thing) — use this skill when
the question is about structure or cross-host timing, not about diagnosing
a failure.

## Workflow

### 1. Inventory and dependency questions — `map`

`cortex action=map [mode=...] [host=...] [domain=...] [service=...] [host_limit=N] [include_sections=[...]] [finding_types=[...]]`

**No CLI equivalent exists for this action** — it's MCP/REST-only
(`POST /mcp` action=map, or the `/api` surface). `mode` selects the answer
shape:

- omitted or `snapshot` — full inventory dump: `nodes`, `services`,
  `compose_projects`, `reverse_proxies`, `networks`, `storage`,
  `media_services`, `projects`, plus a `cortex_overlay` summary. Use for
  "what's out there" broad surveys.
- `host_services` (needs `host`) — services running on one host.
- `service_dependencies` (needs `service`, as a `service_instance` key
  `host/name` — e.g. `nashost/plex` — or `host` + bare name) — what a
  service depends on / is depended on by. Legacy `host:name` identities
  are rejected with `rejected_legacy_shape`.
- `domain_routes` (needs `domain`) — which reverse-proxy routes serve a
  public domain.
- `findings` — scans for `potential_public_route`, `risky_mounts`, and
  `collector_health` issues; narrow with `finding_types`.

Responses include a `graph_answer` field for graph-backed modes and a
`cache_status`/`freshness` field — treat a stale cache as a caveat on the
answer, not a hard error.

### 2. Single-host or fleet-wide heartbeat state

`cortex action=host_state host=<name> [since=...] [limit=N]`
(CLI: `cortex state host <name> [--since T] [--limit N]`)

`cortex action=fleet_state [include_ok=false] [sort=pressure|freshness|hostname]`
(CLI: `cortex state fleet [--sort ...]`)

Use `host_state` when the user names a specific host ("what's devhost's
state"); use `fleet_state` for "what's unhealthy across the fleet" —
`include_ok=false` (the default) already filters to hosts that aren't
`status == "ok"`, so you don't need to filter the response yourself.

### 3. Correlating events around a point in time

`cortex action=correlate reference_time=<ISO> [window_minutes=N] [severity_min=...] [host=...] [source=...] [query=...] [limit=N]`
(CLI: `cortex correlate events --reference-time <ISO> [--window-minutes N] ...`)

Groups matching logs by hostname into `CorrelatedHost` buckets. **`limit` is
silently capped at 999**, not 1000 — the implementation over-fetches by one
row to detect truncation and `search` already hard-caps at 1000; don't be
surprised if a `limit=1000` request effectively returns 999.

`cortex action=correlate_state reference_time=<ISO> [window_minutes=N] [host=...] [severity_min=...] [limit=N]`
(CLI: `cortex correlate state ...`)

Same idea as `correlate`, but joins in heartbeat summaries per host instead
of just logs — use this when you need to know not just "what logs fired"
but "what state was each host reporting" at that moment. There's also
`cortex correlate topic` (backs the `topic_correlate` action) for resolving
a topic to graph entities first and correlating everything related into one
timeline — reach for that when the user names a concept/service rather than
a bare time window.

### 4. Graph entity, neighborhood, and explanation queries

`cortex action=graph mode=entity key=<name> entity_type=<type> [alias_type=...] [alias_key=...] [limit=N]`
(CLI: `cortex entity <entity-type> <key> [--limit N]`, or the shorthand
`cortex entity <entity-type:key>`, or `cortex entity --alias-type T
--alias-key K` for alias lookups)

Look up one entity. Valid `entity_type` values: `host`, `container`,
`logical_service`, `service_instance`, `app`, `source_ip`, `ai_project`,
`ai_session`, `error_signature`, `compose_project`, `reverse_proxy`,
`domain`, `network`, `storage`, `config_artifact`, `git_commit`, `user`,
`device`.

Service identity is `logical_service` (key like `plex`) plus
`service_instance` (key like `nashost/plex`). Legacy nested identities
(`nashost:plex`, `nashost:plex:plex`, `plex/plex/plex`) are rejected with
`rejected_legacy_shape` — resolve the logical service first, then follow
its `instance_of` instances.

`cortex action=graph mode=around entity_id=<id>|key=<name> [depth=1] [limit=N] [evidence_sample_limit=N]`
(CLI: `cortex graph around <entity-type> <key> [--limit N]`, or
`cortex graph around --entity-id <id> [--limit N]`)

One-hop neighborhood of relationships around an entity (v1 supports depth 1
only) — use for "what's directly connected to X".

`cortex action=graph mode=explain entity_id=<id>|key=<name> [depth<=3, default 2] [beam_width=N] [max_chains=N]`
(CLI: `cortex graph explain <entity-type> <key> [--depth N]
[--beam-width N] [--max-chains N]`, or `cortex graph explain --entity-id
<id> [--depth N]`)

Multi-hop narrative explanation chaining relationships out to `depth` hops
— use when the user wants "how are X and Y connected" rather than just the
immediate neighbors.

`cortex action=graph mode=evidence ...` (CLI: `cortex graph evidence
<evidence-id> [--payload-budget BYTES]`) fetches the underlying evidence
samples backing a specific relationship —
use to back up a claim from `around`/`explain` with concrete log/event
citations rather than asserting the graph edge exists.

All graph-backed responses report `nodes: Vec<HomelabMapNode>` (for `map`)
or relationship/evidence lists bounded by `payload_budget` — treat a
non-empty `collection_errors` or truncation flag as a caveat to surface,
not something to silently drop.

## Choosing between these four families

- Know the host/service, want what's on/around it → `map`.
- Know the host, want its live health → `host_state`/`fleet_state`.
- Know roughly when, want what happened everywhere → `correlate` /
  `correlate_state`.
- Know two named things, want how they relate (with citations) →
  `graph` (`around` for direct links, `explain` for multi-hop, `evidence`
  to back a specific edge).

These compose: e.g. `map mode=service_dependencies` can surface an entity
key, which you then feed into `graph mode=explain` for a deeper multi-hop
story with evidence, or into `correlate` at a specific time to see what
else fired alongside it.

## Guardrails

- Don't present `map`'s `snapshot` or `graph`'s neighborhood results as
  currently-true state without noting `cache_status`/`freshness` — the
  inventory is a periodically-refreshed projection, not a live poll.
- Don't claim a dependency or connection exists without it appearing in a
  `map`/`graph` response or its `evidence` — this skill is for
  evidence-backed topology, not inference from naming conventions.
- Respect the `correlate` 999-row cap; if a response looks truncated,
  narrow the window or host filter rather than assuming you saw everything.
- If `map`/`graph` responses report `collection_errors` or a stale
  `freshness`, surface that to the user rather than treating the rest of
  the response as complete.
