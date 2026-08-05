# Agent Mode: WebSocket + JSON-RPC 2.0

**Status:** Draft
**Date:** 2026-05-16
**Epic:** `syslog-mcp-qgnx`
**Sibling:** `syslog-mcp-fue9` (probe registry — out of scope here)

---

## 1. Goal & Non-Goals

### Goal

Add a second, **first-class** ingest path to `syslog-mcp`: a persistent, authenticated, bidirectional channel between the central server (`nashost`) and a lightweight agent process running on each homelab host (`devhost`, `edgehost`, `winhost-wsl`, `laptophost-wsl`, plus future nodes). The channel is **JSON-RPC 2.0 over WebSocket**, terminated by `wss://` in production.

The agent must:

1. Stream local logs (journald, file tails, Docker, app-specific sources) to the server with at-least-once delivery and durable local buffering across restarts.
2. Provide a control surface: the server can issue **pull-on-demand probes** from a hardcoded whitelisted registry (defined by epic `fue9`) and receive structured responses.
3. Represent itself as a long-lived identity — one agent = one host = one row in `agents`. The WebSocket connection's liveness *is* the heartbeat.

### Non-goals

- **Not** replacing the UDP/TCP syslog listener. Routers, switches, printers, embedded gear, and anything else that can't run the agent continue to land on `:1514` exactly as today.
- **Not** designing the probe registry contents. This spec only defines the RPC plumbing that carries probe requests/responses.
- **Not** redesigning storage, FTS5, retention, or the SQL schema for `logs`. Agent rows tag themselves via `source_kind="agent"` and otherwise use the existing path.
- **Not** TLS termination inside the agent or server in v1. Production deployments terminate `wss://` at SWAG (reverse proxy) on `nashost`; agents speak `wss://syslog.example.internal/ws/agent`. Loopback `ws://` is permitted for dev.

---

## 2. Topology

```text
┌───────────────────────────────────────────────────────────────────────┐
│                              nashost                                   │
│                                                                       │
│  rsyslog forwarders ──UDP/TCP :1514──►  ┌─────────────────────────┐   │
│  (routers, IoT, hosts                   │ syslog/listener.rs      │   │
│   running rsyslogd)                     │ (UNCHANGED)             │   │
│                                         └────────────┬────────────┘   │
│                                                      │ IngestTx       │
│                                                      │ (mpsc)         │
│                                                      ▼                │
│  agents ───── wss://…/ws/agent ──►  ┌────────────────────────────┐    │
│  (devhost, edgehost,                  │ mcp/ws_agent.rs (NEW)      │    │
│   winhost-wsl, …)                    │  - axum WS upgrade         │    │
│                                     │  - JSON-RPC 2.0 codec      │    │
│                                     │  - per-conn task           │    │
│                                     │  - probe dispatcher        │    │
│                                     └────────────┬───────────────┘    │
│                                                  │ IngestTx           │
│                                                  ▼                    │
│  HTTP API / MCP (:3100) ──────────►  ┌────────────────────────────┐   │
│                                      │ db/ pool + queries         │   │
│                                      │  logs (metadata.source_kind=…) │  │
│                                      │  agents (NEW table)        │   │
│                                      └────────────────────────────┘   │
└───────────────────────────────────────────────────────────────────────┘
                  ▲                                  ▲
                  │ probe.response                   │ probe.request
                  │ logs.push                        │ config.update
                  └──────────────┬───────────────────┘
                                 │ wss
                ┌────────────────┴──────────────────┐
                │ syslog agent (NEW binary on each  │
                │ remote host)                      │
                │  - tailers (journald, files, ...) │
                │  - local buffer (redb)            │
                │  - JSON-RPC client                │
                │  - probe registry (epic fue9)     │
                └───────────────────────────────────┘
```

Both ingest paths converge at the existing `IngestTx` mpsc, so batching, backpressure, retention, and storage guardrails are unchanged.

---

## 3. Wire Protocol

### 3.1 Transport

- WebSocket (RFC 6455). Text frames, UTF-8 JSON payloads. Binary frames reserved (future log-batch compression).
- Subprotocol negotiation: client sends `Sec-WebSocket-Protocol: syslog-mcp.v1`. Server confirms the same; mismatch → 400 close before upgrade. Bumping to `v2` is how we break compatibility.
- TLS via outer reverse proxy (`wss://`). Loopback `ws://` allowed when `agent.allow_insecure = true` (dev only, refused if bind is non-loopback).
- Max frame size: 1 MiB. Anything bigger is a protocol violation and closes the connection with `1009 Message Too Big`.
- WebSocket-level ping/pong: server pings every 20 s, missing 3 consecutive pongs → close `1011`.

### 3.2 JSON-RPC 2.0 Envelope

