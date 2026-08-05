---
title: "Heartbeat Telemetry Contract (V1 Pre-Implementation)"
created: 2026-05-24
updated: 2026-07-30
---

# Heartbeat Telemetry Contract (V1 Pre-Implementation)

## 1. Purpose And Status

This document is the contract for the first-class host heartbeat telemetry
surface. It is derived from
[`docs/superpowers/specs/2026-05-24-heartbeat-telemetry-design.md`](../superpowers/specs/2026-05-24-heartbeat-telemetry-design.md).

Status: **pre-implementation contract**.

The implementation plan MUST treat this file as the source of truth. Any code
PR that changes endpoint names, payload fields, database tables, MCP actions,
auth behavior, retention behavior, or error codes MUST update this contract in
the same PR.

Heartbeat telemetry is not synthetic syslog text. It is sampled host state with
its own ingest path, storage schema, query actions, and retention policy.

## 2. Stability

The v1 heartbeat surface uses these tiers:

| Surface | Tier | Compatibility Rule |
| --- | --- | --- |
| `POST /v1/heartbeats` | experimental until the first working release, then stable | Path, method, auth semantics, and accepted payload fields are additive-only after stabilization. |
| MCP actions in this file | experimental until shipped | `host_state` is the first v1 query surface. `fleet_state` and `correlate_state` are follow-up surfaces unless the implementation plan explicitly pulls them into the first slice. Parameters and result fields are additive-only after stabilization. |
| SQLite tables in this file | migration contract | New nullable columns and new tables are additive. Renaming or deleting fields requires a migration note and changelog entry. |
| Agent probe internals | internal | Can change when the emitted heartbeat payload remains compatible. |

## 3. Transport

Heartbeat agents push snapshots to the existing `cortex` HTTP listener.

### 3.1 Endpoint

```http
POST /v1/heartbeats
Content-Type: application/json
Authorization: Bearer <CORTEX_TOKEN>
```

The route is mounted beside OTLP on the same server process. It MUST use a
route-local body limit for heartbeat ingest. It MUST NOT inherit the MCP
router's smaller body limit or OTLP's larger body limit by accident.

### 3.2 Auth

The route follows the OTLP trust model:

- When `CORTEX_TOKEN` is configured, requests MUST include a valid bearer
  token.
- When no token is configured, loopback-only development MAY be unauthenticated.
- Non-loopback unauthenticated exposure MUST be rejected at startup unless the
  operator explicitly enables no-auth mode.
- OAuth JWT support is not required for v1 heartbeat ingest.

The route MUST NOT accept tokens in query parameters.

Bearer parsing and comparison SHOULD reuse the existing OTLP-style helpers,
including constant-time token comparison and redacted auth diagnostics. Missing,
malformed, wrong, query-parameter, and valid bearer-token cases MUST have tests.

### 3.3 Body Limit

The maximum request body size is **256 KiB**.

This is an ingest-side HTTP request limit for one heartbeat snapshot sent from
an agent to `cortex`. It does not permit returning full heartbeat snapshots
through MCP actions or other LLM-facing query surfaces. Query surfaces MUST
return bounded summaries, bounded sample arrays, and explicit truncation signals
where applicable.

Oversize requests return:

```json
{
  "error": "payload_too_large"
}
```

with HTTP `413`.

### 3.4 Response

Successful ingest returns HTTP `202 Accepted`:

```json
{
  "ok": true,
  "accepted": 1,
  "heartbeat_id": 12345,
  "received_at": "2026-05-24T14:30:00.000Z"
}
```

The server accepts exactly one heartbeat snapshot per request in v1.

Duplicate `(host_id, boot_id, sequence)` retries are idempotent. A duplicate
request that matches an already accepted heartbeat returns HTTP `202 Accepted`
with `accepted: 0` and the existing `heartbeat_id`; it MUST NOT insert metric
child rows a second time.

## 4. Agent Operation

The v1 collector is an always-on agent, invoked as:

```bash
cortex heartbeat agent
```

Default agent behavior:

