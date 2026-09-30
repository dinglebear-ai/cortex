# API Pollers: UniFi + AdGuard

Epic: `syslog-mcp-awvr` (API Pollers)
Status: Design — not yet implemented
Owner: jmagar
Related: epic `syslog-mcp-1wjr` (Enrichment Framework)

## 1. Goal & non-goals

### Goal

Add two server-side polling tasks to `syslog-mcp` that pull events from stateful APIs
which have no native log stream, normalize them into the existing `LogBatchEntry` shape,
and feed them into the same batch writer (`syslog::writer::batch_writer`) and enrichment
dispatch as RFC 3164/5424 syslog. The two sources:

1. **UniFi controller** — site events (`/api/s/<site>/stat/event`) and alarms
   (`/api/s/<site>/rest/alarm`) on a fixed cadence. Surfaces IP conflicts, subnet
   collisions, AP roams, WAN drops, client connect/disconnect.
2. **AdGuard Home** — `/control/querylog` incremental pull. Surfaces DNS queries with
   client, question, answer, block status, upstream, and elapsed time. Directly addresses
   the user's pain: DNS issues on WSL machines that never appear in any log today.

### Non-goals

- **No container log parsing for these sources.** The decision to API-poll is locked.
- **No write paths into UniFi or AdGuard.** Read-only by construction.
- **No new microservice.** Pollers are `tokio::spawn`ed children of `RuntimeCore`.
- **No rustifi dependency.** Reuse UniFi-API knowledge from the `unifi` skill but build a
  focused, embedded poller — rustifi is a query tool, not a watcher.
- **No new MCP tool actions for poller data** — query the resulting rows via existing
  `cortex search`/`tail`/`stats`. Only `cortex status` gets per-poller health.
- **No new auth surface.** Pollers authenticate outbound; nobody authenticates inbound to
  the poller.

## 2. Architecture

```text
                                          ┌─────────────────────────────┐
   AdGuard Home  ──── HTTPS basicAuth ─▶  │  adguard_poller (tokio task) │
   (./control/querylog?older_than=…)      │  cursor: last item time      │
                                          └──────────────┬──────────────┘
                                                         │ normalize → LogBatchEntry
                                                         │ source_kind="adguard-api"
                                                         ▼
   UniFi controller ── HTTPS X-API-KEY ─▶ ┌─────────────────────────────┐
   (/proxy/network/api/s/<site>/…)        │  unifi_poller   (tokio task) │
                                          │  cursor: last event _id+time │
                                          └──────────────┬──────────────┘
                                                         │ normalize → LogBatchEntry
                                                         │ source_kind="unifi-api"
                                                         ▼
                                          ┌─────────────────────────────┐
                                          │       IngestTx              │
                                          │  (mpsc → batch_writer)      │
                                          └──────────────┬──────────────┘
                                                         │
                                                  enrich_entry()  ← adguard_parser
                                                         │           (epic 1wjr)
                                                         ▼
                                          ┌─────────────────────────────┐
                                          │  db::insert_logs_batch      │
                                          │  + poller_checkpoints       │
                                          └─────────────────────────────┘
```

Pollers are spawned from `RuntimeCore::spawn_maintenance_tasks` alongside
`spawn_retention_task` / `spawn_storage_task` / `docker_ingest::spawn_all`. They take
`IngestTx::clone()` and the pool `Arc<DbPool>`, write watermarks back into a new
`poller_checkpoints` table, and emit normalized rows that look syntactically identical
to anything the syslog parser produces — the enrichment layer cannot tell the difference,
and that is the design intent.

## 3. Polling model

A single `tokio::time::interval` per source. Defaults:

| Source   | Interval | Jitter | Backfill on first run |
|----------|----------|--------|-----------------------|
| UniFi    | 30 s     | ±10 %  | 1 h, capped at 3000 events (the `stat/event` server cap) |
| AdGuard  | 15 s     | ±10 %  | configurable hours; default 1 h |

**Single-shot on startup:** the interval is constructed with
`background_interval(Duration::from_secs(N))` (matching `runtime.rs` line 51) so the
**first tick fires after `N`** — not immediately. The poller does a one-time manual
tick at startup before entering the loop so we don't wait 30 s for the first sample.
Jitter is added per-tick (`thread_rng().gen_range(0.9..=1.1) * interval`) so multiple
syslog-mcp restarts inside a homelab don't synchronize.

