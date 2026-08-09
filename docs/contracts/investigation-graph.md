---
title: "Investigation Graph Contract"
created: 2026-06-02
updated: 2026-07-30
---

# Investigation Graph Contract

## 1. Purpose And Status

This document is the compatibility contract for the Cortex investigation graph.
It is derived from [`docs/specs/investigation-graph.md`](../specs/investigation-graph.md).

Status: **implemented v1 contract**.

Any change to graph table semantics, entity types, relationship types, reason
codes, trust levels, evidence fields, rebuild lifecycle, CLI command names, MCP
action shape, REST graph route shape, or projection status semantics MUST update
this contract in the same change.

## 2. Stability Tiers

| Surface | Tier | Compatibility Rule |
| --- | --- | --- |
| Entity type strings | stable | Additive only unless a migration and changelog explain a rename. |
| Relationship type strings | stable | Additive only unless a migration and changelog explain a rename. |
| Reason code strings | stable | Additive only unless a migration and changelog explain a rename. |
| Trust level strings | stable | Additive only. Meaning must not be weakened silently. |
| Graph table names | migration contract | Schema changes require migration tests and changelog entry. |
| Public response fields | additive stable | Existing fields remain unless versioned or explicitly deprecated. |
| CLI command names | stable | Existing commands remain; aliases may be added. |
| Human formatting | ergonomic | May change when JSON contract is preserved. |
| Projection algorithm | internal with observable outputs | May change if entity/relationship/evidence semantics remain compatible. |

## 3. Projection Lifecycle Contract

### 3.1 Rebuild

```bash
cortex graph rebuild [--json]
```

Rebuild MUST:

- be explicit; public lookup commands MUST NOT implicitly trigger it,
- be single-flight,
- publish `projection_status = "building"` at start,
- reset stale count/progress fields at start,
- publish in-flight `source_row_count` and `last_chunk_count`,
- use staging tables before replacing public graph tables,
- preserve public graph tables until the final swap,
- set `projection_status = "ready"` after a successful swap,
- set `projection_status = "failed"` and a redacted `last_error` on failure,
- update `last_started_at`, `last_completed_at`, `source_watermark`,
  `entity_count`, `relationship_count`, `evidence_count`, `last_runtime_ms`,
  and `last_chunk_count`.

If another rebuild is already running, the command returns an `already_running`
outcome and MUST NOT start a second rebuild.

### 3.2 Status

```bash
cortex graph status [--json]
```

Status MUST return:

```json
{
  "projection_status": "ready",
  "last_started_at": "2026-06-02T22:51:14.513Z",
  "last_completed_at": "2026-06-02T23:08:50.712Z",
  "source_watermark": "logs:3750550;heartbeats:5738;signatures:3028",
  "source_row_count": 3713029,
  "entity_count": 28402,
  "relationship_count": 23369,
  "evidence_count": 54365,
  "is_degraded": false,
  "last_error": null,
  "last_runtime_ms": 1055784,
  "last_chunk_count": 371
}
```

Example values are illustrative. Field names and types are contractual.

Valid `projection_status` values:

| Value | Meaning |
| --- | --- |
| `never_built` | Projection tables exist but have not been populated. |
| `building` | A rebuild is in progress; counts may be progress counters. |
| `ready` | Last rebuild completed successfully. |
| `stale` | Projection is known stale. Reserved for future use. |
| `failed` | Last rebuild failed; `last_error` carries a redacted reason. |

## 4. Vocabulary Contract

### 4.1 Entity Types

These entity type strings are valid on lookup surfaces:

```text
host
container
app
source_ip
ai_project
ai_session
error_signature
compose_project
reverse_proxy
domain
network
storage
config_artifact
git_commit
user
device
logical_service
service_instance
```

`logical_service` is the canonical logical service identity (key like `plex`).
`service_instance` is a host-scoped runtime deployment of a logical service
(key like `nashost/plex`). These two types replace the legacy `service`
topology rows as the supported service identity: `nashost:plex`,
`nashost:plex:plex`, and `plex/plex/plex` are stale defect shapes that are
deleted by migration 41 and rejected on lookup surfaces with
`rejected_legacy_shape`. Keep logical identity and deployment topology
separate: `plex` answers "what is this service", `nashost/plex` answers
"where does it run".