| Setting | Default |
| --- | --- |
| Sample interval | 30 seconds |
| Soft collection budget | 2 seconds |
| Hard collection deadline | 5 seconds |
| Retry buffer | bounded memory buffer |
| Host identity | generated stable host id stored in syslog config |
| Primary target OS | Linux |

The agent MAY emit partial snapshots. It MUST prefer sending partial data over
skipping a heartbeat entirely when cheap probes completed.

The agent MUST bound:

- probe execution time,
- number of per-interface rows,
- number of per-disk rows,
- number of top processes,
- number of container details,
- retry buffer size.

## 5. Heartbeat Request Schema

The request body is a JSON object.

Required top-level fields:

```json
{
  "schema_version": 1,
  "host": {},
  "sample": {},
  "agent": {},
  "cpu": {},
  "memory": {},
  "disks": [],
  "networks": [],
  "processes": {},
  "containers": {}
}
```

Unknown top-level fields are rejected.

### 5.1 Host

```json
{
  "host_id": "01jyn5j7n3h9wc0x0mx2x2fb9c",
  "hostname": "tootie",
  "os": "linux",
  "kernel": "6.8.0-60-generic",
  "architecture": "x86_64",
  "boot_id": "9f7f0b8e-f9b7-44bd-92c4-9ab66f43d80a",
  "timezone": "America/New_York"
}
```

Required:

- `host_id`
- `hostname`
- `os`
- `architecture`
- `boot_id`

Rules:

- `host_id` is the primary identity for heartbeat rows.
- `host_id` is generated by the agent, stable across restarts, and stored in
  syslog configuration.
- `hostname` is display/filter metadata and is self-reported.
- Raw `/etc/machine-id` MUST NOT be sent. Plain unsalted machine-id hashes MUST
  NOT be sent by default because they are durable cross-service identifiers. If
  a future mode derives any field from machine id, it must be explicitly opt-in
  and use a keyed/HMAC-style derivation documented in this contract.

### 5.2 Sample

```json
{
  "sequence": 42,
  "sampled_at": "2026-05-24T14:29:59.250Z",
  "uptime_secs": 86400,
  "monotonic_ms": 86400000,
  "collection_ms": 37,
  "partial": false,
  "probe_errors": [],
  "skipped_probes": []
}
```

Required:

- `sequence`
- `sampled_at`
- `uptime_secs`
- `collection_ms`
- `partial`

Rules:

- `sequence` increases monotonically per host agent process.
- A lower `sequence` with a new `boot_id` indicates reboot or agent restart.
- `probe_errors` entries are bounded strings and MUST NOT contain secrets.

### 5.3 Agent

```json
{
  "version": "0.31.0",
  "mode": "always_on",
  "interval_secs": 30,
  "push_latency_ms": 12,
  "retry_backlog": 0
}
```

Required:

- `version`
- `mode`
- `interval_secs`

`mode` is currently fixed to `always_on`.

### 5.4 CPU

```json
{
  "load1": 0.42,
  "load5": 0.51,
  "load15": 0.49,
  "usage_pct": 11.4,
  "user_pct": 6.1,
  "system_pct": 3.2,
  "iowait_pct": 1.7,
  "steal_pct": 0.0,
  "core_count": 8
}
```

Required:

- `load1`
- `load5`
- `load15`
- `core_count`

Percentage fields are nullable on the first sample because they require a
previous counter snapshot.

### 5.5 Memory

```json
{
  "mem_total_bytes": 33554432000,
  "mem_available_bytes": 21474836480,
  "mem_used_bytes": 12079595520,
  "swap_total_bytes": 8589934592,
  "swap_used_bytes": 0
}
```

Required:

- `mem_total_bytes`
- `mem_available_bytes`
- `swap_total_bytes`
- `swap_used_bytes`

### 5.6 Disks

Each disk entry is either a mount capacity record or a block-device IO record.

Capacity entry:

```json
{
  "kind": "mount",
  "name": "/mnt/cache",
  "fs_type": "xfs",
  "bytes_total": 1000204886016,
  "bytes_free": 500102443008,
  "bytes_used": 500102443008,
  "inodes_total": 488378368,
  "inodes_free": 488000000,
  "readonly": false
}
```

