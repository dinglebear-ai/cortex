---
title: "Design: Authenticated Bidirectional WebSocket Agent Channel"
created: 2026-06-21
updated: 2026-07-30
---

# Design: Authenticated Bidirectional WebSocket Agent Channel

**Status:** Proposed
**Branch:** `claude/agent-server-communication-erfc9f`
**Type:** `feat` (minor version bump)

## Goal

Unify the cortex **agent's** two phone-home channels onto a single authenticated,
bidirectional WebSocket connection carrying:

- heartbeats (today: HTTP `POST /v1/heartbeats`)
- logs (today: RFC 5424 syslog over unauthenticated TCP on 1514)
- server→agent control (today: `agent_update` piggybacked on the heartbeat `202` body)

### Non-goals / hard constraints

- **The generic syslog receiver on port 1514 (UDP+TCP) stays permanently.** It
  receives RFC 3164/5424 from third-party homelab devices (UniFi, routers,
  rsyslog, WSL) that cannot speak WebSocket. WS only replaces the *cortex
  agent's own* two channels — it is not a wholesale protocol replacement.
- **No durable on-disk WAL** on the agent. Delivery is *bounded* at-least-once
  via an in-memory ring buffer; sustained outages degrade to lossy with a metric.
- **Duplicates tolerated initially** — no server-side dedup. Add per-message-id
  dedup later only if it proves necessary.

## Current state (verified in code)

| Concern | Today | File |
|---|---|---|
| Heartbeat | HTTP POST JSON, 30s, `202` w/ `agent_update`, `RetryBuffer` (32, backoff `250ms*2^min(n,4)` cap 4s) | `src/heartbeat_agent.rs`, server `src/heartbeat.rs` |
| Logs | persistent TCP, newline-framed RFC 5424, mpsc cap 4096, backoff `500ms*2^min(n,6)` cap 30s, **at-most-once** | `src/agent/syslog_sender.rs` |
| Collectors | docker / journald / syslog-file / file-tails → finished RFC 5424 string → `sender.try_send(line)` | `src/agent/{docker,journald,syslog_file}.rs` |
| Receiver (1514) | UDP+TCP → `parse_syslog` → `stamp_source_kind` → `ingest.try_send` | `src/receiver/listener.rs` |
| HTTP server | axum 0.8, all routers merged on port 3100 | `src/main.rs:418-468` |
| Auth | `is_authorized` — loopback exempt, else constant-time bearer | `src/heartbeat.rs:283` |

## Crate situation

- `axum = "0.8"` is declared **with no features**; WebSocket support
  (`axum::extract::ws`) needs `features = ["ws"]`. → enable it.
- Agent is a *client* dialing out; axum cannot dial. Add
  `tokio-tungstenite` (rustls, matching the existing reqwest rustls stack) for
  the agent side. `futures-util` (already present) splits the socket.

## Endpoint & framing

- **`GET /v1/agent/ws`** mounted on the existing 3100 listener via a new
  `agent_ws_router()`, beside `heartbeat_router()` and `/v1/agent/binary`.
- **JSON text frames** (matches every existing wire format). One JSON object per
  frame; WebSocket is message-delimited so no manual newline framing.
- Frame-size guard mirroring `HEARTBEAT_BODY_LIMIT_BYTES` (256 KiB) + separate
  log-batch cap, via `WebSocketUpgrade::max_message_size()`.

### Protocol envelope (`src/agent_ws/protocol.rs`, shared)

```rust
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentToServer {
    Hello { schema_version: u8, host_id, agent_version, boot_id, last_acked_seq: Option<u64> },
    Heartbeat { payload: HeartbeatPayload },   // reuse existing struct unchanged
    LogBatch { seq: u64, lines: Vec<String> }, // RFC 5424 strings, unchanged
    // liveness via protocol-level Pong frames (no app-level variant)
}

#[serde(tag = "type", rename_all = "snake_case")]
enum ServerToAgent {
    Welcome { server_version: String, resume_from_seq: Option<u64> },
    HeartbeatAck { agent_update: Option<AgentUpdateDirective> },
    LogAck { seq: u64 },          // cumulative: acks all batches <= seq
    Control(ControlMessage),       // agent_update / config_push / run_command (reserved)
    // liveness via protocol-level Ping frames (no app-level variant)
}
```

- **Cumulative ack**: `LogAck { seq }` acks all batches with `batch.seq <= seq`.
  Agent assigns monotonic `seq` per connection-lifetime sequence space.

## Auth

WS upgrade is an HTTP GET, so auth reuses `is_authorized` **before**
`ws.on_upgrade`:

- Bearer token in the `Authorization` header of the handshake (agent sets it on
  the tungstenite request).