The retired `service` entity type is **schema-tolerated but not
lookup-supported**: the SQLite CHECK constraints still accept `service` rows
for migration compatibility (a pre-resolver binary may write them during a
downgrade window; the incremental refresh detects and purges them), but every
public lookup surface rejects `service` as an entity type with
`unsupported graph entity_type` — it is deliberately absent from the valid
lookup list above.

Case sensitivity: canonical keys are lowercase, but the log fan-out
predicates compare `logs.hostname` / `logs.app_name` with SQLite's default
BINARY collation, so matching is case-sensitive. A mixed-case syslog
hostname (`Nashost`) never matches the canonical instance key
(`nashost/plex`) — syslog senders should emit lowercase hostnames. Hostname
case normalization at ingest is tracked separately.

`user` is a human/identity principal (operator name, authenticated username),
keyed `{hostname}:{username}`. `device` is a client endpoint (DNS client IP,
MAC) distinct from a server `host`, keyed by the client identifier. Both are
projected from identity-bearing log rows (AdGuard DNS clients, Authelia
usernames, shell-history users).

`git_commit` is a commit event observed in an agent-command or shell-history
row whose command is a `git commit`/`git push`. It is keyed by
`{ai_project}:{timestamp}` (agent-command rows) or `{hostname}:{timestamp}`
(shell-history rows); the SHA is not assumed.

Unknown entity types MUST be rejected on public lookup surfaces.

### 4.2 Relationship Types

These relationship type strings are valid:

```text
observed_as
runs_on
emitted_by
worked_on
matches_signature
defines_service
routes_to
exposes_domain
attached_to
mounts
backed_by
has_artifact
authenticated_as
accessed
communicates_with
instance_of
```

`authenticated_as` links a `user` to a service/host they authenticated to
(Authelia). `accessed` links a `user` or `device` to a domain/service/host it
reached (AdGuard DNS, shell-history). `communicates_with` (device↔peer, UniFi
flow data) is vocabulary-reserved for future flow ingestion. `instance_of`
links a `service_instance` to its `logical_service`
(`service_instance:nashost/plex instance_of logical_service:plex`).

### 4.3 Trust Levels

These trust level strings are valid:

```text
verified
claimed
inferred
correlated
refuted
```

Meanings:

| Trust | Meaning |
| --- | --- |
| `verified` | Derived from a field Cortex treats as an identity source for this relationship. |
| `claimed` | Derived from a sender-claimed field such as syslog hostname. |
| `inferred` | Derived from metadata or summaries that imply a relationship but are not identity proof. |
| `correlated` | A derivation *method* (temporal co-occurrence), not an epistemic status. Reserved for future query-time correlation; effective confidence is capped at 0.5. |
| `refuted` | The relationship was believed true but has been explicitly disproved or retracted (manual override only). Refuted edges are excluded from every traversal/query result and contribute zero effective confidence; they must not be resurrected by rebuild. |

Trust levels MUST NOT be upgraded without stronger source evidence.

### 4.4 Source Kinds

These source kind strings are valid:

```text
log
heartbeat
ai_session_rollup
source_inventory
app_inventory
error_signature
```

The current v1 projection emits evidence rows for:

```text
log
error_signature
source_inventory
app_inventory
```

Heartbeat currently contributes host entities and aliases, not relationship
evidence rows.

### 4.5 Reason Codes

These reason code strings are valid:

```text
syslog_claimed_hostname
log_app_name
docker_container_id
docker_service_label
ai_session_project
heartbeat_host_state
error_signature_match
inventory_node
inventory_service
compose_config
reverse_proxy_config
docker_network
storage_probe
config_artifact
agent_command_session
agent_command_cwd_infer
agent_command_git_commit
shell_history_git_commit
adguard_client_query
shell_history_user
authelia_auth
resolver_instance_of
resolver_service_instance
resolver_raw_app_label
```

`resolver_instance_of` records the resolver-derived
`service_instance instance_of logical_service` edge.
`resolver_service_instance` records resolver-projected service-instance
identity from structured evidence (agent Docker metadata, verified
inventory). `resolver_raw_app_label` records a raw observed log app label
associated with a host; raw labels never self-upgrade into
`logical_service` identity.

