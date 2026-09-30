# Lightweight Host Heartbeat Telemetry Design

## Purpose

`syslog-mcp` already correlates logs across hosts, containers, AI sessions, and infrastructure services. The heartbeat feature adds sampled host state so troubleshooting can answer a broader question: what was the machine doing when the logs happened?

The v1 design is intentionally direct. Each host runs an always-on local collector that samples cheap host state every 30 seconds and pushes compact snapshots to the canonical `syslog-mcp` HTTP listener. `syslog-mcp` stores heartbeat data as first-class telemetry and exposes it through CLI, API, and MCP actions for latest state, fleet state, and log correlation windows.

## Goals

- Gather useful host state with a soft collection target of 2 seconds and hard cap of 5 seconds per sample.
- Keep default overhead low enough for 30-second collection on small homelab hosts.
- Make missing, late, partial, or failing heartbeats visible as operational signals.
- Store heartbeat data in structured tables, not as synthetic log text.
- Correlate heartbeat state with existing logs by host and time window.
- Leave room for future mesh-aware reachability and vector/RAG enrichment without building those in v1.

## Non-Goals

- No peer-to-peer mesh, relay election, distributed storage, or gossip protocol in v1.
- No PostgreSQL or pgvector migration as part of heartbeat v1.
- No Windows/macOS collector implementation in v1.
- No heavyweight SMART, zpool, package inventory, or full process inventory every 30 seconds.
- No dependency on OpenTelemetry metrics semantics for the first version.

## Approach

Use a native heartbeat API and schema.

The collector runs as `cortex heartbeat agent`. It is a long-lived process that keeps prior counters in memory, allowing cheap rate calculation for CPU, disk IO, network IO, and container deltas. It sends each snapshot to a new authenticated endpoint on the existing HTTP listener, likely `POST /v1/heartbeats`.

The server validates and stores snapshots in dedicated heartbeat tables. Common dimensions use typed columns for query speed. Host-specific or probe-specific details use bounded JSON fields.

This approach keeps heartbeat separate from logs while making correlation explicit at query time.

## Collector

The v1 collector targets Linux first.

Cheap probes run every sample:

- Host identity: hostname, source host id, OS, kernel, architecture, boot id, uptime.
- Timing: sampled timestamp, monotonic uptime, collection duration, sequence number.
- CPU: load average, total CPU usage delta, user/system/iowait/steal deltas when available.
- Memory: total, available, used, swap total, swap used.
- Disk capacity: selected mounts, bytes total/free/used, inode total/free/used, readonly flag.
- Disk IO: selected block devices, read/write bytes, ops, busy time deltas.
- Network IO: selected interfaces, rx/tx bytes, packets, drops, errors, link state when cheap.
- Processes: total process count and zombie count.
- Containers: container runtime reachable flag, counts by running/exited/restarting/unhealthy where cheap.
- Agent health: probe errors, skipped probes, partial flag, push latency, retry backlog length.

Slower probes run on internal schedules:

- Every 2-5 minutes: top processes by CPU/RSS, failed systemd units count, unhealthy container list, GPU summary if the command is available and consistently fast.
- Every 15-60 minutes: storage health summaries such as SMART, btrfs, zfs, mdraid, and reboot-required flags.

Each probe has its own timeout. Failed optional probes produce a partial snapshot rather than failing the whole heartbeat.

## Agent Operation

The agent is always-on, not a one-shot timer.

Default behavior:

- Sample interval: 30 seconds.
- Soft collection budget: 2 seconds.
- Hard sample deadline: 5 seconds.
- Push endpoint: configured server URL, defaulting to the local configured `syslog-mcp` HTTP endpoint when appropriate.
- Authentication: bearer token using existing `SYSLOG_MCP_TOKEN` conventions.
- Buffering: bounded in-memory retry buffer first; optional small disk spool can be added if field testing shows it is needed.
- Backoff: push failures retry with jitter and bounded backlog, never unbounded memory growth.

The agent sends raw measurements plus cheap local flags. The server remains authoritative for normalized pressure/anomaly interpretation because thresholds should be centrally configurable.

## Server Ingest

Add a new HTTP route on the existing listener:

- `POST /v1/heartbeats`: accepts heartbeat snapshots.

The route should use the same auth policy style as MCP/OTLP:

- Bearer auth when `SYSLOG_MCP_TOKEN` is configured.
- Explicit startup warning or refusal if exposed in an unsafe mode, matching the existing OTLP trust-boundary posture.
- Request body cap significantly smaller than OTLP logs; heartbeat snapshots should be compact.

Ingest behavior:

- Reject invalid payloads with structured errors.
- Accept partial snapshots and store probe error metadata.
- Deduplicate by host id plus sequence number or host id plus sample timestamp where possible.
- Update host heartbeat registry fields such as `last_heartbeat_at`, `last_sequence`, `missed_count`, and `last_status`.
- Record server `received_at` separately from agent sample time to support clock skew and late heartbeat detection.

## Storage

Use dedicated SQLite tables for v1.

Proposed core tables:

- `host_heartbeats`: one row per accepted sample. Columns include id, host id, hostname, source ip, sampled_at, received_at, boot id, uptime seconds, sequence number, collection ms, push latency ms, partial flag, status flags, agent version, and metadata JSON.
- `heartbeat_cpu`: one row per sample with load averages and CPU time/rate fields.
- `heartbeat_memory`: one row per sample with memory and swap fields.
- `heartbeat_disks`: one row per sample per mount or device, depending on metric kind.
- `heartbeat_network`: one row per sample per interface.
- `heartbeat_processes`: one row per sample for aggregate process counts plus optional top-process JSON from slower probes.
- `heartbeat_containers`: one row per sample for container summary plus optional unhealthy/restarting container JSON.
- `heartbeat_gpu`: optional table for hosts with GPU probes.

Indexes should support:

- Latest heartbeat per host.
- Heartbeats for a host in a time window.
- Fleet pressure lookup over recent samples.
- Correlation joins by hostname/source identity and time range.

Retention should be independent from log retention. A sensible default is shorter high-resolution retention, for example 7-30 days, with later rollups if needed.

## Query Surfaces

Add first-class MCP actions, with CLI/API parity:

- `host_state`: latest heartbeat or windowed summary for one host.
- `fleet_state`: latest state for all hosts, sorted by pressure or freshness.
- `correlate_state`: logs plus heartbeat summaries around a reference time/window.
- `heartbeat_timeline`: bucketed heartbeat pressure/state over time.
- `heartbeat_status`: collector health, late hosts, missing hosts, partial snapshot counts, ingest counters.

V1 should prioritize:

1. `host_state`
2. `fleet_state`
3. `correlate_state`

`correlate_state` should accept the same kind of time-window parameters as existing correlation actions and return:

- Matching logs grouped by host.
- Heartbeat samples or summaries in the same window.
- Derived pressure flags for CPU, memory, disk, IO, network, containers, and collector health.
- Clock skew and late/missing heartbeat notes where relevant.

## Derived Signals

The server computes normalized flags from raw measurements:

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

Local agent flags can be stored, but server-computed flags are the source of truth for fleet views and correlation.

## Configuration

New configuration areas:

- Heartbeat server ingest: enabled flag, auth policy, body cap, retention days.
- Agent: enabled flag, server URL, token, sample interval, hard timeout, buffer size, host id override.
- Probe config: enabled probe list, mount allow/deny lists, interface allow/deny lists, container runtime settings, slower-probe intervals.

Defaults should be conservative:

- Include real filesystems, exclude pseudo filesystems.
- Include physical and primary virtual interfaces, exclude loopback by default.
- Do not collect full command lines unless explicitly enabled.
- Bound all JSON arrays such as top processes and unhealthy containers.

## Error Handling

Collector:

- Individual probe failure marks the snapshot partial.
- Hard sample deadline stops optional probes and sends whatever is available.
- Push failure increments retry counters and buffers bounded snapshots.
- Repeated push failure should log locally at a bounded rate.

Server:

- Invalid payload returns 400.
- Unauthorized returns 401.
- Body too large returns 413 with retry guidance.
- DB write unavailable returns 503.
- Late/missing heartbeats are not ingest errors; they are fleet state.

## Security and Privacy

Heartbeat payloads can expose sensitive operational details.

V1 should:

- Require bearer auth for non-loopback deployments.
- Avoid environment variables, process command lines, usernames, secrets, open file names, and network connection lists by default.
- Bound top-process output to process name, pid, CPU, RSS, and state unless expanded collection is explicitly enabled.
- Treat hostname from payload as self-reported. Use source IP and configured host identity for trust decisions where possible.
- Redact tokens and endpoint URLs in logs.

## Testing

Unit tests:

- Probe parsers for `/proc`, `/sys`, cgroup, mount, network, and disk samples using fixtures.
- Delta calculations for counters, resets, reboot, and wraparound.
- Payload validation and bounded JSON behavior.
- Derived pressure flag thresholds.
- SQLite migrations and query functions.

Integration tests:

- HTTP heartbeat ingest route with auth success/failure.
- Latest host state query.
- Fleet state query with late/missing/partial hosts.
- Correlate state combining seeded logs and seeded heartbeats.
- Collector loop with fake probes and fake transport to verify deadlines, partial snapshots, and retry buffer behavior.

Manual smoke:

- Run a local collector against a local server.
- Verify `host_state`, `fleet_state`, and `correlate_state`.
- Kill the collector and verify missing/late status.
- Restart the host or simulate boot id change and verify reboot detection.

## Rollout Plan

The implementation should be split into slices:

1. Schema and server ingest route with tests.
2. App/DB query functions and MCP/CLI/API surfaces for latest state.
3. Linux collector core with fake-probe tests.
4. Real cheap probes for CPU, memory, disk capacity, network, disk IO, process count, and container counts.
5. Correlation action that joins logs and heartbeat windows.
6. Setup integration for installing/running the always-on agent.
7. Slower optional probes and retention tuning.

## V1 Defaults

V1 uses these defaults unless implementation evidence forces a change:

- Host identity defaults to a generated stable host id stored in syslog config. Hostname remains a display/filter field, not the primary identity.
- Retry buffering is memory-only and bounded. Disk spool is deferred until real push-failure data proves it is needed.
- Setup installs the heartbeat collector as a separate systemd service from the central server service.
- High-resolution heartbeat retention defaults to 14 days.