```rust
async fn run_poller<F, Fut>(name: &'static str, interval_secs: u64, mut tick: F) -> !
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<TickReport>>,
{
    // One-shot kickoff, then steady cadence with jitter.
    run_one_tick(name, &mut tick).await;
    loop {
        let jittered = jitter(interval_secs);
        tokio::time::sleep(jittered).await;
        run_one_tick(name, &mut tick).await;
    }
}
```

A single-threaded `tokio::time::interval` is correct here: only one in-flight HTTP
request per source at a time. We do **not** need `tokio-cron-scheduler` — that crate is
overkill for "poll every N seconds" and adds a chrono/uuid dep we don't need.

## 4. UniFi poller

### Endpoints

| Use      | Path                                              | Method | Cap         |
|----------|---------------------------------------------------|--------|-------------|
| Events   | `/proxy/network/api/s/<site>/stat/event`          | GET    | 3000 rows   |
| Alarms   | `/proxy/network/api/s/<site>/rest/alarm?archived=false` | GET    | unbounded |
| Sysinfo  | `/proxy/network/api/s/<site>/stat/sysinfo`        | GET    | 1 row       |

**Auth:** `X-API-KEY: <UNIFI_API_KEY>` (UniFi OS consoles only — UDM/UCG/UDR/UX/UDW).
TLS verification disabled by default (self-signed certs); overridable.
For legacy controllers omit the `/proxy/network` prefix (`SYSLOG_MCP_POLLERS_UNIFI_LEGACY=true`).

### Cursor strategy

UniFi events have a monotonic `time` field (epoch ms) and a Mongo `_id` ObjectId.
Both `(time, _id)` together are stable and unique. Store `(last_time_ms, last_id)` in
the `poller_checkpoints` table:

```sql
CREATE TABLE poller_checkpoints (
    poller       TEXT NOT NULL,   -- 'unifi-events', 'unifi-alarms', 'adguard-querylog'
    instance     TEXT NOT NULL,   -- controller/site identifier; '' for default
    cursor_a     TEXT,            -- primary cursor (time ms / older_than rfc3339)
    cursor_b     TEXT,            -- secondary cursor (mongo _id / dedup hash anchor)
    last_tick_at TEXT,
    last_error   TEXT,
    PRIMARY KEY (poller, instance)
);
```

Tick algorithm:

1. Fetch `stat/event` (server returns descending by time, 3000-row cap).
2. Filter to entries with `time > cursor_a` **or** `(time == cursor_a && _id > cursor_b)`.
   The compound test handles same-millisecond events safely without dropping any.
3. Reverse the filtered slice so we emit oldest-first (consistent with syslog ordering).
4. Normalize → `IngestTx::send`. Update `(cursor_a, cursor_b)` to the newest emitted
   tuple after a successful send.

Alarms reuse the same pattern but use `archived=false` + `_id` only — alarm `time`
field is creation time and only advances when new alarms appear.

### Pagination & backoff

`stat/event` returns the **most recent 3000**. If we ever fall behind by >3000 events
we silently lose history; we surface this via the `status` MCP action when the count
returned equals 3000 (`saturated=true`). We do not paginate further — the legacy
`rest/event` "oldest first, no limit" endpoint exists, but at default 30 s cadence a
homelab will never see 3000 events between ticks (typical: 0–10).

Backoff (per source):
- 4xx (except 429): mark `last_error`, do not retry until next tick (likely auth/config).
- 429 / 5xx / network: exponential backoff in-tick `1s → 2s → 4s → 8s`, max 3 retries,
  then skip this tick. Cursor is **not** advanced on failure.
- Persistent failure (>5 consecutive failed ticks): mark poller `unhealthy`, surface
  in `cortex status`, keep retrying — never abort the spawned task.

### Mapping to `LogBatchEntry`

Example event from `stat/event` (per the public UniFi schemas):

```json
{
  "_id": "65d3f0a2b8c4d5e6f7a8b9c0",
  "key": "EVT_LAN_IP_Conflict",
  "msg": "Detected an IP conflict involving 192.168.1.42 (mac 00:11:22:33:44:55)",
  "subsystem": "lan",
  "site_id": "default",
  "time": 1731234567890,
  "datetime": "2026-05-15T20:09:27Z",
  "ip": "192.168.1.42",
  "mac": "00:11:22:33:44:55"
}
```