`resolver_service_instance` and `resolver_raw_app_label` are
**vocabulary-reserved**: they are registered in the schema and reason
registry but no projection path emits them today — only
`resolver_instance_of` is written.

`adguard_client_query` links a client `device` to the queried `domain` (AdGuard
DNS). `shell_history_user` links the operating `user` to the host (shell
history). `authelia_auth` links a `user` to the host they authenticated to
(Authelia).

`agent_command_git_commit` links an AI session and its project to a `git_commit`
entity when an agent-command row runs a `git commit`/`git push` (inferred — the
commit happened, but the exact SHA is not asserted). `shell_history_git_commit`
links a host to a `git_commit` entity from a shell-history row.

`agent_command_session` links an AI session to a host it provably ran commands
on (verified, `session_id` is a hard FK on the agent-command spool record).
`agent_command_cwd_infer` links an AI session to the project inferred from the
agent command's working directory (inferred). Both are emitted by
`extract_agent_command_row` for `agent-command://` log rows; the session entity
is keyed by the inferred project so it converges with transcript-derived
sessions for the same `session_id`.

**Reason code vocabulary (v1 → v2).** The flat strings above are the **stable v1
vocabulary** — they are the values stored in the DB. A **v2 hierarchical
vocabulary** (OTel-attribute style, e.g. `source:syslog:claimed_hostname`,
`source:docker:container_id`, `derivation:ai:session_project`) is exposed as a
read-only registry via `graph::reason_code_namespace()` / `reason_code_family()`,
enabling prefix queries (`source:docker:*`) and family-level weighting without
changing stored values. Migrating the **stored** values to v2 is future work and
will be a full graph rebuild; until then v1 strings remain authoritative.

`heartbeat_host_state` is reserved for heartbeat-derived relationship evidence.
The current v1 implementation uses heartbeat rows for host identity/aliases.
Inventory projections use `source_inventory` and `app_inventory` evidence to
link hosts, services, Compose projects, reverse proxy routes, domains, Docker
networks, storage mounts, and redacted config artifacts.

## 5. Entity Construction Contract

Entity keys are normalized before storage. Normalization trims whitespace,
rejects empty values, and canonicalizes keys for matching. Display labels retain
the source-facing value where practical.

Entities are unique by:

```text
(entity_type, canonical_key)
```

Repeated observations update first/last seen timestamps and keep the existing
entity id stable within a rebuild.

### 5.1 Resolver Observation Model

Service identity entities (`logical_service`, `service_instance`) are
constructed from bounded, typed resolver observations
(`src/db/entity_resolution/observation.rs`), never from ad-hoc topology
string building. Observations are chunk-local or aggregated in memory:
there is no per-log resolver-observation table. Each observation carries a
kind, canonical keys, a safe display label (`safe_display_value` redacts
credentialed URLs, home paths, and token/secret material and bounds output
to 128 printable characters), source kind/id, an evidence path, and a
resolver trust level (`verified` > `claimed` > `inferred`).

Adapter rules (`src/db/entity_resolution/adapters.rs`):

- Structured agent Docker identity (`metadata_json.agent_docker`) yields
  verified host, logical-service, and service-instance observations. The
  compose service label (falling back to container name) is the logical
  identity; the agent host scopes the instance.
- Verified/observed inventory services yield verified host,
  logical-service, service-instance, domain, and storage (mount)
  observations.
- Raw log app labels yield only a `RawAppLabel` observation. Raw labels
  never self-upgrade into `logical_service` identity.

## 6. Relationship Construction Contract

Relationships are unique by:

```text
src_entity_id:relationship_type:dst_entity_id
```

Repeated observations of the same relationship update:

- first seen timestamp,
- last seen timestamp,
- max confidence,
- evidence count,
- representative evidence rows.

Relationship direction is meaningful and MUST be preserved.

## 7. Current Edge Rules

### 7.1 Source Observed As Claimed Host

Input:

- `logs.source_ip`
- `logs.hostname`

Output:

```text
source_ip observed_as host
```

Fields:

| Field | Value |
| --- | --- |
| `relationship_type` | `observed_as` |
| `reason_code` | `syslog_claimed_hostname` |
| `trust_level` | `claimed` |
| `confidence` | `0.6` |
| `evidence.source_kind` | `log` |
| `evidence.source_log_id` | source `logs.id` |
| `evidence.reason_text` | `syslog header hostname claimed by sender` |