We follow [the spec](https://www.jsonrpc.org/specification) verbatim. Three frame kinds:

| Kind         | Shape                                                                   |
| ------------ | ----------------------------------------------------------------------- |
| Request      | `{"jsonrpc":"2.0","id":<u64>,"method":"…","params":{…}}`                 |
| Response     | `{"jsonrpc":"2.0","id":<u64>,"result":{…}}` OR `{"…","error":{…}}`     |
| Notification | `{"jsonrpc":"2.0","method":"…","params":{…}}` (no `id`, no response)    |

**Batching:** server-side **disabled in v1**. Agents MAY send arrays for `logs.push` payloads only via the `entries[]` parameter — i.e., we batch *inside* a single notification rather than using JSON-RPC array batching, which simplifies the codec and avoids partial-failure semantics. The server rejects top-level JSON arrays with `-32600 Invalid Request`.

**Ordering guarantees:**

- Per connection, frames are processed in the order received (single WebSocket = single ordered stream).
- `logs.push` notifications carry a monotonically increasing `seq` per agent. The server records the highest acknowledged `seq` and uses it during reconnect to let the agent skip already-persisted entries (see §8).
- Responses MAY arrive out of order relative to requests on the same side (server may dispatch concurrent `probe.request`s); correlation is by `id` only.

### 3.3 Error Codes

Standard JSON-RPC reserved range plus an application range:

| Code        | Meaning                                              |
| ----------- | ---------------------------------------------------- |
| `-32700`    | Parse error (malformed JSON)                         |
| `-32600`    | Invalid Request (envelope shape wrong)               |
| `-32601`    | Method not found                                     |
| `-32602`    | Invalid params                                       |
| `-32603`    | Internal error                                       |
| `-32000`    | Authentication required (pre-handshake)              |
| `-32001`    | Authentication failed                                |
| `-32002`    | Token revoked                                        |
| `-32003`    | Agent version unsupported                            |
| `-32010`    | Probe not in registry                                |
| `-32011`    | Probe execution failed (agent-side)                  |
| `-32012`    | Probe timed out                                      |
| `-32020`    | Backpressure (server) — agent should slow `logs.push`|
| `-32030`    | Quota exceeded (per-agent rate limit)                |

---

## 4. Method Catalog (v1)

All Rust types are sketches. Real definitions will live under `src/mcp/ws_agent/proto.rs` and derive `serde::{Serialize,Deserialize}` + `JsonSchema` (for tooling).

### 4.1 Client → Server

#### `agent.hello` (request)

First message after WS upgrade. Server MUST receive this within `handshake_timeout = 5s` or it closes with `-32000`.

```rust
struct HelloParams {
    agent_version: String,          // semver, e.g. "0.1.0"
    protocol_version: u16,          // matches subprotocol minor; currently 1
    hostname: String,               // canonical hostname (FQDN if available)
    host_id: String,                // stable UUID v4 generated once, persisted in /var/lib/syslog-agent/host_id
    platform: PlatformInfo,         // os, kernel, arch, distro
    capabilities: Capabilities,     // see below
    token: String,                  // bearer token, opaque
    resume_from_seq: Option<u64>,   // last seq the agent successfully shipped pre-restart
}

struct PlatformInfo { os: String, arch: String, kernel: String, distro: Option<String> }

struct Capabilities {
    supported_probes: Vec<String>,  // names from the registry the agent has compiled in
    sources: Vec<LogSource>,        // ["journald","file","docker", …]
    max_batch_entries: u32,         // hint to server-driven config.update
    compression: Vec<String>,       // e.g. ["zstd"] — v1 server ignores, future
}
```

Server response:

```rust
struct HelloResult {
    server_version: String,
    server_time: String,            // RFC3339, for clock-skew diagnostics
    accepted_seq: u64,              // server's recorded high-water mark; agent resumes from accepted_seq + 1
    config: AgentConfig,            // initial pushed config (intervals, max_batch, retention hints)
    session_id: String,             // UUID, used in log_seq scoping and observability
}
```

Failure: `-32001` (bad token), `-32003` (version too old), `-32002` (revoked).

#### `logs.push` (notification)

The hot path. Agent ships parsed log entries.

```rust
struct LogsPushParams {
    seq_start: u64,                 // seq of entries[0]
    entries: Vec<AgentLogEntry>,    // contiguous, monotonically increasing
}

struct AgentLogEntry {
    seq: u64,
    ts: String,                     // RFC3339 with millis, agent-local clock
    severity: Severity,             // emerg..debug, mapped to existing severity strings
    facility: Option<String>,
    app_name: Option<String>,
    process_id: Option<String>,
    message: String,                // already UTF-8 sanitised by agent
    source: LogSource,              // journald | file:<path> | docker:<container> | custom:<name>
    metadata: Option<serde_json::Value>, // free-form, stored in logs.metadata_json
}
```

No response (notification). Server applies the entries through `IngestTx` and asynchronously sends a `logs.ack` notification (see §4.2) so the agent can advance its durable cursor and free buffer space.

#### `metrics.push` (notification)

Lightweight per-host metrics (load, mem, disk %). Out of scope for v1 storage — server can drop or write to a separate table later. Defined so agents don't need a protocol bump to add it.

```rust
struct MetricsPushParams {
    ts: String,
    metrics: HashMap<String, f64>,   // e.g. {"load1": 0.42, "mem_used_pct": 38.1}
}
```

#### `probe.response` (request → response from agent)

Agents respond to a server-initiated `probe.request` (§4.2) by **replying with a JSON-RPC response** correlated by `id`. There is no separate `probe.response` method — the existing response envelope carries it. Mentioned here for completeness.

#### `agent.heartbeat` (notification)

Defensive: an explicit application-level heartbeat in addition to WS ping/pong. Lets the server tag last-activity in the DB even if no logs flow.

```rust
struct HeartbeatParams { ts: String, queue_depth: u32, buffer_bytes: u64 }
```

Cadence: every 30 s, jittered ±5 s.

### 4.2 Server → Client

#### `probe.request` (request)

```rust
struct ProbeRequestParams {
    probe: String,                   // registry key, e.g. "exec.systemctl_status"
    args: serde_json::Value,         // probe-defined; validated agent-side against registry schema
    timeout_ms: u32,                 // server-enforced deadline
}

struct ProbeResponseResult {
    output: serde_json::Value,       // probe-defined
    exit_code: Option<i32>,
    stderr: Option<String>,
    duration_ms: u32,
}
```

Errors: `-32010` (probe not in agent's `supported_probes`), `-32011` (execution failed — `data` includes stderr), `-32012` (timed out).

#### `config.update` (notification)

```rust
struct ConfigUpdateParams {
    config: AgentConfig,            // full snapshot; agent diffs & applies
    apply_after: Option<String>,    // RFC3339; staggered rollouts
}

struct AgentConfig {
    heartbeat_interval_secs: u32,
    max_batch_entries: u32,
    flush_interval_ms: u32,
    log_sources: Vec<LogSourceConfig>,    // server tells agent which journals/files to tail
    metrics_interval_secs: Option<u32>,
    probe_concurrency: u32,
}
```

#### `logs.ack` (notification)

```rust
struct LogsAckParams { up_to_seq: u64 }
```

Sent periodically (every N batches or every 1 s, whichever first) after the server has durably persisted entries through `IngestTx`. Agent truncates buffer up to and including `up_to_seq`.

#### `agent.shutdown` (request)

```rust
struct ShutdownParams { reason: ShutdownReason, drain_seconds: u32 }
enum ShutdownReason { ServerReload, Revoked, Decommission }
struct ShutdownResult { final_seq: u64 }
```

Agent flushes its buffer (best-effort within `drain_seconds`), replies with the final `seq` it shipped, then disconnects gracefully. If `Revoked`, the agent additionally deletes its token and stops trying to reconnect.

---

## 5. Capability Handshake

On `agent.hello`:

1. **Token check** (§6) — failure short-circuits with `-32001`/`-32002`.
2. **Version gate** — server config holds `min_agent_version`. Older agents get `-32003` with `data: { min_required: "…", upgrade_url: "…" }`.
3. **Host identity:**
   - `host_id` is the canonical key. It's a UUID v4 the agent generates on first run and persists to `/var/lib/syslog-agent/host_id` (mode 0600, root-owned).
   - Hostname is informational and CAN change without re-registration.
   - Server upserts the `agents` row (§9) — `first_seen` set on insert, `last_handshake` on every hello.
4. **Capability registration** — server stores `supported_probes` and `agent_version` on the row. Any later `probe.request` for a probe not in this list is rejected client-side with `-32010` without invoking the registry.
5. **Resume coordination** — server returns `accepted_seq`. Agent SHOULD discard buffered entries with `seq <= accepted_seq` before starting `logs.push`. (See §8 for the race.)
6. **Initial config push** — server returns an `AgentConfig` derived from server config + host overrides. Subsequent updates use `config.update`.

Connection state in `agents` flips to `Active` on successful hello.

---

## 6. Auth Model

### 6.1 Choice: bearer token in the **first JSON-RPC message**

Surveyed options:

| Pattern                                  | Verdict                                                      |
| ---------------------------------------- | ------------------------------------------------------------ |
| `Authorization: Bearer` header           | Works, but SWAG strips/normalises headers inconsistently; clients have to set it pre-upgrade. Browsers (irrelevant here, but worth noting) can't set it on `new WebSocket()`. |
| `?token=…` query param                   | Leaks into nginx access logs, reverse-proxy logs, browser history. **Reject.** |
| `Sec-WebSocket-Protocol` carrying token   | Hacky overload; tools mis-handle it.                         |
| **First JSON-RPC `agent.hello.params.token`** | Token traverses the same TLS channel, lives only in `wss` payload, never in URLs or headers. Server enforces handshake timeout to bound exposure to a connected-but-unauthenticated socket. **Chosen.** |

The unauthenticated socket window is bounded by `handshake_timeout = 5s` and an idle byte counter (1 KiB max before hello).

### 6.2 Token lifecycle

- **Issuance:** `cortex admin agent issue --host devhost` on the server prints a one-time token (32 bytes, base64url). Server stores only its hash (BLAKE3 of the raw token) in `agents.token_hash`. A pending row is inserted with `connection_state = NeverConnected`.
- **Bootstrap on agent:** an operator pastes the token into `/etc/syslog-agent/token` (mode 0600), or feeds it via `syslog-agent register --token <…>` which writes the same file.
- **Storage on agent:** plain file on disk, perms 0600, owned by the dedicated `syslog-agent` user. Not encrypted at rest — tailnet trust + filesystem perms are the boundary, matching every other agent in this class (Promtail, Filebeat).
- **Rotation:** `cortex admin agent rotate --host-id <uuid>` issues a new token; server keeps both old and new `token_hash` for `rotation_grace_secs` (default 300). After grace, old hash is dropped.
- **Revocation:** `cortex admin agent revoke --host-id <uuid>` zeroes both `token_hash` columns and sets `state = Revoked`. The next handshake (or in-flight session, on next operation) gets `-32002`; in-flight session receives a `agent.shutdown` with `reason: Revoked` immediately.
- **Transport assumption:** `wss://` is mandatory in prod. The server refuses to start with `agent.enabled = true` AND non-loopback bind AND `tls.terminated_externally = false` AND `agent.allow_insecure = false`.

---

## 7. Connection Lifecycle

### 7.1 State machine (agent-side)

```text
Disconnected ──(start)──► Connecting
   ▲                         │
   │              wss upgrade│
   │                         ▼
   │                    Authenticating ──(hello accepted)──► Active
   │                         │                                  │
   │                         │ -32001/-32002/-32003             │
   │                         ▼                                  │
   │                      FatalAuth ──(no retry, alert)         │
   │                                                            │
   │                                          close/timeout/io  │
   └─────────────────── Reconnecting ◄──────────────────────────┘
           (backoff)
```

`Revoked` is a terminal sub-state of `Disconnected`: agent stops reconnecting and logs to local syslog.

### 7.2 Reconnect / backoff

Full-jitter exponential backoff over `(base=1s, cap=60s)`:

```text
delay = random_between(0, min(cap, base * 2^attempt))
```

Reset to `attempt = 0` after `Active` for ≥ 60 s. Cap stays at 60 s indefinitely — we want eventual reconnection without burst when the server comes back from a 6 h outage.

### 7.3 Heartbeats

Two layers:

- **WS-level** ping/pong, server-initiated, 20 s interval, 3 missed → close. Catches TCP black-holing fast.
- **App-level** `agent.heartbeat`, 30 s ±5 s jitter, agent-initiated. Updates `agents.last_seen` and serves as the connection-state heartbeat in the DB.

Connection state in `agents` is updated on:
- WS upgrade complete → no row change (still unauthenticated).
- `agent.hello` accepted → `Active`, `last_handshake = now`.
- WS close (any reason) → `Disconnected`, `last_disconnect = now`, `last_disconnect_reason = "<code> <msg>"`.

This **is** the heartbeat — no separate `silent_hosts` table polling. The server can answer "is devhost reachable?" with a single `SELECT connection_state FROM agents WHERE hostname = 'devhost'`.

---

## 8. Local Buffer Queue

### 8.1 Survey

| Option                              | Pros                                                        | Cons                                          |
| ----------------------------------- | ----------------------------------------------------------- | --------------------------------------------- |
| **`sled`**                          | Embedded, crash-safe, ordered keys (perfect for `seq`), pure Rust | Maintenance mode upstream; ~10 MB dep weight |
| `redb`                              | Active dev, ACID, smaller scope, simpler                    | Newer, less battle-tested at scale            |
| `rusqlite` (separate DB)            | Already in tree, well-known                                 | Heavyweight for a FIFO; same-process write contention with future agent SQL features |
| Append-only file + manifest         | Tiny, transparent                                           | We'd re-implement durability, rotation, fsck — not worth it |

**Choice: `redb`.** Active maintenance beats `sled`'s recent stagnation; ACID semantics map cleanly to "advance cursor only after server ACK"; pure-Rust dep with no C build deps simplifies cross-compiling the agent for `winhost-wsl` and `laptophost-wsl`.

### 8.2 Schema

Single `redb` database at `/var/lib/syslog-agent/buffer.redb`:

```rust
// Table: entries
//   key:   u64 (seq, big-endian)
//   value: bincode-encoded AgentLogEntry
// Table: meta
//   key:   &'static str   ("next_seq", "acked_seq", "host_id", …)
//   value: bincode-encoded scalar
```

Ordered key iteration is native to redb's BTree, so replay = `range(acked_seq+1..)`.

### 8.3 Durability tier

- **Write path:** every K=100 entries OR T=200ms (whichever first), an entries write is flushed (`Durability::Immediate`). Between flushes, entries are in-memory only.
- **Crash semantics:** at most K-1 entries lost on power cut. This matches `rsyslog`'s typical defaults and is acceptable for a syslog-grade pipeline.
- **ACK semantics:** `acked_seq` only advances on `logs.ack` from server, in a single committed redb txn that also deletes entries `<= up_to_seq`. So a crash between server-ACK and local-truncate causes duplicates on next connect, not loss — agent sends with `seq` and server is idempotent on `(host_id, seq)`.

### 8.4 Capacity & eviction

- **Soft cap:** 256 MiB on disk (configurable). When approached, agent switches `logs.push` cadence from time-based to pressure-based.
- **Hard cap:** 512 MiB. **Eviction policy: drop-oldest with audit.** On each eviction event the agent emits one `metrics.push` carrying `{"buffer_evicted_entries": N, "buffer_evicted_oldest_seq": S}` and logs locally. Drop-oldest beats back-pressuring the tailers — we'd rather lose 5-min-old debug logs than wedge journald reads and miss new critical events.

### 8.5 Replay protocol

1. On `Active`, agent reads `acked_seq` from meta.
2. Iterates `entries` from `acked_seq+1` onwards in chunks of `max_batch_entries` (server-pushed config).
3. Sends each chunk as `logs.push` with `seq_start = chunk[0].seq`.
4. Continues live tailing in parallel, appending new entries to redb with `next_seq++`.
5. The receive side (handler for `logs.ack`) advances `acked_seq` and deletes acked entries in one redb txn.

If the server's `accepted_seq` from hello is **greater** than agent's `acked_seq` (we crashed before truncating), agent fast-forwards `acked_seq = accepted_seq` and deletes through it before resuming.

---

## 9. `agents` Table

```sql
CREATE TABLE IF NOT EXISTS agents (
    host_id              TEXT PRIMARY KEY,            -- UUID v4 from agent
    hostname             TEXT NOT NULL,               -- last-known; not unique
    agent_version        TEXT,                        -- last hello's reported version
    platform_json        TEXT,                        -- serialised PlatformInfo
    capabilities_json    TEXT,                        -- serialised Capabilities

    token_hash           BLOB,                        -- BLAKE3(token); NULL when revoked
    token_hash_prev      BLOB,                        -- previous hash during rotation grace
    token_rotated_at     TEXT,                        -- RFC3339

    connection_state     TEXT NOT NULL                -- 'NeverConnected'|'Active'|'Disconnected'|'Revoked'
                         DEFAULT 'NeverConnected',
    session_id           TEXT,                        -- current session UUID (Active only)
    accepted_seq         INTEGER NOT NULL DEFAULT 0,  -- high-water mark of acked logs.push

    first_seen           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    last_handshake       TEXT,
    last_seen            TEXT,                        -- updated on every heartbeat/logs.push
    last_disconnect      TEXT,
    last_disconnect_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_agents_hostname ON agents(hostname);
CREATE INDEX IF NOT EXISTS idx_agents_state    ON agents(connection_state);
CREATE INDEX IF NOT EXISTS idx_agents_lastseen ON agents(last_seen);
```

### Lifecycle

| Event                          | Row mutation                                                          |
| ------------------------------ | --------------------------------------------------------------------- |
| `agent issue` admin command    | INSERT with `state = NeverConnected`, `token_hash` set                |
| First successful `agent.hello` | UPDATE state to `Active`, set `agent_version`, `capabilities_json`, etc. |
| Heartbeat / `logs.push`        | UPDATE `last_seen = now`                                              |
| `logs.ack` issued              | UPDATE `accepted_seq = up_to_seq`                                     |
| WS close                       | UPDATE state to `Disconnected`, `last_disconnect`, `last_disconnect_reason` |
| `agent rotate`                 | Set `token_hash_prev = token_hash`, write new `token_hash`             |
| `agent revoke`                 | NULL both token hashes, state → `Revoked`                              |

Migration: added under new schema version `N+1` in `db/pool.rs`, idempotent.

---

## 10. Server WS Endpoint

### 10.1 Route

`/ws/agent` on the existing axum router (port 3100). Authenticated path; **bypasses** the standard `AuthLayer` token check (which is for HTTP `/api/*` and `/mcp`) — instead the route applies its own JSON-RPC-level auth (§6). The route is registered conditionally on `agent.enabled = true`.

### 10.2 Wiring (sketch)

```rust
// src/mcp/ws_agent/mod.rs
pub fn router(state: AgentWsState) -> Router {
    Router::new()
        .route("/ws/agent", any(handle_upgrade))
        .with_state(state)
}

#[derive(Clone)]
pub struct AgentWsState {
    pub ingest: IngestTx,
    pub pool: Arc<DbPool>,
    pub config: AgentServerConfig,
    pub probe_router: ProbeRouter,   // server-side request fanout (see §11)
    pub observability: Arc<RuntimeObservability>,
}

async fn handle_upgrade(
    State(state): State<AgentWsState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.protocols(["syslog-mcp.v1"])
      .max_frame_size(1 << 20)
      .on_upgrade(move |socket| Connection::new(state, socket, addr).run())
}
```

`runtime.rs` composes this router into the existing top-level router alongside `api::router()` and `mcp::router()`.

### 10.3 Concurrency model

**One task per connection.** Inside that task, two halves split off the WS sink/stream:

- **Reader half**: `socket.next()` loop → JSON-RPC decode → dispatch.
  - Notifications & requests (client→server methods) run on a bounded `tokio::spawn` pool per-connection (default 8 concurrent) to keep request handlers from blocking the reader. Order is preserved for `logs.push` by routing it on the reader task itself, not the pool — it's pure forwarding to `IngestTx`.
- **Writer half**: dedicated `tokio::sync::mpsc<OutFrame>` channel (capacity 256). The connection task owns the `WebSocket` sink and drains the mpsc. Any place in the codebase that wants to send to this agent gets an `AgentHandle { tx: Sender<OutFrame>, host_id, … }` and pushes onto the channel.

A global `DashMap<HostId, AgentHandle>` lets the probe dispatcher find the right channel for `probe.request`.

---

## 11. Probe RPC Plumbing

This is the bridge **only**; the registry itself is epic `fue9`.

### 11.1 Server side

```rust
pub struct ProbeRouter {
    handles: Arc<DashMap<HostId, AgentHandle>>,
    pending: Arc<DashMap<RequestId, oneshot::Sender<JsonRpcResult>>>,
    next_id: AtomicU64,
}

impl ProbeRouter {
    pub async fn invoke(
        &self,
        host: &HostId,
        probe: &str,
        args: serde_json::Value,
        timeout: Duration,
    ) -> Result<ProbeResponseResult, ProbeError> {
        let handle = self.handles.get(host).ok_or(ProbeError::NotConnected)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);
        handle.tx.send(OutFrame::Request {
            id, method: "probe.request".into(),
            params: serde_json::json!({ "probe": probe, "args": args, "timeout_ms": timeout.as_millis() }),
        }).await?;
        match tokio::time::timeout(timeout + GRACE, rx).await {
            Ok(Ok(Ok(v)))  => serde_json::from_value(v).map_err(ProbeError::Decode),
            Ok(Ok(Err(e))) => Err(ProbeError::Remote(e)),
            Ok(Err(_))     => Err(ProbeError::Cancelled),
            Err(_)         => Err(ProbeError::Timeout),
        }
    }
}
```

The connection's reader task, on receiving a JSON-RPC **response** with a known `id`, looks up `pending` and fulfils the oneshot.

### 11.2 Agent side

The agent has a `ProbeRegistry` (epic `fue9` content) which is a `HashMap<&'static str, Box<dyn Probe>>`. On `probe.request`:

1. Lookup; missing → reply with `-32010`.
2. Validate `args` against the probe's schema; failure → `-32602`.
3. Spawn under `probe_concurrency` semaphore.
4. Run with `tokio::time::timeout(timeout_ms)`.
5. Reply with `result` or appropriate error code.

No agent-side persistence of probe state; probes are stateless from the protocol's perspective.

### 11.3 Exposing to MCP

A new MCP action `agent.probe` (separate spec) calls `ProbeRouter::invoke`. Inside scope here is only the router primitive and the wire methods.

---

## 12. Co-existence With UDP/TCP Listener

### Source identification

**`source_kind` is stored in `metadata_json`, not as a column on `logs`.** Per the cross-cutting audit locked in `docs/contracts/source-kinds.md` and `docs/contracts/log-row-shape.md` (invariant 1: "no source-private columns"), all ingest sources identify themselves via the `source_ip` URI scheme and the `metadata_json.source_kind` key — not a dedicated SQL column.

The agent path sets `source_ip = "agent://<host_id>/"` on every row it produces. The existing enrichment dispatch (epic 1wjr) and MCP filters key off this URI scheme to distinguish agent rows from UDP/TCP syslog rows.

The closed enumeration of `source_kind` values is pinned in `docs/contracts/source-kinds.md`. Relevant values for this epic:

| Value            | Producer                                | `source_ip` scheme        |
| ---------------- | --------------------------------------- | ------------------------- |
| `syslog-udp`     | UDP listener                            | `udp://…`                 |
| `syslog-tcp`     | TCP listener                            | `tcp://…`                 |
| `agent`          | WS agent path (this spec)               | `agent://<host_id>/`      |
| `docker-stream`  | Docker container stdout/stderr ingester | `docker://…`              |
| `docker-event`   | Docker lifecycle event ingester         | `docker-event://…`        |
| `otlp`           | OTLP HTTP ingest                        | `otlp://…`                |
| `unifi-api`      | UniFi poller (epic C)                   | `unifi://<controller>/`   |
| `adguard-api`    | AdGuard poller (epic C)                 | `adguard://<host>/`       |

### Dedup

A host running BOTH rsyslog→UDP AND the agent will produce two rows per event. Per locked decision (#6): **no dedup in v1.** Mitigation guidance in the deploy docs:

- On hosts where we install the agent, the deploy hook (`scripts/agent-deploy.sh`, future) **disables** the rsyslog forwarding drop-in installed by the existing `syslog-deploy-dropins` skill.
- The agent itself reads journald, not via the syslog socket, so rsyslog on that host can continue to write to disk locally without forwarding.

If duplicates do appear, downstream search queries can filter on `source_ip LIKE 'agent://%'` to get the canonical agent stream.

---

## 13. Observability

### Metrics (Prometheus-style, exposed via existing observability layer)

| Name                                            | Type      | Labels                          |
| ----------------------------------------------- | --------- | ------------------------------- |
| `syslog_agent_connections_active`               | Gauge     |                                 |
| `syslog_agent_handshakes_total`                 | Counter   | `result=ok\|bad_token\|version` |
| `syslog_agent_frames_in_total`                  | Counter   | `method`                        |
| `syslog_agent_frames_out_total`                 | Counter   | `method`                        |
| `syslog_agent_logs_pushed_total`                | Counter   | `host_id`                       |
| `syslog_agent_logs_acked_lag_seq`               | Gauge     | `host_id`                       |
| `syslog_agent_probe_requests_total`             | Counter   | `probe,result`                  |
| `syslog_agent_probe_duration_seconds`           | Histogram | `probe`                         |
| `syslog_agent_disconnects_total`                | Counter   | `reason`                        |
| `syslog_agent_buffer_bytes` (from `metrics.push`) | Gauge   | `host_id`                       |

### Tracing

- One `tracing` span per connection (`agent_conn`, fields: `host_id`, `session_id`, `remote_addr`).
- Per-frame spans for requests (parent = connection span), with method + id.

---

## 14. Security Threat Model

| Threat                                     | Mitigation                                                                   |
| ------------------------------------------ | ---------------------------------------------------------------------------- |
| **Token theft** off agent host             | Filesystem perms (0600), tailnet-only; rotation+revocation primitives in §6. Token only carries auth-as-agent-X privilege — no PII, no admin scope. |
| **Replay** of intercepted hello frame      | TLS terminates at SWAG; replay requires already-broken transport. Tokens are bearer; if attacker has the token, they are the agent. Same model as Telegraf, Promtail. |
| **MITM**                                   | `wss://` mandatory in non-loopback configs; server refuses to start insecure non-loopback. |
| **DoS by malicious agent** (high-rate push, oversize frames, slowloris hello) | Frame size cap (1 MiB), handshake timeout (5s), per-connection rate limit on `logs.push` (configurable, default 50k entries/s), `-32020` backpressure code, per-agent quota enforced via leaky bucket. |
| **DoS by token-flooding** (unauth connects) | Pre-hello byte counter (1 KiB), 5 s timeout, per-source-IP connection rate limit (10/min/IP) at axum middleware. |
| **Server-compromise → agent fan-out**     | Probes are whitelisted by hardcoded registry on the **agent** (epic `fue9`). A compromised server cannot make agents run arbitrary code — at worst it can invoke any whitelisted probe with attacker-controlled args, so probe registry MUST validate args. This is explicitly delegated to `fue9` and called out in its spec. |
| **Privilege escalation via probe args**    | Each probe declares an arg schema and an "effects" classification. `fue9` will define this. Spec only ensures the args travel as `serde_json::Value` and are not interpreted by the plumbing. |
| **Token leak via logging**                 | Tokens never logged: `Display`/`Debug` impls on `HelloParams` redact `token`. Verified by test. |

---

## 15. Test Plan

### Unit

- JSON-RPC codec round-trip: request, response, notification, batch-rejection, malformed cases for each `-326xx` error code.
- Connection state machine: every transition exercised, including `Active → Reconnecting` on three different close codes.
- Backoff calculator: deterministic seed, assert distribution stays in `[0, cap]`.
- redb buffer: write/replay/ack/truncate; crash-recovery test using `std::process::abort()` mid-batch and a fresh process re-opening the file.

### Integration (loopback)

- Spawn server + in-process agent client over a UNIX socket transport adapter.
- Cases:
  - Cold start, hello, push 1000 entries, observe rows in `logs` with `source_kind='agent'`.
  - `logs.ack` causes buffer truncation.
  - Server-initiated `probe.request` round-trip end-to-end against a mock registry (`echo` probe).
  - `agent.shutdown { Revoked }` causes agent to delete its token file.
  - Version-too-old gets `-32003`.
  - Bad token gets `-32001`.
  - `accepted_seq > acked_seq` resume case.

### Chaos / E2E

- Network partition: drop server's TCP socket mid-stream; verify agent reconnects with backoff, replays from `acked_seq + 1`, no duplicates above what the protocol allows (≤ K-1 + un-acked-but-sent).
- Disk full on agent buffer: simulate by capping `buffer.redb` file size; verify drop-oldest fires and `metrics.push` reports `buffer_evicted_entries`.
- Server restart while 5 agents active: verify all reconnect within 60 s and resume without data loss.
- Long-tail clock skew: agent clock 1 h ahead — entries persisted with agent-provided `ts`, server records `received_at = now`; search ranges still work because the existing schema indexes both.

---

## 16. Open Questions

1. **TLS termination location.** Locked answer is "SWAG terminates `wss://`." Should the server *also* support native TLS (rustls) for deployments without SWAG? Probably yes in v1.1; v1 ships proxy-only to limit scope.
2. **Multi-tenant scope.** Spec assumes one trust domain per server. If we ever add Anthropic-issued homelab fleets sharing a server, we need an `agent_group` column and per-group probe whitelists. **Out of scope.**
3. **Agent → MCP feature parity.** Should agent be able to subscribe to log streams *back* from the server (for cross-host correlation queries)? Tempting but bloats v1. Flag for v2.
4. ~~**`metrics.push` storage.**~~ **RESOLVED 2026-05-16 (bead `syslog-mcp-swv9`):** Earlier resolution pre-created a `host_metrics` table here so Epic D would avoid a schema bump. On cross-cutting review we found that table redundant with Epic D's `metrics_gauge` (same purpose: scalar samples per `(host_id, metric_name)` over time). Final decision: **no Epic A migration creates a metrics table**. Epic D's `metrics_gauge` is the canonical target for probe gauges AND any future `metrics.push` writes. V1 still drops incoming `metrics.push` payloads on the floor (see §4 of this spec). When a future v2 wants to wire `metrics.push` to storage, it routes the payload into `metrics_gauge` directly — no migration needed because Epic D already creates the table. Contract: `docs/contracts/db-additions.sql` (Epic D section).
5. **Compression.** `Capabilities.compression: ["zstd"]` declared but unused in v1. Worth wiring through if any agent host produces > 10 MiB/min steady-state — `devhost`'s Plex logs might. **Decision deferred** until we measure.
6. **Bootstrap UX.** One-time tokens via copy-paste vs. printing a `wireguard-style` invite URL the agent can read. Lean toward QR/URL for the next epic.
7. ~~**`cortex agent` CLI subcommand surface.**~~ **RESOLVED — IN SCOPE.** Server-side CLI subcommands (run on `nashost`, operate on the central DB):
   - `cortex agent list` — table of agents: host_id, hostname, connection_state, last_handshake, agent_version
   - `cortex agent issue --host=<h>` — generate and print a one-time enrollment token; record `token_hash` row in `agents` with `connection_state=NeverConnected`
   - `cortex agent revoke <host_id>` — set `connection_state=Revoked`, server-side kicks any active connection
   - `cortex agent rotate <host_id>` — issue a new token, mark old `token_hash_prev` for grace window, agent picks up on next reconnect
   - `cortex agent tail <host_id>` — server-side `tail -f` of recent log rows from that host (convenience wrapper over `search` with `hostname=...`)

   Client-side CLI subcommands (run on the host, by the agent binary):
   - `cortex agent run` — long-lived agent daemon (the existing wire protocol entry point)
   - `cortex agent enroll <token>` — accept a one-time token, perform handshake, store rotated long-lived token in `~/.config/syslog-mcp/agent-token`
   - `cortex agent status` — local-only: print connection state, last-success-push, buffer queue depth, recent errors

   All subcommands go through the same `clap` derive-based parser as the existing `cortex` CLI. See contract: `docs/contracts/agent-cli.md`.

---

*End of spec.*