Mapping:

| `LogBatchEntry` field | UniFi source                              |
|-----------------------|-------------------------------------------|
| `timestamp`           | `datetime` (RFC 3339, UTC normalized)     |
| `hostname`            | controller hostname from `sysinfo` (cached) — e.g. `"udm-pro"` |
| `facility`            | `"local0"` (matches our existing convention for non-syslog sources) |
| `severity`            | derived from `key` (see classification table below) |
| `app_name`            | `"unifi"`                                  |
| `process_id`          | `subsystem` (`"lan"` / `"wlan"` / `"wan"`) |
| `message`             | `msg`                                      |
| `raw`                 | the entire JSON event re-serialized        |
| `source_ip`           | `unifi://<controller_host>/`               |
| `metadata_json`       | `{"unifi": {"key": "...", "_id": "...", "ip": "...", "mac": "...", "ap": "...", "ssid": "...", "user": "..."}}` |

`source_kind` is **not** a column on `logs`. Per locked decision (4), poller rows are
identified by their `source_ip` URI scheme (`unifi://...`, `adguard://...`) — same
convention `docker_ingest` already uses (`docker://host/container/stream`). The
enrichment dispatch in epic `1wjr` keys off this URI scheme.

### Severity classification

Mapped from the `key` prefix in `eventStrings.json` (the canonical UniFi event catalog):

| Key pattern                          | severity   |
|--------------------------------------|------------|
| `EVT_*_IP_Conflict`, `EVT_*_DhcpPoolExhausted`, `EVT_*_SubnetConflict` | `err`   |
| `EVT_*_Lost*`, `EVT_*_Disconnected`, `EVT_WAN_*Failover` | `warning` |
| `EVT_*_Connected`, `EVT_*_Restarted`, `EVT_*_Adopted` | `notice`  |
| `EVT_WU_Roam*`                       | `info`     |
| everything else                      | `info`     |

Alarms: severity from the alarm's `severity` field if present; otherwise `warning`.

### High-value example rows

| key                       | severity | message                                                            |
|---------------------------|----------|--------------------------------------------------------------------|
| `EVT_LAN_IP_Conflict`     | err      | `Detected an IP conflict involving 192.168.1.42 (mac …)`           |
| `EVT_WU_Roam_Radio`       | info     | `Client … roamed from AP "Office" to AP "Garage"`                  |
| `EVT_WU_Connected`        | notice   | `Client … connected to AP "Office" on SSID "homelab-5g"`           |
| `EVT_WU_Disconnected`     | warning  | `Client … disconnected from AP "Office"`                           |
| `EVT_GW_WANTransition`    | warning  | `WAN transition: WAN1 down, failover to WAN2`                      |
| `EVT_LAN_DhcpPoolExhausted` | err    | `DHCP pool 192.168.1.0/24 exhausted`                               |

## 5. AdGuard poller

### Endpoints

| Use            | Path                                                       | Auth        |
|----------------|------------------------------------------------------------|-------------|
| Query log      | `GET /control/querylog?limit=500&older_than=<rfc3339>`     | basicAuth   |
| Status         | `GET /control/status`                                      | basicAuth   |