### 7.2 App Emitted By Host

Input:

- `logs.app_name`
- `logs.hostname`

Output:

```text
app emitted_by host
```

Fields:

| Field | Value |
| --- | --- |
| `relationship_type` | `emitted_by` |
| `reason_code` | `log_app_name` |
| `trust_level` | `inferred` |
| `confidence` | `0.5` |
| `evidence.metadata_path` | `logs.app_name` |
| `evidence.source_log_id` | source `logs.id` |

### 7.3 AI Session Worked On Project

Input:

- `logs.ai_project`
- `logs.ai_tool`
- `logs.ai_session_id`

Output:

```text
ai_session worked_on ai_project
```

Fields:

| Field | Value |
| --- | --- |
| `relationship_type` | `worked_on` |
| `reason_code` | `ai_session_project` |
| `trust_level` | `verified` |
| `confidence` | `0.9` |
| `evidence.metadata_path` | `logs.ai_project/logs.ai_session_id` |
| `evidence.source_log_id` | source `logs.id` |

### 7.4 Container Runs On Host

Input:

- `logs.source_ip` with `docker://` or `docker-event://`
- `metadata_json.container_id` or `metadata_json.container_name`
- Docker host from metadata or source string

Output:

```text
container runs_on host
```

Fields:

| Field | Value |
| --- | --- |
| `relationship_type` | `runs_on` |
| `reason_code` | `docker_container_id` |
| `trust_level` | `verified` |
| `confidence` | `0.9` |
| `evidence.metadata_path` | `logs.source_ip/metadata_json` |
| `evidence.source_log_id` | source `logs.id` |

### 7.5 Resolver Service Identity (Agent Docker Metadata)

Hard break (entity_resolution_v2): the previous
`container runs_on service` rule (`docker_service_label`, legacy
`host:project:service` keys) is REMOVED. Central-pull `docker://` /
`docker-event://` rows keep only the verified host/container edges from
7.4 and are not resolver proof.

Input:

- `metadata_json.agent_docker.host` (required),
- `metadata_json.agent_docker.container_id` (required),
- `metadata_json.agent_docker.container_name` (required),
- `metadata_json.agent_docker.stream` (required),
- `metadata_json.agent_docker.compose_project` / `compose_service` /
  `image` (optional).

The compose service label (falling back to the container name) is the
logical identity; the agent host scopes the instance.

Outputs:

```text
logical_service entity            (canonical key: plex)
service_instance entity           (canonical key: nashost/plex)
service_instance instance_of logical_service
```

Fields:

| Field | Value |
| --- | --- |
| `relationship_type` | `instance_of` |
| `reason_code` | `resolver_instance_of` |
| `trust_level` | `verified` |
| `confidence` | `1.0` |
| `evidence.metadata_path` | `metadata_json.agent_docker` |
| `evidence.source_log_id` | source `logs.id` |

Nested slash-triplet app labels (`plex/plex/plex`) are never projected as
`app` entities; raw app labels never self-upgrade into `logical_service`
identity.

### 7.6 Error Signature Links

Input:

- `error_signatures.signature_hash`
- `error_signatures.normalizer_version`
- `error_signatures.template`
- `error_signatures.sample_app_name`
- `error_signatures.sample_hostname`

Outputs:

```text
app matches_signature error_signature
host matches_signature error_signature
```

Fields:

| Field | App Link | Host Link |
| --- | --- | --- |
| `relationship_type` | `matches_signature` | `matches_signature` |
| `reason_code` | `error_signature_match` | `error_signature_match` |
| `trust_level` | `inferred` | `inferred` |
| `confidence` | `0.7` | `0.5` |
| `evidence.source_kind` | `error_signature` | `error_signature` |
| `evidence.source_signature_hash` | signature hash | signature hash |
| `evidence.metadata_path` | `error_signatures` | `error_signatures` |

### 7.7 Inventory Topology Links

Input:

- normalized `HomelabInventory.nodes`,
- normalized `HomelabInventory.services`,
- normalized `HomelabInventory.compose_projects`,
- normalized `HomelabInventory.reverse_proxies`,
- normalized `HomelabInventory.networks`,
- normalized `HomelabInventory.storage`,
- normalized redacted `HomelabInventory.artifact_refs`.