- Loopback exemption identical to heartbeat/OTLP.
- Non-loopback without token → reject upgrade with `401`; emit the same startup
  warning as `main.rs:448`.

## Agent side (`src/agent/ws_client.rs`)

- **`trait LogSink`** implemented by both `SyslogSender` (existing) and
  `WsLogSink` (new). `run_agent_streams` takes `Arc<dyn LogSink>`; collectors
  change only their parameter type — `sender.try_send(line)` call sites unchanged
  (minimal blast radius).
- **Lifecycle**: dial `ws(s)://host:3100/v1/agent/ws` (scheme from heartbeat
  target: `http→ws`, `https→wss`) → `Hello{last_acked_seq}` → `Welcome` → split
  into writer/reader tasks → on disconnect retain ring buffer, reconnect, replay.
- **Backoff**: standardize on the heartbeat values (`250ms*2^min(n,4)` cap 4s),
  since the channel now carries heartbeats. Reset on `Welcome`.
- **Keepalive**: prefer WebSocket protocol-level ping/pong for liveness; treat
  silence > 2×interval as dead. (App-level `Ping`/`Pong` only if RTT metrics
  wanted — see open question.)
- **`LogBatchRingBuffer`** (mirrors `RetryBuffer`): `VecDeque<UnackedBatch{seq,lines}>`,
  bounded by count and/or bytes. Flow: collectors → mpsc(4096) → batcher
  (coalesce ≤N lines / ≤200ms, assign seq, push clone into ring, send frame).
  On `LogAck{seq}` pop all `<= seq`. On reconnect replay all retained in seq
  order. On overflow `pop_front` oldest + increment `ws_log_batches_dropped_overflow`
  + warn. **No WAL.**
- **Heartbeats over WS**: `run_agent` hands `HeartbeatPayload` to the WS outbound
  channel and correlates the next `HeartbeatAck` (carries `agent_update`).
  Existing collector / self-update / `confirm_update_success` logic preserved —
  only transport changes. `RetryBuffer` subsumed (keep newest snapshot only).
- **Binary self-update** stays an HTTP GET to `/v1/agent/binary` (multi-MB stream
  unsuited to the control WS); `agent_update` directive still arrives over WS.

## Server side (`src/agent_ws/`)

- **State**: `AgentWsState { pool, ingest, api_token, auth_policy, release }`,
  built by `RuntimeCore::agent_ws_router()` (mirrors `heartbeat_router()`, adds
  `ingest`).
- **`handle_socket`**: split sink/stream; track `last_acked_seq`, `host_id`, peer.
  - `Hello` → `Welcome{resume_from_seq: last_acked_seq}` (best-effort; server
    keeps no durable cursor).
  - `LogBatch` → per line `parse_syslog` + `stamp_source_kind(.., AgentWs)` +
    `ingest` (the exact receiver path). **Recommend `ingest.send().await` then
    ack** so backpressure flows to the agent rather than dropping; advance
    `last_acked_seq` and emit `LogAck`. On enqueue failure, do **not** ack (agent
    replays) → end-to-end at-least-once into the ingest queue.
  - `Heartbeat` → existing insert path (`spawn_blocking`), reply `HeartbeatAck`
    with `release.directive_for(os, arch, version)`.
  - Keepalive ping; close on pong timeout.
- **Control registry**: per-connection `mpsc` keyed by `host_id` in a
  `Mutex<HashMap<..>>` on the state, so future on-demand commands can target a
  host. Ships as a wired stub; only heartbeat-ack `agent_update` produces today.

## Migration path

- New config `CORTEX_AGENT_TRANSPORT` ∈ `{syslog_tcp (default), websocket}`.
  Default unchanged → existing deployments unaffected on upgrade.
- Server is **purely additive**: always mounts 1514 receiver + `/v1/heartbeats`
  + new `/v1/agent/ws`. Zero risk to existing agents/devices.
- Agent picks transport; `websocket` wires heartbeats + collectors to `WsLogSink`
  and skips reqwest-heartbeat + `SyslogSender`.
- Phases: (1) ship both, default off → (2) flip individual agents, watch metrics
  → (3) change default → (4) later, retire the agent's HTTP-heartbeat + TCP-sender
  paths (never the 1514 receiver).
- `HeartbeatPayload` schema reused unchanged → identical DB rows across
  transports. `schema_version` in `Hello` guards incompatible agents.

## Config additions

Agent (`heartbeat_agent.rs::from_env`): `CORTEX_AGENT_TRANSPORT`,
`CORTEX_AGENT_WS_TARGET` (derived from heartbeat target if absent),
`CORTEX_AGENT_WS_RING_LIMIT` (default ~256 batches),
`CORTEX_AGENT_WS_BATCH_MAX_LINES` / `_MS` (256 / 200ms),
`CORTEX_AGENT_WS_PING_SECS` (20). Reuses existing token vars.