Per the OpenAPI spec at
[github.com/AdguardTeam/AdGuardHome/openapi/openapi.yaml](https://github.com/AdguardTeam/AdGuardHome/blob/master/openapi/openapi.yaml),
the auth scheme is global `basicAuth`. Credentials: `SYSLOG_MCP_POLLERS_ADGUARD_USER` +
`_PASSWORD`. Token-based auth is not exposed.

### Query record shape

Per the same OpenAPI, a `QueryLogItem` has:

- `time` — RFC 3339 timestamp (request start)
- `question` — `{name, unicode_name, type, class}`
- `answer` — array of `{type, ttl, value}`
- `original_answer` — pre-filter answer (when rewritten)
- `client` — client IP
- `client_id` — DoH/DoQ/DoT client id
- `client_info` — `{name, disallowed, disallowed_rule, whois}`
- `client_proto` — `dot|doh|doq|dnscrypt|""`
- `elapsedMs` — string milliseconds
- `upstream` — upstream URL (`tcp://`, `tls://`, `https://`, or IP)
- `cached` — bool
- `reason` — filtering reason enum (`FilteredBlackList`, `Rewrite`, `NotFilteredNotFound`, etc.)
- `status` — DNS rcode (`NOERROR`, `NXDOMAIN`, …)
- `rules` — `[{filter_list_id, text}]`

Example:

```json
{
  "time": "2026-05-15T20:09:27.123Z",
  "question": {"name": "telemetry.example.com", "type": "A", "class": "IN"},
  "client": "192.168.1.42",
  "elapsedMs": "1.234",
  "upstream": "https://dns.quad9.net/dns-query",
  "cached": false,
  "reason": "FilteredBlackList",
  "status": "NOERROR",
  "rules": [{"filter_list_id": 1, "text": "||telemetry.example.com^"}],
  "answer": []
}
```

### Cursor strategy

AdGuard's pagination cursor is `older_than` (timestamp). The endpoint returns entries
**older than** the cursor in **descending** time order. Newest-first pagination is
the opposite of what we want, so:

1. On each tick, fetch with `older_than="" limit=500` (returns newest 500).
2. Walk newest → oldest, accumulating entries with `time > cursor_a`.
3. If the oldest entry in the page still satisfies `time > cursor_a`, page again with
   `older_than=<oldest.time>` until we hit a row with `time <= cursor_a` or the page
   comes back empty.
4. Reverse the accumulated list (oldest-first) and emit via `IngestTx`.
5. Update `cursor_a` to the newest emitted `time`. `cursor_b` is unused for AdGuard.

This bounds the worst case at `ceil(events_since_last_tick / 500)` requests. At default
15 s cadence and a healthy homelab DNS volume of ~40 q/s, one tick = ~600 events =
2 requests — entirely tolerable.

### Pagination & backoff

Same backoff ladder as UniFi (1/2/4/8 s, max 3, mark unhealthy after 5 consecutive
failures). On a paginated tick if request N succeeds and N+1 fails, we still advance
the cursor to the newest **emitted** row — partial progress is durable and the next
tick picks up where we stopped.

### Mapping to `LogBatchEntry` and the `adguard_parser` interface

Per locked decision (2): poller rows go through the **same enrichment dispatch** as
syslog rows. The contract with epic `1wjr`'s `adguard_parser` is: produce a row whose
`source_ip` starts with `adguard://` and whose `message` is a single-line, parser-friendly
text representation of the query. `adguard_parser` then re-parses (or, more likely,
inspects `metadata_json.adguard`) to produce tags like `adguard-allowed`,
`adguard-query`, `adguard-rewrite` — the same tags `runtime.rs` already special-cases
for 7-day retention (line 58).

| `LogBatchEntry` field | AdGuard source                                          |
|-----------------------|---------------------------------------------------------|
| `timestamp`           | `time`                                                   |
| `hostname`            | configured `adguard.hostname` (e.g. `"adguard-nashost"`) |
| `facility`            | `"local6"`                                               |
| `severity`            | `notice` for blocked, `info` otherwise                   |
| `app_name`            | `"adguard"`                                              |
| `process_id`          | `client_info.name` if present, else `client`             |
| `message`             | `"{reason} {client} {question.name} {question.type} → {answer[0].value or ''} ({elapsedMs}ms via {upstream})"` |
| `raw`                 | re-serialized JSON                                        |
| `source_ip`           | `adguard://<hostname>/`                                   |
| `metadata_json`       | `{"adguard": {<full record minus prompt-sized fields>}}` |

`metadata_json.adguard` is the canonical input that `adguard_parser` reads (epic 1wjr).
Keeping the full JSON record there means we don't need a second round-trip if the parser
wants to enrich on `rules[].filter_list_id` or `client_proto`.

### Volume & storage projection

Typical homelab DNS rate: 30–60 queries/s sustained, 200 q/s burst. Conservative
average: **45 q/s ≈ 3.9 M queries/day**. At ~400 B per row (compressed FTS5 + base
table; verified on prod where 4.9 M rows ≈ 1.7 GB), one day = 1.5 GB. With the
hardcoded 7-day retention on `adguard-*` tags (already in `runtime.rs`), steady-state
storage is **~10–12 GB**.

That is too much. **The default poller cadence will be 15 s but the user must opt in**
(`pollers.adguard.enabled = false` by default). When enabled, the storage guardrails
(`max_db_size_mb`, retention task) already in place will protect the DB. We will
also add a `sample_rate` knob (`pollers.adguard.sample_rate = 1.0`) so a user can
ingest 10 % of queries to cut storage 10x while still seeing patterns.

## 6. Dedup strategy

Per locked decision (2) the row produced by the poller is indistinguishable from any
other normalized row downstream, which means there is no natural dedup in
`db::insert_logs_batch`. Idempotency is the **poller's** responsibility:

| Source  | Idempotency key                                | Stored where             |
|---------|-----------------------------------------------|--------------------------|
| UniFi   | `(time_ms, _id)`                              | `poller_checkpoints`     |
| AdGuard | `time` (RFC 3339 ns precision)                | `poller_checkpoints`     |

If two ticks overlap (a slow tick gets pre-empted by the next interval — which we
prevent by structuring the loop as `sleep → tick`, not `tick concurrently`), the
cursor-after-emit invariant guarantees no row is sent twice.

On restart: cursor is loaded from `poller_checkpoints`; first tick fetches everything
with `time > cursor` capped at the API's per-call ceiling (3000 for UniFi, paginated
for AdGuard). If the cursor row is older than the API's retention window we silently
miss the gap — surfaced via `status.<poller>.gap_seconds`.