IO entry:

```json
{
  "kind": "block",
  "name": "nvme0n1",
  "read_bytes_per_sec": 1048576,
  "write_bytes_per_sec": 524288,
  "read_ops_per_sec": 20.5,
  "write_ops_per_sec": 8.0,
  "busy_pct": 3.4
}
```

Rules:

- Pseudo filesystems are excluded by default.
- Device and mount arrays MUST be bounded.
- Per-second rates are nullable on the first sample.

### 5.7 Networks

```json
{
  "interface": "eth0",
  "rx_bytes_per_sec": 12000,
  "tx_bytes_per_sec": 8200,
  "rx_packets_per_sec": 90.5,
  "tx_packets_per_sec": 70.2,
  "rx_errors_per_sec": 0,
  "tx_errors_per_sec": 0,
  "rx_drops_per_sec": 0,
  "tx_drops_per_sec": 0,
  "link_state": "up",
  "speed_mbps": 1000
}
```

Rules:

- Loopback is excluded by default.
- Interface arrays MUST be bounded.
- Per-second rates are nullable on the first sample.

### 5.8 Processes

```json
{
  "total": 312,
  "running": 2,
  "sleeping": 302,
  "zombies": 0,
  "top": [
    {
      "pid": 1234,
      "name": "postgres",
      "cpu_pct": 7.5,
      "rss_bytes": 2147483648,
      "state": "S"
    }
  ]
}
```

Required:

- `total`
- `zombies`

Rules:

- `top` is optional and collected on a slower schedule.
- Full command lines, environment variables, usernames, open files, and network
  connections are excluded by default.

### 5.9 Containers

```json
{
  "runtime": "docker",
  "reachable": true,
  "running": 42,
  "exited": 3,
  "restarting": 0,
  "unhealthy": 1,
  "details": [
    {
      "id": "6d2f...",
      "name": "freshrss",
      "state": "running",
      "health": "unhealthy"
    }
  ]
}
```

Required:

- `reachable`
- `running`
- `exited`
- `restarting`
- `unhealthy`

Rules:

- `details` is optional and bounded.
- Container environment variables are never collected.

### 5.10 Optional GPU

```json
{
  "present": true,
  "devices": [
    {
      "name": "NVIDIA GeForce RTX 3060",
      "utilization_pct": 6,
      "memory_used_bytes": 1073741824,
      "memory_total_bytes": 12884901888,
      "temperature_c": 48,
      "power_watts": 52.5
    }
  ]
}
```

GPU is optional. If the probe command is unavailable or slow, the agent omits
the `gpu` field and records the skipped probe.

## 6. Storage Contract

The implementation creates dedicated heartbeat tables. Table and column names
below are normative for v1 unless this contract is updated before code lands.

Implementation MUST base the next migration on the current schema version in
code, not on stale generated schema snapshots. If
`docs/contracts/current-schema.sql` is stale when heartbeat storage lands, the
same PR must update, regenerate, or retire that snapshot.

### 6.1 `host_heartbeats`

```sql
CREATE TABLE IF NOT EXISTS host_heartbeats (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id         TEXT NOT NULL,
    hostname        TEXT NOT NULL,
    source_ip       TEXT NOT NULL DEFAULT '',
    sampled_at      TEXT NOT NULL,
    received_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    boot_id         TEXT NOT NULL,
    uptime_secs     INTEGER NOT NULL,
    sequence        INTEGER NOT NULL,
    collection_ms   INTEGER NOT NULL,
    push_latency_ms INTEGER,
    partial         INTEGER NOT NULL DEFAULT 0,
    agent_version   TEXT NOT NULL,
    os              TEXT NOT NULL,
    kernel          TEXT,
    architecture    TEXT NOT NULL,
    metadata_json   TEXT,
    UNIQUE(host_id, boot_id, sequence)
);
```

Required indexes:

```sql
CREATE INDEX IF NOT EXISTS idx_host_heartbeats_host_sampled
    ON host_heartbeats(host_id, sampled_at);

CREATE INDEX IF NOT EXISTS idx_host_heartbeats_received
    ON host_heartbeats(received_at);

CREATE INDEX IF NOT EXISTS idx_host_heartbeats_hostname_sampled
    ON host_heartbeats(hostname, sampled_at);
```

### 6.2 Metric Tables

Metric tables reference `host_heartbeats(id)` by `heartbeat_id`.

Foreign key enforcement is not required in v1 because the existing SQLite
contract does not globally enable `PRAGMA foreign_keys`.

Required tables:

- `heartbeat_cpu`
- `heartbeat_memory`
- `heartbeat_disks`
- `heartbeat_network`
- `heartbeat_processes`
- `heartbeat_containers`

Optional table:

- `heartbeat_gpu`

Each table MUST include `heartbeat_id INTEGER NOT NULL` and enough typed fields
to satisfy the request/result schemas in this contract. Extra probe-specific
metadata belongs in bounded `metadata_json`.

Each metric table MUST have an index on `heartbeat_id`. Tables queried by
time-window summaries or latest-state views MUST include the additional indexes
needed by those query plans before the corresponding MCP action ships.

### 6.3 Retention

Heartbeat retention is independent from log retention.

Default high-resolution retention: **14 days**.

Retention deletes heartbeat rows by `received_at`, oldest first. The delete
path MUST be chunked so it does not hold SQLite's write lock long enough to
starve log ingest.

Because global SQLite foreign-key enforcement is not required in v1, retention
and storage-recovery code MUST explicitly delete metric child rows before or
with their parent `host_heartbeats` rows.

The existing storage guardrail measures whole-database pressure. Heartbeat
tables therefore MUST participate in storage recovery; the server MUST NOT
enter a state where heartbeat data can breach the DB budget while log-only
deletion is the only recovery path.

## 7. Derived Server Signals

The server computes these normalized flags:

- `cpu_pressure`
- `memory_pressure`
- `swap_pressure`
- `disk_capacity_pressure`
- `disk_io_pressure`
- `network_error_pressure`
- `container_unhealthy`
- `collector_partial`
- `heartbeat_late`
- `host_rebooted`
- `clock_skew`

Agent-supplied local flags MAY be stored, but server-computed flags are the
source of truth for fleet views and correlation.

Default late heartbeat threshold: no accepted heartbeat for **2.5x the agent's
declared interval**.

## 8. MCP Actions

Heartbeat MCP actions dispatch through the existing `cortex` tool using the
`action` argument.

`host_state` is the first v1 MCP action. `fleet_state` and `correlate_state`
are specified as follow-up surfaces and may be deferred by the implementation
plan. All MCP action responses MUST be bounded and MUST NOT expose the raw full
heartbeat ingest payload by default.

All actions return the existing MCP envelope style:

```json
{
  "ok": true,
  "result": {}
}
```

On error:

```json
{
  "ok": false,
  "error": {
    "code": "invalid_params",
    "message": "..."
  }
}
```

### 8.1 `host_state`

Purpose: return latest heartbeat state or a bounded summary for one host.

Params:

```json
{
  "action": "host_state",
  "host": "tootie",
  "since": "2026-05-24T14:00:00Z",
  "limit": 20
}
```

Rules:

- `host` first matches `host_id`.
- If no `host_id` matches, `hostname` may be used only when it resolves to
  exactly one `host_id`.
- If a hostname resolves to multiple host ids, return `ambiguous_host`.
- When `since` is omitted, return the latest heartbeat.
- `limit` defaults to 1 and is capped at 100.

Result:

```json
{
  "host_id": "01jyn5j7n3h9wc0x0mx2x2fb9c",
  "hostname": "tootie",
  "latest": {},
  "samples": [],
  "derived": {
    "heartbeat_late": false,
    "host_rebooted": false,
    "cpu_pressure": false,
    "memory_pressure": false,
    "disk_capacity_pressure": false
  }
}
```

### 8.2 `fleet_state`

Purpose: return latest heartbeat status for all known heartbeat hosts.

Params:

```json
{
  "action": "fleet_state",
  "include_ok": true,
  "sort": "pressure"
}
```

Rules:

- `include_ok` defaults to true.
- `sort` values: `pressure`, `freshness`, `hostname`.

Result:

```json
{
  "hosts": [
    {
      "host_id": "01jyn5j7n3h9wc0x0mx2x2fb9c",
      "hostname": "tootie",
      "last_heartbeat_at": "2026-05-24T14:30:00Z",
      "status": "ok",
      "pressure": ["container_unhealthy"],
      "partial": false
    }
  ],
  "summary": {
    "total": 1,
    "ok": 1,
    "late": 0,
    "partial": 0,
    "pressure": 0
  }
}
```

### 8.3 `correlate_state`

Purpose: return logs plus heartbeat summaries around a time window.

Params:

```json
{
  "action": "correlate_state",
  "reference_time": "2026-05-24T14:30:00Z",
  "window_minutes": 10,
  "host": "tootie",
  "severity_min": "warning",
  "limit": 100
}
```

Rules:

- `reference_time` is required.
- `window_minutes` defaults to 10 and is capped at 120.
- `host` is optional; omitted means correlate across all hosts.
- `limit` caps returned log rows and defaults to 100.

Result:

```json
{
  "window": {
    "from": "2026-05-24T14:20:00Z",
    "to": "2026-05-24T14:40:00Z"
  },
  "hosts": [
    {
      "host_id": "01jyn5j7n3h9wc0x0mx2x2fb9c",
      "hostname": "tootie",
      "heartbeat_summary": {
        "samples": 20,
        "partial_samples": 0,
        "max_cpu_pct": 55.2,
        "min_mem_available_bytes": 2147483648,
        "pressure": []
      },
      "logs": []
    }
  ],
  "truncated": false
}
```

## 9. CLI/API Parity

`host_state` needs MCP parity first. CLI parity, REST parity, and parity for
`fleet_state`/`correlate_state` may be deferred only when the implementation
plan explicitly records the deferral and the reason.

Expected CLI shape:

```bash
cortex state host --host tootie --json
cortex state fleet --json
cortex correlate state --reference-time 2026-05-24T14:30:00Z --window-minutes 10 --json
```

Expected REST shape:

- `GET /api/host-state`
- `GET /api/fleet-state`
- `GET /api/correlate-state`

REST endpoints, when implemented, MUST follow the current API auth and mounting
conventions in code at the time of implementation. Do not key new heartbeat
planning to retired or stale API enablement flags.

## 10. Error Codes

HTTP ingest errors:

| HTTP | Error | Meaning |
| --- | --- | --- |
| 400 | `invalid_payload` | JSON is malformed, unknown fields are present, or required fields are missing. |
| 401 | `unauthorized` | Missing or invalid bearer token. |
| 413 | `payload_too_large` | Body exceeds 256 KiB. |
| 503 | `storage_unavailable` | DB write path unavailable or backpressured. |

MCP/action errors:

| Code | Meaning |
| --- | --- |
| `invalid_params` | Bad host, timestamp, window, limit, or enum value. |
| `ambiguous_host` | A hostname matched multiple heartbeat host ids; retry with `host_id`. |
| `not_found` | Requested host has no heartbeat data. |
| `heartbeat_unavailable` | Heartbeat tables are not initialized or feature is disabled. |
| `internal` | Unexpected server-side failure. |

## 11. Privacy Contract

V1 heartbeat collection MUST NOT collect these by default:

- environment variables,
- process command lines,
- usernames,
- open files,
- network connection tuples,
- container environment variables,
- raw machine id,
- secrets or auth material.

If any future mode collects those fields, it must be explicitly opt-in and
called out in this contract.

## 12. Future Extension Points

Reserved but not implemented in v1:

- mesh reachability fields,
- relay/spool mode,
- disk-backed heartbeat retry queue,
- OTLP metrics export/import mapping,
- vector embeddings over heartbeat/log incidents,
- non-Linux collectors.

These may be added without changing the v1 endpoint if the existing payload
remains accepted.