Outputs:

```text
logical_service entity (per unique service name)
service_instance instance_of logical_service
service_instance runs_on host
compose_project defines_service service_instance
compose_project has_artifact config_artifact
reverse_proxy exposes_domain domain
reverse_proxy routes_to service_instance
service_instance attached_to network
service_instance mounts storage
host backed_by storage
```

Fields:

| Relationship | Reason Code | Trust | Confidence | Evidence Kind |
| --- | --- | --- | --- | --- |
| `service_instance instance_of logical_service` | `resolver_instance_of` | inventory trust | `0.95` | `app_inventory` |
| `service_instance runs_on host` | `inventory_service` | `inferred` | `0.85` | `app_inventory` |
| `compose_project defines_service service_instance` | `compose_config` | `verified` | `0.90` | `app_inventory` |
| `compose_project has_artifact config_artifact` | `config_artifact` | `verified` | `0.95` | `app_inventory` |
| `reverse_proxy exposes_domain domain` | `reverse_proxy_config` | `verified` | `0.95` | `app_inventory` |
| `reverse_proxy routes_to service_instance` | `reverse_proxy_config` | `verified` | `0.85` | `app_inventory` |
| `service_instance attached_to network` | `docker_network` | `verified` | `0.80` | `app_inventory` |
| `service_instance mounts storage` | `storage_probe` | `inferred` | `0.65` | `app_inventory` |
| `host backed_by storage` | `storage_probe` | `verified` | `0.75` | `source_inventory` |

The inventory projection paths (`compose_config`, `reverse_proxy_config`,
`docker_network`) are **active**, projected from the homelab inventory snapshot
(`app_inventory`). Service-instance keying is `host/service` (`nashost/plex`);
legacy `{host}:{service}` keys are never emitted. A service without host
context projects the `logical_service` only.

Bare service-name matches are valid only when the service name is unique across
the inventory snapshot. Ambiguous service names MUST be matched through a
host-scoped `service_instance` key or left unlinked.

## 8. Evidence Contract

Every persisted relationship MUST have evidence.

Evidence fields:

| Field | Type | Rule |
| --- | --- | --- |
| `id` | integer | Stable row id within current projection. |
| `relationship_id` | integer | References `graph_relationships.id`. |
| `evidence_key` | string | Dedupe key for repeated observations. |
| `source_kind` | string | One of the valid source kinds. |
| `source_id` | string | Source-local id string. |
| `source_log_id` | integer/null | Set for evidence derived from `logs`. |
| `source_heartbeat_id` | integer/null | Reserved for heartbeat relationship evidence. |
| `source_signature_hash` | string/null | Set for error signature evidence. |
| `observed_at` | string | RFC3339-ish timestamp from source observation. |
| `reason_code` | string | One of the valid reason codes. |
| `reason_text` | string/null | Short human explanation. |
| `confidence_delta` | number | Rule contribution. |
| `trust_level` | string | Trust level for this evidence. |
| `safe_excerpt` | string/null | Bounded redacted source excerpt. |
| `metadata_path` | string/null | Source field path used for the rule. |
| `evidence_count` | integer | Number of observations represented by this evidence row. |

Public graph responses MUST expose safe evidence only. Raw frames,
unbounded metadata JSON, secrets, and terminal control characters MUST NOT be
emitted through graph evidence fields.

Relationship response fields:

| Field | Type | Rule |
| --- | --- | --- |
| `src_entity_id` | integer | Preserved compatibility id for the source endpoint. |
| `dst_entity_id` | integer | Preserved compatibility id for the destination endpoint. |
| `src_entity` | object/null | Optional compact endpoint summary when available. |
| `dst_entity` | object/null | Optional compact endpoint summary when available. |

Endpoint summaries are additive and compact. They include `id`, `entity_type`,
`canonical_key`, `display_label`, and `trust_level`; they do not embed full
entity source ids or aliases.

Evidence lookup response fields:

| Field | Type | Rule |
| --- | --- | --- |
| `evidence` | object | Safe evidence row from `graph_relationship_evidence`. |
| `relationship` | object | Owning relationship, including compatibility ids and endpoint summaries. |
| `src_entity` | object | Source endpoint summary. |
| `dst_entity` | object | Destination endpoint summary. |
| `source_log_summary` | object/null | Bounded scalar log summary for log-derived evidence when the source row still exists. |
| `missing_source_reason` | string/null | `evidence_source_is_not_a_log` or `source_log_missing_or_retained_out` when no summary is returned. |
| `metadata` | object | Projection status, caps, and truncation metadata. |

`source_log_summary` MUST be structurally incapable of containing raw frames or
full `metadata_json`. It contains only `id`, `timestamp`, `received_at`,
`hostname`, `severity`, `app_name`, `process_id`, `source_ip`, `message`, and
`message_truncated`. Message and text-like fields MUST be bounded, redacted for
auth/header/credential/private-key/home-path/URL-userinfo markers, and stripped
of terminal control characters.

## 9. Public Query Contract

### 9.1 Entity Lookup

```bash
cortex entity <entity-type> <key> [--limit N] [--json]
cortex entity <entity-type:key> [--json]
cortex entity --alias-type TYPE --alias-key KEY [--limit N] [--json]
```

Entity lookup MUST:

- reject unknown entity types with `unsupported graph entity_type`,
  including the retired `service` type (schema-tolerated for migration
  compatibility but not a valid lookup type — see §4.1),
- reject legacy nested service identity keys (`nashost:plex`,
  `nashost:plex:plex`, `plex/plex/plex`) on service-identity lookups with
  `rejected_legacy_shape` before any graph query runs (alias lookups query
  first and reject a legacy-shaped alias key only when the alias does not
  resolve — a resolving alias, e.g. a colon-bearing `ai_session` key, is
  returned normally),
- return one exact entity when unambiguous,
- return candidates for ambiguous alias matches,
- include projection metadata,
- respect bounded limits,
- redact identifiers (canonical keys, display labels, source ids,
  relationship keys, alias keys) with the same secret/home-path redaction as
  evidence excerpts.

### 9.2 Neighborhood Lookup

```bash
cortex graph around <entity-type> <key> [--limit N] [--json]
cortex graph around <entity-type:key> [--limit N] [--json]
```

Neighborhood lookup MUST:

- be bounded,
- include relationships and safe evidence samples,
- include source/destination endpoint summaries on relationships when available,
- include projection metadata,
- include truncation metadata when caps are hit,
- avoid implicit rebuilds.

V1 neighborhood lookup is one-hop.

### 9.3 Explanation

```bash
cortex graph explain <entity-type> <key> [--depth N] [--beam-width N] [--max-chains N] [--json]
cortex graph explain <entity-type:key> [--json]
```

Explanation MUST:

- be deterministic,
- cite relationship/evidence ids,
- include readable endpoint summaries on returned relationships when available,
- use conservative language,
- report missing evidence,
- avoid claiming root cause from correlation alone,
- include projection and truncation metadata.

### 9.4 Evidence Lookup

```bash
cortex graph evidence <evidence-id> [--payload-budget BYTES] [--json]
```

MCP uses `action=graph mode=evidence evidence_id=<id>`. REST uses
`GET /api/graph/evidence?evidence_id=<id>`.

Evidence lookup MUST:

- anchor on `graph_relationship_evidence.id`,
- be read-only and never trigger rebuild,
- return the safe evidence row, owning relationship, endpoint summaries, and
  projection metadata,
- return bounded `source_log_summary` for existing log-derived evidence,
- return `source_log_summary: null` with `missing_source_reason` when the source
  is non-log evidence or the referenced log row is missing,
- preserve source identifiers such as `source_log_id`, `source_heartbeat_id`,
  and `source_signature_hash`,
- reject unknown REST query fields,
- avoid exposing raw frames, full `metadata_json`, secrets, or terminal controls.

## 10. Compatibility Requirements

Changes MUST preserve:

- existing entity type strings,
- existing relationship type strings,
- existing reason code strings,
- existing trust level meanings,
- JSON field names for public responses,
- explicit rebuild semantics,
- no implicit rebuild from lookup,
- safe evidence output rules.

Breaking changes require:

1. migration note,
2. changelog entry,
3. update to this contract,
4. tests covering old/new behavior or explicit rejection,
5. operator-facing migration guidance when persisted data shape changes.