Server (`config.rs`, `[agent_ws]`): `max_frame_bytes` (256 KiB),
`log_batch_max_bytes` (1 MiB).

## Testing

- `agent_ws/protocol_tests.rs` — serde round-trip per variant; schema_version.
- `agent/ws_client_tests.rs` — ring buffer push/ack/overflow + metric; seq
  monotonicity; replay order; ws-url derivation.
- `agent_ws/handler_tests.rs` — cumulative ack; backpressure→no-ack→replay;
  heartbeat routing returns directive; auth (loopback exempt / bearer required).
- `tests/agent_ws_integration.rs` — full axum app on ephemeral port,
  `LoopbackDev`, real tungstenite client: `Hello`+`Heartbeat`+`LogBatch` →
  assert `Welcome`/`HeartbeatAck`/`LogAck` + row in SQLite. Style of
  `tests/rmcp_compat.rs`. `tokio::test(start_paused)` for timing.
- Negatives: oversize frame → policy close; bad bearer → 401 no upgrade;
  malformed JSON → close (no panic).

## File-by-file changes

**New:** `src/agent_ws/{mod,protocol,handler}.rs` + `*_tests.rs`,
`src/agent/ws_client.rs` + tests, `tests/agent_ws_integration.rs`.

**Modified:**
- `Cargo.toml` — `axum` features `["ws"]`; add `tokio-tungstenite`.
- `src/main.rs` / `src/lib.rs` — declare + mount `agent_ws_router()`; non-loopback warning.
- `src/runtime.rs` — `agent_ws_router()` (mirrors `heartbeat_router()`, passes `ingest`).
- `src/heartbeat.rs` — extract `is_authorized`/`unauthorized` to shared helper;
  expose `insert_heartbeat` + `AgentReleaseInfo::directive_for` to `agent_ws`.
- `src/agent.rs` — `LogSink` trait; `run_agent_streams` takes `Arc<dyn LogSink>`;
  `ws_url_from_heartbeat` + tests.
- `src/agent/syslog_sender.rs` — `impl LogSink`.
- `src/agent/{docker,journald,syslog_file}.rs` — sender param type → `Arc<dyn LogSink>`.
- `src/heartbeat_agent.rs` — transport branch; WS fields in config + `from_env`.
- `src/config.rs` — `[agent_ws]` table + validation.
- `src/observability.rs` — counters: `agent_ws_connections_active`,
  `agent_ws_log_batches_received/_acked`, `agent_ws_log_lines_dropped_backpressure`,
  agent-side `ws_log_batches_dropped_overflow`.
- `src/enrich.rs` — optional `SourceKind::AgentWs` (else reuse `SyslogTcp`).
- `CHANGELOG.md`, `docs/`.

## Version

This refers to the **implementation** PR, not this design-doc PR. The
implementation is a `feat` → **minor** bump (project rule: features bump the
minor) via `cargo xtask bump-version minor`, taking whatever the then-current
version is to the next minor (e.g. from the current `1.32.x` baseline → `1.33.0`).
CHANGELOG entry required.

(This design-doc PR itself is a docs-only change and uses a `patch` bump from
the current base version, preserving the minor bump for the implementation.)

## Decisions

1. **Keepalive — DECIDED: protocol-level ping/pong.** Use WebSocket's built-in
   `Message::Ping`/`Pong` for liveness; drop the app-level `Ping`/`Pong`
   envelope variants (no RTT metric requirement). Treat silence > 2×interval as
   dead and force reconnect.
2. **Backpressure — DECIDED: `ingest.send().await` + ack.** The server awaits
   enqueue and only emits `LogAck` after the batch is in the ingest queue;
   backpressure flows back to the agent rather than dropping. On enqueue failure,
   do not ack (agent replays).

## Open questions / decisions needed

1. **At-least-once boundary**: guarantee holds only up to the ingest queue
   accepting the entry; the ingest channel + lack of WAL mean it's
   "at-least-once into the ingest queue," not "into SQLite." State explicitly.
4. **tokio-tungstenite version/rustls** must match the resolved tungstenite and
   the existing reqwest rustls stack (avoid two rustls versions).
5. **Control registry** ships as a stub (no external producer yet) — confirm
   on-demand commands are deferred.
6. **host_id collision** on fast reconnect → brief dual connections (duplicate
   logs tolerated); registry uses last-writer-wins, abort stale sender.
7. **`/v1/agent/binary` stays HTTP** — confirm acceptable (it is).
8. **TLS** via the same reverse-proxy termination as the rest of 3100;
   plaintext-exposure warning applies to WS too.
