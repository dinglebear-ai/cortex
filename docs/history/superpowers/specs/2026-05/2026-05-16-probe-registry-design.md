# Probe Registry + Live-state MCP Actions — Design Spec

**Epic**: `syslog-mcp-fue9`
**Depends on**: `syslog-mcp-qgnx` (Agent Mode: WebSocket + JSON-RPC 2.0)
**Date**: 2026-05-16
**Status**: Draft

---

## 1. Goal & Non-Goals

### Goal

Define a **hardcoded-in-Rust probe registry** that runs inside the per-host agent and a set of **MCP actions** that surface live host state through `syslog-mcp`. The system must let an AI agent answer concrete operational questions that log streams alone cannot answer:

- "Who's eating devhost's RAM right now?" (`mem.top`, `mem.pressure`)
- "Which paths on edgehost are blackholes?" (`disk.blackholes` — `~/.cargo/target`, `node_modules`, Docker overlay, `.venv`, `__pycache__`)
- "Is `/var/log` filling again?" (`disk.usage`)
- "Why is DNS flaky on the WSL boxes?" (`net.dns_check`)
- "What MAC just collided on the LAN?" (`net.neigh`)
- "What systemd units are dead on the Unraid stack?" (`systemd.failed`)
- "Which container is in a crashloop?" (`docker.health`)

### Non-Goals

- **No user-extensible probes.** Probe set is closed and shipped in the agent binary. This eliminates the shell-injection surface entirely.
- **No remediation actions.** Probes are read-only. No `systemctl restart`, no `rm -rf`, no `docker kill`. Acting is the operator's job.
- **Not a metrics platform.** `metrics_gauge` is a coarse time-series for *operational triage*, not Prometheus. Retention is short, cardinality is bounded.
- **No probe scripting language.** Probes are Rust functions; parameters are typed structs.
- **No alerting in V1.** Anomaly detection on probe results is a follow-up epic.

---

## 2. Probe Trait / Interface

All probes implement a single async trait. The agent binary owns the implementations; the server only sees the wire types.

```rust
/// One concrete probe implementation. Stateless — all state lives in args.
#[async_trait::async_trait]
pub trait Probe: Send + Sync + 'static {
    /// Stable name: e.g. "disk.blackholes". Used in registry lookup, RPC, DB.
    fn name(&self) -> &'static str;

    /// Capability advertisement returned during agent handshake.
    fn descriptor(&self) -> ProbeDescriptor;

    /// Execute the probe. MUST respect ctx.deadline. MUST be cancel-safe.
    async fn run(&self, ctx: ProbeCtx, args: ProbeArgs) -> Result<ProbeOutput, ProbeError>;
}

pub struct ProbeDescriptor {
    pub name: &'static str,
    pub default_cadence: Option<Duration>,  // None = on-demand only
    pub on_demand: bool,
    pub default_timeout: Duration,
    pub os_support: OsSupport,              // bitflags: LINUX | WSL | DARWIN
    pub args_schema: serde_json::Value,     // JSON Schema for params
}

pub struct ProbeCtx {
    pub deadline: Instant,                  // hard kill at this point
    pub host_id: HostId,
    pub request_id: Option<Uuid>,           // None for scheduled runs
    pub cancel: CancellationToken,          // tokio_util
}

/// Wire format. Probes deserialize their typed args struct from this.
pub type ProbeArgs = serde_json::Value;

/// Tagged union — one variant per probe family.
#[derive(Serialize, Deserialize)]
#[serde(tag = "probe", content = "payload")]
pub enum ProbeOutput {
    DiskUsage(DiskUsagePayload),
    DiskBlackholes(DiskBlackholesPayload),
    MemTop(MemTopPayload),
    MemPressure(MemPressurePayload),
    NetNeigh(NetNeighPayload),
    NetDnsCheck(NetDnsCheckPayload),
    SystemdFailed(SystemdFailedPayload),
    DockerHealth(DockerHealthPayload),
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("probe timeout after {0:?}")] Timeout(Duration),
    #[error("not supported on this OS")] Unsupported,
    #[error("missing capability: {0}")] MissingCapability(String),  // e.g. "docker.sock"
    #[error("invalid args: {0}")] InvalidArgs(String),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("internal: {0}")] Internal(String),
}
```