## 7. Config schema

```toml
[pollers.unifi]
enabled = false
url = "https://192.168.1.1"
api_key = ""              # SYSLOG_MCP_POLLERS_UNIFI_API_KEY
site = "default"
hostname = "udm-pro"      # used as logs.hostname for emitted rows
skip_tls_verify = true
legacy = false
poll_interval_secs = 30
poll_alarms = true
backfill_max_events = 3000

[pollers.adguard]
enabled = false
url = "http://adguard:3000"
username = ""             # SYSLOG_MCP_POLLERS_ADGUARD_USERNAME
password = ""             # SYSLOG_MCP_POLLERS_ADGUARD_PASSWORD
hostname = "adguard"
poll_interval_secs = 15
backfill_hours = 1
sample_rate = 1.0         # 0.0..=1.0, applied per-row pre-emit
page_size = 500
```

Env var equivalents follow the flat `SYSLOG_MCP_*` convention (recorded in
`Memory · Infrastructure & Deployment`):

```bash
SYSLOG_MCP_POLLERS_UNIFI_ENABLED
SYSLOG_MCP_POLLERS_UNIFI_URL
SYSLOG_MCP_POLLERS_UNIFI_API_KEY
SYSLOG_MCP_POLLERS_UNIFI_SITE
SYSLOG_MCP_POLLERS_UNIFI_HOSTNAME
SYSLOG_MCP_POLLERS_UNIFI_SKIP_TLS_VERIFY
SYSLOG_MCP_POLLERS_UNIFI_LEGACY
SYSLOG_MCP_POLLERS_UNIFI_POLL_INTERVAL_SECS
SYSLOG_MCP_POLLERS_UNIFI_POLL_ALARMS

SYSLOG_MCP_POLLERS_ADGUARD_ENABLED
SYSLOG_MCP_POLLERS_ADGUARD_URL
SYSLOG_MCP_POLLERS_ADGUARD_USERNAME
SYSLOG_MCP_POLLERS_ADGUARD_PASSWORD
SYSLOG_MCP_POLLERS_ADGUARD_HOSTNAME
SYSLOG_MCP_POLLERS_ADGUARD_POLL_INTERVAL_SECS
SYSLOG_MCP_POLLERS_ADGUARD_BACKFILL_HOURS
SYSLOG_MCP_POLLERS_ADGUARD_SAMPLE_RATE
SYSLOG_MCP_POLLERS_ADGUARD_PAGE_SIZE
```

Validation (added to `validate_config`):

- If `pollers.unifi.enabled = true`: `url` and `api_key` must be set and non-blank.
- If `pollers.adguard.enabled = true`: `url`, `username`, `password` must be set.
- Reject `poll_interval_secs < 5` for both pollers (matches our existing
  `cleanup_interval_secs ≥ 5` floor).
- Reject `adguard.sample_rate` outside `[0.0, 1.0]`.
- Reject `adguard.page_size` outside `[1, 5000]`.

## 8. Credential storage & rotation

