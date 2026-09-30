# Heartbeat V1 Implementation Plan

Tracker epic: `syslog-mcp-h2kq`

Primary contract: `docs/contracts/heartbeat-telemetry.md`

Primary design: `docs/superpowers/specs/2026-05-24-heartbeat-telemetry-design.md`

## Scope

V1 implements the minimum useful host-state data plane:

1. Structured SQLite storage for heartbeat snapshots and metric child tables.
2. `POST /v1/heartbeats` on the existing HTTP listener.
3. `syslog` MCP action `host_state`.
4. `syslog heartbeat agent` as a Linux collector.
5. Binary-owned setup for a separate heartbeat-agent user service.

V1 does not implement the older WebSocket agent mode or probe-registry control plane. Those designs remain inputs for later phases after the heartbeat data plane runs in production.

## Schema

The implementation increments the source migration from `KNOWN_SCHEMA_VERSION = 14` to `15`.

Migration 15 adds:

- `host_heartbeats`
- `heartbeat_cpu`
- `heartbeat_memory`
- `heartbeat_disks`
- `heartbeat_network`
- `heartbeat_processes`
- `heartbeat_containers`
- `heartbeat_gpu`

`host_heartbeats` owns the unique retry key:

```sql
UNIQUE(host_id, boot_id, sequence)
```

The schema treats `host_id` as authoritative. `hostname` remains self-reported display/filter metadata and cannot identify a host unless it maps to exactly one `host_id`.

Retention and storage recovery explicitly delete heartbeat child rows because the repo does not rely on global SQLite foreign-key enforcement for cleanup safety.

## HTTP Ingest

`POST /v1/heartbeats` is mounted as a sibling route on the HTTP server, separate from MCP and OTLP routing.

Route requirements:

- Bearer token uses the existing `SYSLOG_MCP_TOKEN` posture.
- Query-parameter tokens are rejected.
- Request body limit is route-local `256 KiB`.
- Unknown fields and malformed JSON return `invalid_payload`.
- Source IP is captured from the HTTP peer connection.
- Duplicate `(host_id, boot_id, sequence)` retries return `202 accepted:0` and do not insert duplicate child metric rows.

This route-local `256 KiB` cap limits heartbeat ingest payloads. It does not allow sending a 256 KiB payload to the LLM; MCP query actions return bounded summaries and samples.

## MCP Query

`host_state` is the first shipped query surface.

Rules:

- `host_id` lookup is authoritative.
- `hostname` fallback is allowed only when exactly one `host_id` matches.
- Ambiguous hostname lookup returns `ambiguous_host`.
- `limit` is capped at 100.
- Responses are bounded summaries/samples, not raw full heartbeat ingest payloads.

## Agent

`syslog heartbeat agent` is the V1 collector.

Defaults:

- Target: `http://127.0.0.1:3100`
- Interval: 30 seconds
- Probe deadline: 2 seconds
- Collection deadline: 5 seconds
- Retry buffer: bounded in-memory queue

Identity:

- A generated stable `host_id` is stored in syslog appdata.
- The collector does not send raw machine-id or plain machine-id hashes.

Linux probes:

- CPU load/core count from procfs.
- Memory and swap from procfs.
- Mount capacity from `/proc/self/mounts` plus `statvfs`.
- Disk IO counters from `/proc/diskstats` with nullable first-sample rates.
- Network counters/errors from `/proc/net/dev` with nullable first-sample rates.
- Process counts and bounded pid/state summaries from `/proc`.
- Docker container state summary from `docker ps` when reachable.

Heavy probes remain out of V1: GPU, SMART, zfs, btrfs, mdraid, full process command lines, environment variables, open connection tuples, and disk-backed heartbeat spooling.

## Setup

`syslog setup heartbeat-agent install|remove|check` owns service setup.

The service is separate from the central syslog server and runs:

```text
syslog heartbeat agent --host-id-path <appdata>/heartbeat-host-id
```

`scripts/plugin-setup.sh` stays a thin adapter to `syslog setup plugin-hook`; heartbeat-agent systemd/service logic belongs in the binary.

## Deferred Surfaces

These remain explicitly deferred after the V1 slice:

- `fleet_state`
- `correlate_state`
- REST parity beyond heartbeat ingest
- CLI parity beyond the collector/setup commands
- WebSocket `/ws/agent`
- per-host enrollment and token rotation
- durable log streaming and replay
- server-driven probe requests
- typed probe registry scheduling
- disk-backed heartbeat spool
- non-Linux collectors

Reasons:

- `fleet_state` needs latest-state indexes or cache support before exposing all-host summaries safely.
- `correlate_state` needs a bounded query plan that joins logs and heartbeat samples without broad scans.
- WebSocket/control-plane work should build on observed production heartbeat behavior, not compete with the first data-plane slice.
- Probe registry work should reuse the collector/probe code after the cheap Linux probes settle.

## Verification

Focused checks used during implementation:

```bash
cargo test heartbeat
cargo test heartbeat_agent
cargo test host_state
cargo test schema_actions_are_dispatchable
cargo test public_action_references_cover_schema_registry
cargo clippy -- -D warnings
```