**Async vs sync**: All probes are `async`. CPU-bound work (`du`-style walks) is dispatched via `tokio::task::spawn_blocking` so the agent's I/O loop stays responsive. The trait remains async so probe selection, timeout enforcement, and result reporting can interleave on the WS connection.

---

## 3. Probe Registry

A `phf`-style perfect-hash static map is built at compile time. No `inventory` runtime registration — the set is closed and we want grep-able provenance.

```rust
pub struct Registry { entries: &'static [&'static dyn Probe] }

pub fn registry() -> &'static Registry { /* OnceLock */ }

impl Registry {
    pub fn get(&self, name: &str) -> Option<&'static dyn Probe>;
    pub fn descriptors(&self) -> Vec<ProbeDescriptor>;
}
```

### Capability handshake

On WS connect (epic A), the agent sends a `hello` JSON-RPC notification containing `{ agent_version, os, kernel, capabilities: [...], probes: [Descriptor, ...] }`. The server persists this in a new `agent_capabilities` table (defined in epic A; this spec only consumes it). MCP actions consult capabilities before dispatching: if `docker.health` isn't advertised, `service_health` action returns `{status: "unavailable", reason: "docker not present on host"}`.

---

## 4. Scheduling Model

The **server** owns the schedule. Each connected agent receives a `schedule.set` JSON-RPC call after handshake:

```json
{
  "jsonrpc": "2.0",
  "method": "schedule.set",
  "params": {
    "entries": [
      { "probe": "disk.usage",     "interval_secs": 60,   "args": {} },
      { "probe": "mem.pressure",   "interval_secs": 30,   "args": {} },
      { "probe": "mem.top",        "interval_secs": 300,  "args": {"n": 20} },
      { "probe": "disk.blackholes","interval_secs": 21600,"args": {"paths": "$default"} },
      { "probe": "net.neigh",      "interval_secs": 600,  "args": {} },
      { "probe": "systemd.failed", "interval_secs": 300,  "args": {} },
      { "probe": "docker.health",  "interval_secs": 60,   "args": {} }
    ]
  }
}
```

- **Interval-based, not cron.** Avoids timezone footguns; jitter is added per-entry (`±10%`) to prevent fleet-wide thundering herds.
- **Local fallback**: agent ships a baked-in default schedule used when the WS is down (so a freshly-restarted agent still does *something*). Once it reconnects and receives `schedule.set`, the server's schedule wins.
- **Hot reload**: `schedule.set` is idempotent. Server can resend at any time; agent diffs and re-arms tokio timers.
- **Drift handling**: each entry tracks `last_run_at`. If `now - last_run_at > interval * 3` (e.g. the agent was suspended), we run **once immediately** and resume the cadence — we do NOT replay missed runs.
- **Concurrency**: probes are dispatched on a `tokio::task::JoinSet` with a per-agent semaphore (`MAX_CONCURRENT_PROBES = 4`). If a long-running probe (e.g. `disk.blackholes`) is still going when its next tick fires, the new tick is **skipped** and a warning is logged.

---

## 5. On-Demand Pull Flow

```text
MCP client ──action──▶ syslog-mcp server
                            │
                            │ 1. cache check (probe_results, freshness window)
                            │ 2. if stale or force_refresh:
                            │      RPC "probe.run" → agent
                            │ 3. await response with deadline
                            │ 4. persist into probe_results
                            │ 5. return latest row to caller
                            ▼
                    MCP client
```

**JSON-RPC request** (server → agent):

```json
{
  "jsonrpc": "2.0",
  "id": "req-7f2e",
  "method": "probe.run",
  "params": {
    "probe": "mem.top",
    "args": { "n": 25 },
    "deadline_ms": 5000
  }
}
```