- Credentials live in env vars (preferred) or `~/.syslog-mcp/.env` (already loaded by
  `load_setup_env_file()` in `config.rs:715`). They never land in `config.toml` checked
  into a repo, and they never land in the DB.
- The `.env` file enforces non-symlink + ignores invalid keys (existing behaviour
  preserved).
- Rotation: change the env var, send `SIGHUP`... we don't actually support SIGHUP today.
  Rotation procedure is **bounce the syslog-mcp container** (`docker compose restart
  syslog-mcp`). Pollers reload credentials only at startup. This is acceptable: rotation
  is rare and we already have rolling-restart docs from the OAuth epic.
- On API auth failure (UniFi 401, AdGuard 401), surface a clear error in `status` and
  in tracing logs (`"unifi auth failed — rotate SYSLOG_MCP_POLLERS_UNIFI_API_KEY"`).

## 9. Error handling

| Class                              | Action                                                  |
|------------------------------------|---------------------------------------------------------|
| Transient (5xx, 429, network)      | Exponential backoff 1/2/4/8 s, max 3 retries in-tick    |
| Persistent auth (401, 403)         | Mark unhealthy, do not advance cursor, log every tick   |
| Bad payload (parse error)          | Increment `parse_errors`, advance cursor past offender, log first 256 chars of payload |
| Backpressure (`IngestTx::Full`)    | Drop row, increment `dropped_rows` counter, do **not** advance cursor — retry next tick |
| Storage write-blocked              | Already handled downstream by `flush_batch` — poller is oblivious |

The poller task **never panics out**. A failed tick logs at `error` with structured
fields (`poller`, `cursor_before`, `error`, `elapsed_ms`) and waits for the next tick.

## 10. MCP exposure: `cortex status` extensions

Today `tool_get_status` (`src/mcp/tools.rs:503`) returns a tight JSON with `db_ok`,
`runtime_observability`, and `otlp` counters. Add a `pollers` object:

```json
{
  "status": "ok",
  "db_ok": true,
  "pollers": {
    "unifi-events": {
      "enabled": true,
      "healthy": true,
      "last_tick_at": "2026-05-15T20:09:27Z",
      "last_tick_age_seconds": 12,
      "lag_seconds": 1,
      "rows_emitted_total": 14283,
      "rows_dropped_total": 0,
      "consecutive_failures": 0,
      "saturated_last_tick": false,
      "last_error": null
    },
    "unifi-alarms":     { ... },
    "adguard-querylog": { ... }
  }
}
```

`healthy = consecutive_failures < 5`. `lag_seconds = now - cursor_a` (how stale the
data is). `saturated_last_tick = true` when the UniFi `stat/event` page hit the 3000-row
cap — a signal the user should shorten `poll_interval_secs`.

Counters live in `Arc<PollerObservability>`, mirroring the existing
`Arc<RuntimeObservability>` pattern. No new MCP action is added; this rides under
`cortex status`.

## 11. Backfill on first run

- UniFi: pull a single `stat/event` page (3000 rows), filter to entries within
  `backfill_max_events` of `now`. The API caps at 3000 server-side anyway. Set cursor
  to the newest fetched.
- AdGuard: pull pages newest → oldest until either (a) we exceed `backfill_hours` of
  history or (b) we hit the AdGuard server's own log retention floor. Cap at 50
  pages × `page_size` to bound startup latency.
- If `cursor_a` already exists (warm start), skip backfill entirely and resume from
  the persisted watermark.

This is configurable but defaults are conservative: 1 hour of history on cold start.
Users who want a clean slate can `DELETE FROM poller_checkpoints WHERE poller = '…'`.

## 12. Test plan

1. **Unit: cursor advancement.** `(cursor_a, cursor_b)` advances exactly when a row is
   successfully sent to `IngestTx`, never on failure. Property-test with random
   interleavings of success/failure.
2. **Unit: dedup invariant.** Replay the same JSON page twice; second invocation emits
   zero rows.
3. **Unit: severity classification.** Table-driven test mapping `EVT_*` keys to
   `severity` per §4.
4. **Unit: AdGuard paginated tick.** Fake a 3-page response (1500 rows, 500/page),
   verify oldest-first emission order and final cursor at the newest row.