**Cancellation**: server-side timeout (`deadline_ms + 500ms` grace) drops the request future and emits a `probe.cancel` notification. Agent honors `ProbeCtx::cancel` to stop walking dirs / draining FDs.

**Result caching window**: per-probe, configurable via server config:

| probe              | default freshness | rationale                                  |
|--------------------|-------------------|--------------------------------------------|
| `disk.usage`       | 30s               | mountpoints rarely change                  |
| `disk.blackholes`  | 1h                | du walks are expensive; reuse aggressively |
| `mem.top`          | 5s                | RAM moves fast during pressure             |
| `mem.pressure`     | 5s                | PSI is the leading indicator               |
| `net.neigh`        | 60s               | ARP table is slow-moving                   |
| `net.dns_check`    | 30s               | balance latency cost vs. recency           |
| `systemd.failed`   | 30s               | failures persist until masked              |
| `docker.health`    | 10s               | container churn is fast                    |

MCP action params accept `max_age_secs` to override and `force_refresh=true` to bypass.

---

## 6. `metrics_gauge` Table

Numeric time-series. One row per scalar sample. Designed for cheap rolling queries and aggressive eviction.

```sql
CREATE TABLE metrics_gauge (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id     TEXT NOT NULL,
    metric_name TEXT NOT NULL,        -- e.g. "mem.used_pct"
    labels      TEXT NOT NULL DEFAULT '{}', -- JSON, sorted keys for dedup
    value       REAL NOT NULL,
    ts          INTEGER NOT NULL      -- unix epoch millis
);

CREATE INDEX idx_metrics_host_metric_ts ON metrics_gauge(host_id, metric_name, ts DESC);
CREATE INDEX idx_metrics_metric_ts       ON metrics_gauge(metric_name, ts DESC);
CREATE INDEX idx_metrics_ts              ON metrics_gauge(ts);
```

**Retention**:
- Raw: **14 days**.
- Downsample to 5-minute means (`metrics_gauge_5m`) at 14d boundary; keep 90d.
- Downsample to 1-hour means (`metrics_gauge_1h`) at 90d boundary; keep 365d.
- Implemented by the existing storage-guardrails task in `src/db/maintenance.rs` (extend, don't fork).

**Cardinality guard**: probes that emit gauges (`mem.pressure`, `disk.usage`) must emit a bounded label set. Reject inserts where `(host_id, metric_name, labels)` cardinality crosses a threshold (e.g. > 200 distinct combos per host). Mountpoint and `psi_scope=some|full` are the only label keys allowed in V1.

**Sample emission** (from `mem.pressure`):

```text
host_id=devhost metric=mem.pressure_avg10 labels={"scope":"some"} value=14.2 ts=...
host_id=devhost metric=mem.pressure_avg10 labels={"scope":"full"} value=2.1  ts=...
```

---

## 7. `probe_results` Table

Structured JSON payloads. One row per probe execution.

```sql
CREATE TABLE probe_results (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id       TEXT NOT NULL,
    probe_name    TEXT NOT NULL,
    requested_by  TEXT,              -- "schedule" | "mcp:<client>" | "rpc"
    request_ts    INTEGER NOT NULL,  -- unix ms
    response_ts   INTEGER NOT NULL,  -- unix ms, == request_ts on error
    duration_ms   INTEGER NOT NULL,
    payload       TEXT NOT NULL,     -- JSON ProbeOutput; "{}" on error
    status        TEXT NOT NULL,     -- "ok" | "timeout" | "unsupported" | "error"
    error         TEXT               -- nullable, populated when status != "ok"
);

CREATE INDEX idx_probe_results_host_probe_ts
    ON probe_results(host_id, probe_name, response_ts DESC);
CREATE INDEX idx_probe_results_status ON probe_results(status, response_ts DESC);
```

**Retention (eviction)**:
- Keep the **last 200 rows per `(host_id, probe_name)`**, regardless of age.
- Plus: drop everything older than 30 days as a hard ceiling.
- Eviction runs hourly under `db::maintenance`, in a single `DELETE ... WHERE id IN (SELECT id FROM probe_results WHERE ... ORDER BY response_ts DESC LIMIT -1 OFFSET 200)` per group. Cheap; uses the composite index.

**Query patterns** (used by MCP actions):
- "latest result for probe X on host Y": index seek + LIMIT 1.
- "all hosts' latest disk.usage": correlated subquery on `(host_id, probe_name)` MAX(`response_ts`).

---

## 8. V1 Probe Definitions

### 8.1 `disk.usage`

**Purpose**: per-mountpoint capacity and inode usage. Direct replacement for `df -h` semantics.

```rust
pub struct DiskUsageArgs {}  // no params; reports all real mounts

pub struct DiskUsagePayload {
    pub mounts: Vec<MountSnapshot>,
}
pub struct MountSnapshot {
    pub mountpoint: String,
    pub fs_type: String,
    pub device: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub avail_bytes: u64,
    pub used_pct: f32,
    pub inodes_total: Option<u64>,
    pub inodes_used: Option<u64>,
}
```

**Implementation**: parse `/proc/mounts`, filter out pseudo-fs (`tmpfs` kept for `/run`, `/dev/shm`; drop `proc`, `sysfs`, `cgroup*`, `overlay` unless asked), call `statvfs(2)` via `nix::sys::statvfs::statvfs` on each.

**Also emits gauges**: `disk.used_pct{mount=...}`, `disk.avail_bytes{mount=...}`.

**Cadence**: 60s. **Timeout**: 2s. **OS**: Linux + WSL.

### 8.2 `disk.blackholes`

**Purpose**: bounded sizing of high-churn build-cache paths. Directly addresses the "1TB NVMe full of `target/`" pain.

```rust
pub struct DiskBlackholesArgs {
    /// Either ["$default"] or absolute paths. Tilde resolved against agent user $HOME.
    pub paths: Vec<String>,
    /// Walker time budget per path. Default 5s.
    pub time_budget_ms: u32,
    /// Max files visited per path. Default 500_000.
    pub max_entries: u64,
}

pub struct DiskBlackholesPayload {
    pub entries: Vec<BlackholeEntry>,
    pub truncated_paths: Vec<String>,   // paths that hit budget
}
pub struct BlackholeEntry {
    pub path: String,
    pub total_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub newest_mtime: Option<i64>,    // unix epoch
    pub oldest_mtime: Option<i64>,
    pub completed: bool,              // false if budget hit
}
```

**Default path set** (resolved per-user; agent scans all `/home/*` + `/root`):

```text
~/.cargo/target          (Rust)
~/.cargo/registry        (Rust)
~/.rustup
~/.npm
~/.pnpm-store
~/.cache
~/.venv                  (literal)
**/node_modules          (HOME-rooted glob, depth-limited to 6)
**/.venv                 (HOME-rooted glob, depth-limited to 6)
**/__pycache__           (HOME-rooted glob, depth-limited to 6)
/var/lib/docker/overlay2 (read summary; needs root or docker group)
```

**Implementation**: `walkdir::WalkDir` with `follow_links(false)`, `same_file_system(true)`. Two budgets enforced in tight loop:

```rust
for entry in walker.into_iter().filter_map(|e| e.ok()) {
    if start.elapsed() > budget { truncated = true; break; }
    if visited > max_entries   { truncated = true; break; }
    if ctx.cancel.is_cancelled() { return Err(Cancelled); }
    // accumulate total_bytes / file_count / mtimes
}
```

Glob expansion uses `globwalk` with `max_depth(6)`. Each globbed match is treated as a separate `BlackholeEntry`.

**Cadence**: 6h. **Timeout**: 60s (whole probe), 5s/path. **OS**: Linux + WSL (slow on Windows DrvFs — surface warning).

**Edge cases**:
- Permission denied → skip + count, don't fail probe.
- Bind mounts / loop devices → `same_file_system` prevents wandering.
- Snap/flatpak dirs with absurd inode counts → time budget catches it.
- Docker overlay: query Docker API for layer sizes instead of walking; falls back to `statvfs` on `/var/lib/docker` if Docker socket absent.

### 8.3 `mem.top`

**Purpose**: top-N processes by RSS. Directly answers "who's OOMing devhost".

```rust
pub struct MemTopArgs { pub n: u32 }  // default 20, cap 100

pub struct MemTopPayload {
    pub total_mem_bytes: u64,
    pub avail_mem_bytes: u64,
    pub procs: Vec<ProcSnapshot>,
}
pub struct ProcSnapshot {
    pub pid: i32,
    pub ppid: i32,
    pub comm: String,           // /proc/<pid>/comm
    pub cmdline: String,        // first 256 chars of /proc/<pid>/cmdline
    pub uid: u32,
    pub rss_bytes: u64,
    pub vsize_bytes: u64,
    pub cgroup: Option<String>, // first cgroup v2 path
}
```

**Implementation**: iterate `/proc`, read `status`, `cmdline`, `cgroup`. Top-K via `BinaryHeap` with N. Use `procfs` crate (well-maintained, no shellout).

**Cadence**: 300s scheduled (low value at high frequency); on-demand for triage. **Timeout**: 3s. **OS**: Linux + WSL.

### 8.4 `mem.pressure`

**Purpose**: PSI memory pressure — *leading* indicator of OOM, far better than free-memory %.

```rust
pub struct MemPressureArgs {}
pub struct MemPressurePayload {
    pub some: PsiBucket,
    pub full: PsiBucket,
}
pub struct PsiBucket { pub avg10: f32, pub avg60: f32, pub avg300: f32, pub total_us: u64 }
```

**Implementation**: read `/proc/pressure/memory` (one open + read; ~200 bytes). Parse two lines:

```text
some avg10=0.00 avg60=0.00 avg300=0.00 total=...
full avg10=0.00 avg60=0.00 avg300=0.00 total=...
```

Emit gauges (see §6).

**Cadence**: 30s. **Timeout**: 1s. **OS**: Linux ≥4.20 + WSL2 (kernel must expose `/proc/pressure/`; some distros need `psi=1` boot param — surface `Unsupported` if missing).

### 8.5 `net.neigh`

**Purpose**: ARP/neighbor table — catches MAC/IP collisions and stale entries.

```rust
pub struct NetNeighArgs {}
pub struct NetNeighPayload { pub entries: Vec<NeighEntry> }
pub struct NeighEntry {
    pub ip: IpAddr,
    pub mac: Option<String>,
    pub dev: String,
    pub state: String,    // REACHABLE | STALE | DELAY | FAILED | PERMANENT | NOARP
    pub age_secs: Option<u32>,
}
```

**Implementation**: prefer `rtnetlink` crate (`NEIGH_GET` dump) — gives both v4 and v6, no fork. Fallback: parse `/proc/net/arp` (v4 only). Avoid shelling to `ip neigh` because parsing locale-dependent output is brittle.

**Cadence**: 600s. **Timeout**: 2s. **OS**: Linux + WSL2 (WSL's virtualized switch makes ARP scope `eth0`-only; document this).

### 8.6 `net.dns_check`

**Purpose**: per-resolver latency + failure tracking. Targets the "DNS flaky on WSL" pain.

```rust
pub struct NetDnsCheckArgs {
    pub hostnames: Vec<String>,     // e.g. ["google.com", "github.com", "nashost.lan"]
    pub resolvers: Vec<IpAddr>,     // empty = use /etc/resolv.conf
    pub query_type: String,         // "A" | "AAAA"; default "A"
}
pub struct NetDnsCheckPayload {
    pub results: Vec<DnsResult>,
}
pub struct DnsResult {
    pub hostname: String,
    pub resolver: IpAddr,
    pub status: String,             // "ok" | "timeout" | "nxdomain" | "servfail" | "error"
    pub latency_ms: Option<u32>,
    pub answers: Vec<IpAddr>,
    pub error: Option<String>,
}
```

**Implementation**: `hickory-resolver` (formerly `trust-dns-resolver`). One `AsyncResolver` per resolver IP, configured with `ResolverOpts { timeout: 1s, attempts: 1, cache_size: 0 }` (caching defeats the latency measurement). Run all (hostname × resolver) queries concurrently via `futures::future::join_all`. Hard 3s deadline on the whole probe.

**Cadence**: configurable; default off (host config opts in). When enabled: 60s. **Timeout**: 3s. **OS**: all.

**WSL note**: WSL2's `nameserver` in `/etc/resolv.conf` points at the host's NAT gateway, which can drop packets under load. We surface this clearly in the payload by always including the resolver IP — operator can see "172.x.x.1 is the slow one".

### 8.7 `systemd.failed`

**Purpose**: enumerate `failed` and `degraded` units.

```rust
pub struct SystemdFailedArgs {}
pub struct SystemdFailedPayload {
    pub system_state: String,       // "running" | "degraded" | "maintenance" | ...
    pub units: Vec<FailedUnit>,
}
pub struct FailedUnit {
    pub name: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
    pub n_restarts: Option<u32>,
}
```

**Implementation**: `zbus` against `org.freedesktop.systemd1` on the system bus. Call `ListUnitsByPatterns(states=["failed"], patterns=[])`. Read `SystemState` property from manager. No shellout to `systemctl`.

**Cadence**: 300s. **Timeout**: 2s. **OS**: Linux only (return `Unsupported` on WSL when systemd isn't enabled; many WSL hosts now run systemd, capability advertisement handles it).

### 8.8 `docker.health`

**Purpose**: per-container state, health, restart count, last exit. Catches crashloops fast.

```rust
pub struct DockerHealthArgs { pub include_stopped: bool }  // default false
pub struct DockerHealthPayload {
    pub containers: Vec<ContainerSnapshot>,
    pub daemon_version: Option<String>,
}
pub struct ContainerSnapshot {
    pub id: String,                 // short id
    pub name: String,
    pub image: String,
    pub state: String,              // running | exited | restarting | paused | dead
    pub health: Option<String>,     // healthy | unhealthy | starting | none
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub started_at: Option<String>, // RFC3339
    pub uptime_secs: Option<u64>,
}
```

**Implementation**: `bollard::Docker::connect_with_local_defaults()` then `list_containers(Some(ListContainersOptions { all: include_stopped, .. }))` followed by `inspect_container` for each (to pull `RestartCount` and `State.Health.Status`). Inspect calls run concurrently with a 16-wide buffered stream.

**Cadence**: 60s. **Timeout**: 5s. **OS**: any host with `/var/run/docker.sock`. Returns `MissingCapability("docker.sock")` otherwise.

---

## 9. MCP Actions

All actions dispatch through `src/mcp/tools.rs`, joining the existing `cortex` super-tool. Each accepts standard params `host` (required for some), `max_age_secs`, `force_refresh`, plus action-specific args.

### 9.1 `disk_usage`

Params: `host` (required), `mount` (optional filter), `max_age_secs`, `force_refresh`.
Response: latest `DiskUsagePayload` for that host + `fetched_at` + `from_cache: bool`.
Stale behavior: if `now - fetched_at > max_age_secs` (default 30s), pull fresh.
Agent offline: return last cached row with `from_cache: true, agent_online: false, last_seen_at: ...`.

### 9.2 `disk_blackholes`

Params: `host`, `top_n` (default 25, sorts by `total_bytes`), `paths_filter`, `max_age_secs` (default 3600), `force_refresh`.
Response: ranked `BlackholeEntry` list plus a `total_reclaim_estimate_bytes`.
Note: `force_refresh=true` on this action is *expensive* (kicks a 60s walk). The action returns a `job_id` and 202-style payload if the walk is already running; subsequent calls return the in-flight result once done.

### 9.3 `mem_top`

Params: `host`, `n` (default 20), `max_age_secs` (default 5), `force_refresh`.
Response: `MemTopPayload` + a computed `interpretation` field summarizing top-3 (e.g. `"node:212312 RSS=14.2GB, claude:9871 RSS=8.4GB"`) for fast model digestion.

### 9.4 `service_health`

Params: `host`, `kind` (`"systemd" | "docker" | "all"`, default `"all"`).
Response: combines `SystemdFailedPayload` and `DockerHealthPayload`. Each section reports `available: bool` based on capability handshake.

### 9.5 `dns_status`

Params: `host`, `hostnames` (optional override), `resolvers` (optional), `max_age_secs` (default 30).
Response: `NetDnsCheckPayload` + a `summary { failures, p50_ms, p95_ms, slow_resolvers: [...] }`.

### 9.6 `network_neigh`

Params: `host`, `state_filter` (e.g. `["FAILED", "STALE"]`), `subnet_filter` (CIDR), `max_age_secs` (default 60).
Response: filtered `NetNeighPayload` + a `collisions` array — entries where the same MAC appears on multiple IPs or vice versa (detected server-side from the result, since we have it).

### 9.7 `agent_status`

Params: `host` (optional; omit for fleet view).
Response: per-host `{ online, agent_version, kernel, capabilities, schedule, last_probe_at: {probe: ts}, last_error }`.
Always live (no caching) — queries WS connection state directly.

### Common semantics

- **Freshness**: every response carries `{fetched_at: <iso8601>, from_cache: bool, agent_online: bool}`.
- **Agent offline**: actions return the last `probe_results` row plus `agent_online: false`. If no row exists, return `{status: "no_data", agent_online: false, last_seen_at: ...}`.
- **Capability missing**: if the probe wasn't advertised in handshake, return `{status: "unavailable", reason: "...", available_probes: [...]}` *without* attempting RPC.

---

## 10. Resource Safety

- **Per-agent concurrent probe cap**: `MAX_CONCURRENT_PROBES = 4` (tokio `Semaphore`). Scheduled probes that can't acquire are skipped; on-demand probes wait with the request's deadline.
- **Per-probe timeout**: enforced as `tokio::time::timeout(descriptor.default_timeout, probe.run(...))`. Hard kill on expiry. The probe MUST honor `ctx.cancel` to stop cleanly (walkdir loop checks it every iteration; reqwest/hickory natively cancel-safe).
- **Memory budget for `disk.blackholes`**:
  - Streaming accumulation only — never collect entries into a `Vec`.
  - Per-path: 3 `u64` counters + 2 `i64` mtimes + 1 `String` = O(1) memory.
  - Glob expansion bounded by `globwalk(max_depth=6)` + 10k-match cap per pattern.
- **Output size cap**: `ProbeOutput` JSON serialized > 1 MiB is rejected at the agent before sending. Server enforces the same on the receive side to defend against malicious agents.
- **WS backpressure**: probe results stream through a bounded `mpsc::channel(64)` on the agent. Full channel → drop *scheduled* results (log a counter), never drop on-demand results.

---

## 11. WSL-Specific Notes

| concern             | divergence                                              | handling                                                           |
|---------------------|---------------------------------------------------------|--------------------------------------------------------------------|
| `/proc/pressure`    | absent on older WSL2 kernels                            | probe returns `Unsupported`; capability flag prevents future calls |
| `systemd`           | only on opt-in WSL distros (`systemd=true` in wsl.conf) | capability advertisement covers this                               |
| `/proc/net/arp`     | only shows entries on the WSL virtual switch            | document in `network_neigh` response                               |
| DrvFs (`/mnt/c/*`)  | extremely slow `walkdir`                                | `disk.blackholes` defaults exclude `/mnt/*`                        |
| DNS via NAT gateway | resolver `172.x.x.1` flaky under load                   | `net.dns_check` always tags resolver IP                            |
| Docker Desktop      | socket may be at `/mnt/wsl/docker-desktop/...`          | `bollard::connect_with_local_defaults` handles it; document fallback |
| Time drift          | WSL clock skews after host sleep                        | already handled by existing `clock_skew` action                    |

---

## 12. Test Plan

### Unit tests (per probe)

Each probe gets a `*_tests.rs` sidecar following the existing pattern.

- **`disk.usage`**: fixture `/proc/mounts` + mocked `statvfs` via a small trait. Assert pseudo-fs filtered.
- **`disk.blackholes`**: build a temp tree with `tempfile`; populate with known sizes; verify totals, mtimes, file counts. Time-budget test creates 10k files and asserts `truncated_paths` populated when budget = 1ms.
- **`mem.top`**: fixture `/proc/<pid>` dirs under tempdir; inject root path via test-only constructor; assert top-N ordering.
- **`mem.pressure`**: parse known-good and known-bad `/proc/pressure/memory` strings. Property test for malformed input → `InvalidArgs`.
- **`net.neigh`**: parse a captured `rtnetlink` dump (use `rtnetlink::test_helpers` or a recorded byte fixture).
- **`net.dns_check`**: spawn an embedded DNS server (`hickory-server`) on a random port; verify timeout, nxdomain, success paths.
- **`systemd.failed`**: mock `zbus::Connection` via the `zbus_mockito` pattern (or wrap in a trait and inject a fake).
- **`docker.health`**: bollard against a mock HTTP server serving recorded responses.

### Integration tests

- **End-to-end probe RPC**: spin up an in-process agent (WS server) and an in-process MCP server (WS client), exchange handshake, schedule, and on-demand calls. Asserts both DB tables get rows with correct shape.
- **Cancellation**: kick a slow `disk.blackholes`, drop the MCP client, assert agent observes cancel and stops within 100ms.
- **Eviction**: insert 250 rows for one `(host, probe)`, run maintenance, assert 200 remain.
- **Cardinality guard**: bombard `metrics_gauge` with 250 distinct labels, assert insert errors past threshold.

### Smoke (`scripts/smoke-test.sh` additions)

- Hit each MCP action against the running fleet; expect `agent_online: true` for known hosts and a non-empty payload from at least one host.

---

## 13. Open Questions

1. **Per-user scoping for `disk.blackholes`**: agent typically runs as `root` (systemd) or a service user. Should it walk *every* `/home/*` or just its own user's? Proposal: every `/home/*` if running as root, current user otherwise. Operator can override via agent config.
2. **Docker rootless**: `bollard::connect_with_local_defaults` looks at `$DOCKER_HOST` and `/var/run/docker.sock` only. Rootless Docker (`$XDG_RUNTIME_DIR/docker.sock`) — explicit env override needed?
3. **PSI subscription mode**: kernel supports event-driven PSI thresholds (`poll(2)` on the file). Future enhancement: agent subscribes and pushes a `probe.event` when memory pressure crosses a threshold, instead of polling every 30s. Worth doing in V2.
4. **Container metrics**: should `docker.health` *also* emit per-container memory/CPU gauges into `metrics_gauge`? Probably yes, but expands cardinality — defer to V2 with a per-host opt-in flag.
5. **Probe versioning**: if we change a payload shape between releases, do we version per-probe (`"probe": "mem.top@2"`) or rely on additive-only JSON evolution? Lean toward additive-only + an agent-version check at the server when consuming.
6. **Schedule drift on resume from suspend**: laptop hosts can be suspended for hours. Current proposal runs all overdue probes once. Confirm: is "once" enough, or do we want a small backfill (e.g. 3 runs spread over 5min) to populate the gauges chart?
7. **`net.neigh` on bonded / VLAN interfaces**: rtnetlink dump returns all neighbors; do we want to group/tag by master interface? Cosmetic — defer.