5. **Integration: mock HTTP server.** Spin a `tokio` listener returning canned UniFi
   and AdGuard fixture responses; assert the poller writes the expected rows into a
   real SQLite (in-memory). Reuse the `Mock-HTTP` pattern from the existing
   `docker_ingest` tests.
6. **Integration: backoff ladder.** Mock server returns 503 four times then 200;
   assert exactly 3 retries with sleep durations within 10 % of `1/2/4 s`.
7. **Integration: backpressure.** Drain the `IngestTx` channel slower than the
   poller; assert `dropped_rows` increments and cursor does not advance.
8. **Smoke: live UniFi.** Optional CI step gated on `UNIFI_URL`/`UNIFI_API_KEY` env;
   asserts ≥ 0 rows emitted and `healthy=true` after one tick.
9. **Smoke: live AdGuard.** Same gating against the homelab AdGuard.

## 13. HTTP client choice

`syslog-mcp` currently uses `hyper` + `hyper-util` directly (docker_ingest path). Adding
`reqwest` is justified for the pollers because:

- TLS is required (UniFi HTTPS, AdGuard often HTTPS in production). `hyper` doesn't
  ship a TLS stack; we'd need to wire `rustls`/`hyper-rustls` manually anyway.
- Basic auth and `X-API-KEY` headers are one-liners in `reqwest::Client::builder()`.
- Per Context7 / current docs, the idiomatic 2026 `reqwest` async pattern is
  `Client::builder().danger_accept_invalid_certs(skip_tls).build()?.get(url).headers(...)
  .send().await?.error_for_status()?.json::<T>().await?` — clean fit for the tick body.

Add `reqwest = { version = "0.12", default-features = false, features = ["json",
"rustls-tls", "gzip"] }`. Disable default features to avoid pulling `native-tls`.

## 14. Open questions

1. ~~**Multi-site UniFi.**~~ **RESOLVED — OUT OF SCOPE.** v1 is single-site only. Config schema fixes `site` to the default site (`default`). If a second site ever shows up the migration is additive (extend `instance` in `poller_checkpoints` to `<controller>:<site>`).
2. ~~**AdGuard query log "anonymized" mode.**~~ **RESOLVED.** Drop the anonymization handling entirely. Per user: we do not run with `anonymize-client-ip` and will not add code for it. If a future user does, they get `0.0.0.0` source rows — no parser branching, no normalization.
3. **DNS rewrite chains.** `original_answer` vs `answer` divergence is the "rewrite happened" signal. Should `adguard_parser` (epic 1wjr) tag those rows specifically (`adguard-rewrite`) — yes, but defining the tag set is `1wjr`'s job, not ours.
4. ~~**Cursor migration.**~~ **RESOLVED — manual CLI.** If a UniFi controller is replaced (new `_id` ObjectId space) the cursor goes stale and the poller silently fetches nothing. We do NOT auto-detect; we expose a CLI command:
   ```
   cortex pollers reset --source=unifi   # rewinds the UniFi cursor to "now - 1h"
   cortex pollers reset --source=adguard
   cortex pollers reset --all
   ```
   Implementation: deletes the row from `poller_checkpoints` (next tick treats it as cold-start with the configured `backfill_hours`). Also dump current cursor state:
   ```
   cortex pollers status     # per-source: enabled, last_tick_at, cursor, lag, errors
   ```
   See contract: `docs/contracts/api-pollers.md`.
5. **`rustifi` symbiosis.** Should we publish the embedded UniFi client as a small
   internal crate to share with rustifi? No — keep it inline for now; if a second
   consumer appears, factor out.

---

**Sources used while drafting:**

- AdGuard Home OpenAPI: [github.com/AdguardTeam/AdGuardHome/openapi/openapi.yaml](https://github.com/AdguardTeam/AdGuardHome/blob/master/openapi/openapi.yaml)
- UniFi controller API (community): [ubntwiki.com/products/software/unifi-controller/api](https://ubntwiki.com/products/software/unifi-controller/api)
- UniFi event catalog: `eventStrings.json` referenced by [oznu/unifi-events](https://github.com/oznu/unifi-events)
- Local `unifi` skill: `/home/jmagar/.claude/skills/unifi/SKILL.md`
- syslog-mcp internals: `src/runtime.rs`, `src/ingest.rs`, `src/syslog/writer.rs`, `src/db/ingest.rs`, `src/db/models.rs`
