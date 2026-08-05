---
title: "`LogBatchEntry` — Canonical Log Row Shape"
created: 2026-05-16
updated: 2026-07-30
---

# `LogBatchEntry` — Canonical Log Row Shape

**Status:** contract — source of truth
**Owners:** every ingest source MUST produce this shape before handing rows to the writer.
**Companion specs:**
- `docs/superpowers/specs/2026-05-16-enrichment-framework-design.md` (epic `cortex-1wjr`) — adds the four enrichment columns.
- `docs/superpowers/specs/2026-05-16-api-pollers-design.md` (epic `cortex-awvr`) — defines the API-poller envelopes and the `source_ip` URI conventions used here.
- Live struct: `src/db/models.rs::LogBatchEntry`.

---

## 1. Purpose & invariants

Every row in the `logs` table is one `LogBatchEntry`. The shape is the **only** contract between an ingest source and the batch writer — UDP, TCP, Docker stream, Docker event, OTLP, the per-host agent, the UniFi poller, and the AdGuard poller all converge on this struct before any row hits SQLite.

Invariants:

1. **No source-private columns.** A new ingest source MUST NOT add columns; it MUST stuff source-specific information into `metadata_json` under a reserved top-level key (§5).
2. **`source_ip` is the source identity, not just a network address.** It is a URI-like string whose scheme encodes the transport (§4).
3. **Single dispatch surface.** Enrichment parsers (epic 1wjr) and downstream MCP actions key off `app_name`, `container_name`-via-metadata, and `source_kind` (carried in `metadata_json` and reconstructable from the `source_ip` scheme) — nothing else.
4. **Never drop a row on parse failure.** If enrichment/parsing fails, the row is written with whatever fields were populated, plus `parse_error`.

---

## 2. Field reference

The table reflects the current struct in `src/db/models.rs` *plus* the four indexed columns and `parse_error` added by enrichment migration 10 (epic 1wjr).

| Field | Rust type | Null? | Source / semantics |
|---|---|---|---|
| `timestamp` | `String` | no | ISO 8601 timestamp of the event. RFC syslog timestamp when available; Docker / OTLP / poller-provided otherwise. |
| `received_at` | `INTEGER` (unix epoch millis, DB-only) | no | Server-side wall-clock when the row was accepted at the ingest path. **Not a field on `LogBatchEntry`** — populated by SQLite `DEFAULT (unixepoch('now','subsec') * 1000)` at insert time. Differs from `timestamp` (event-time, as claimed by the source). Used by `clock_skew` MCP action, RAG retrieval recency boost (Epic F), and Epic E windowed rules. Indexed for time-range queries. |
| `hostname` | `String` | no | Sender-claimed hostname. Trust boundary — operators may forge this. For poller sources, the polled endpoint's host. |
| `facility` | `Option<String>` | yes | Syslog facility name (`kern`, `auth`, `daemon`, …). `None` for non-syslog sources unless the source synthesises one. |
| `severity` | `String` | no | Canonical syslog severity (`emerg` / `alert` / `crit` / `err` / `warning` / `notice` / `info` / `debug`). Parsers MAY overwrite via `ParserOutput::severity`. |
| `app_name` | `Option<String>` | yes | Syslog APP-NAME, Docker container's `app_name()`, or OTLP `service.name`. Lowercased and trimmed before storage. Primary dispatch key for parsers. |
| `process_id` | `Option<String>` | yes | Syslog PROCID or OS pid. Free-form string (some hosts log `123:abc` style). |
| `message` | `String` | no | Post-normalisation message body (post AI-scrub for AI sources, post timestamp-strip for Docker stream rows). FTS5 indexes this. |
| `raw` | `String` | no | Original received bytes as UTF-8. Used by parsers when normalisation stripped recoverable detail (e.g. quoted strings in SWAG access logs). |
| `source_ip` | `String` | no | URI-style source identifier. See §4. Required, never empty for new rows. |
| `docker_checkpoint` | `Option<DockerCheckpoint>` | yes | Set only by the Docker ingester so the checkpoint store can resume tails across restarts. `None` for every other source. |
| `ai_tool` | `Option<String>` | yes | AI ingest only — `claude` / `codex` / `gemini`. |
| `ai_project` | `Option<String>` | yes | AI ingest only — project / workspace identifier. |
| `ai_session_id` | `Option<String>` | yes | AI ingest only — transcript session id. |
| `ai_transcript_path` | `Option<String>` | yes | AI ingest only — absolute path to the source transcript file. |
| `metadata_json` | `Option<String>` | yes | Serialised JSON object. Top-level keys are reserved per parser / source — see §5. |
| `http_status` | `Option<i32>` | yes | **(added by enrichment migration 10.)** Three-digit HTTP status. Indexed (partial). |
| `auth_outcome` | `Option<&'static str>` | yes | **(added by enrichment migration 10.)** One of `success` / `failure` / `denied` / `challenge`. Indexed (partial). |
| `dns_blocked` | `Option<bool>` | yes | **(added by enrichment migration 10.)** `Some(true)` = filtered, `Some(false)` = explicit allow, `None` = N/A (rewrites + non-DNS rows). Indexed (partial). |
| `event_action` | `Option<String>` | yes | **(added by enrichment migration 10.)** Normalised event verb (`oom_kill`, `link_up`, `die`, `ban`, …). Indexed (partial). |
| `parse_error` | `Option<String>` | yes | **(added by enrichment migration 10.)** `"{parser_name}: {ParserError::Display}"`, truncated to 512 bytes. Not indexed. |

The enrichment fields (`http_status`, `auth_outcome`, `dns_blocked`, `event_action`, `parse_error`) are present in `LogBatchEntry` as of epic B (cortex-1wjr). Ingest sources leave them `None`; the enrichment pipeline populates them at flush time.

---

## 3. `source_kind` enumeration

`source_kind` is a parser-dispatch hint. It is **not** a stored column on `logs` (locked decision (4) in the API-pollers spec); it is reconstructable from the `source_ip` scheme and is also written into `metadata_json.source_kind` for human inspection.

| `source_kind` | String form | Emitted by | Example `source_ip` |
|---|---|---|---|
| Syslog (UDP) | `syslog-udp` | `src/syslog/listener.rs` UDP path | `udp://203.0.113.7:48132` |
| Syslog (TCP) | `syslog-tcp` | `src/syslog/listener.rs` TCP path | `tcp://203.0.113.7:51422` |
| Agent | `agent` | per-host agent WS (`cortex-qgnx`) | `agent://devhost/` |
| Docker stream | `docker-stream` | `src/docker_ingest/` `log_output_to_entry` | `docker://rkx/postgres/stdout` |
| Docker event | `docker-event` | `src/docker_ingest/` `docker_event_to_entry` | `docker-event://rkx/postgres/die` |
| OTLP | `otlp` | `src/otlp.rs` | `otlp://10.0.0.5/<service.name>` |
| UniFi API | `unifi-api` | UniFi poller (`cortex-awvr` UniFi half) | `unifi://controller.lan/` |
| AdGuard API | `adguard-api` | AdGuard poller (`cortex-awvr` AdGuard half) | `adguard://adguard.lan/192.168.10.55` |

Adding a new ingest source requires registering a new `source_kind` here **and** a new URI scheme in §4 — pick a name that does not collide.

---

## 4. `source_ip` URI conventions

The string form is opinionated. Every value MUST be parseable with `url::Url::parse` (so reserved chars in path segments are percent-encoded). The dispatcher and the MCP `source_ip` filter rely on the **scheme** to discriminate transports cheaply without joining `metadata_json`.

| Scheme | Authority | Path | Used by |
|---|---|---|---|
| `udp://` | `<sender_ip>:<sender_port>` | empty | Syslog UDP listener. The ephemeral port is intentional — operators can correlate to specific sender sockets. |
| `tcp://` | `<sender_ip>:<sender_port>` | empty | Syslog TCP listener. Same rationale as `udp://`. |
| `agent://` | `<host_id>` | empty | Per-host agent WS. `host_id` matches the agent's reported identity. |
| `docker://` | `<docker_host>` | `/<container_name>/<stream>` where `stream ∈ {stdout, stderr}` | Docker stream ingester. |
| `docker-event://` | `<docker_host>` | `/<container_name>/<action>` (e.g. `/postgres/die`) | Docker event ingester. |
| `otlp://` | `<peer_ip>` | `/<service.name>` | OTLP receiver. Peer IP omits the ephemeral port (one OTLP exporter, many records). |
| `unifi://` | `<controller_host>` | empty | UniFi poller. V1 is single-site (per `api-pollers-design.md` §11.1) so no site segment. |
| `adguard://` | `<adguard_host>` | empty | AdGuard poller. Per-client filtering uses `metadata_json.adguard.client` rather than baking it into the URI. |

Legacy rows inserted before this convention existed carry empty-string `source_ip`. The MCP `source_ip` filter treats `""` as "no constraint" on read; new writes MUST set a non-empty value.

---

## 4.1 Idempotency keys per source

Per-source dedup discriminator. The ingest path for the listed `source_kind` values rejects duplicates when this key collides with an already-stored row from the same source.

| source_kind | Idempotency key | Source of key | Notes |
|---|---|---|---|
| `syslog-udp` | none | — | Best-effort; duplicates possible if rsyslog retransmits |
| `syslog-tcp` | none | — | Best-effort; OS-level TCP retransmit handled below us |
| `agent` | `(host_id, seq)` | agent-protocol.md §7 | Server idempotent on this pair; `seq` is per-connection monotonic |
| `docker-stream` | `(host, container_id, stream, line_offset)` | docker_ingest checkpoint | Cited from existing `docker_ingest_checkpoints` table |
| `docker-event` | `(host, event_id)` | Docker event JSON | Docker provides stable IDs |
| `otlp` | none | — | Spec/protocol does not require it; we accept duplicates |
| `unifi-api` | `(time_ms, _id)` | UniFi event `_id` | Cited from api-pollers spec |
| `adguard-api` | `(time, client, question)` | AdGuard query record | Spec C pinned this |

**Cross-source dedup is NOT performed.** If an operator runs BOTH rsyslog forwarding AND a `cortex agent` on the same host, the same log line will land twice. Deduplication is an ingest-source-internal property only.

---

## 5. `metadata_json` shape rules

`metadata_json` is the union of (a) source-specific envelope data and (b) parser output (epic 1wjr `ParserOutput::metadata`). Rules:

1. **Top-level keys are namespaced.** Every parser declares a `namespace` (`kernel`, `docker`, `authelia`, `swag`, `adguard`, `fail2ban`); its output lives under that key. Source-specific envelope data uses the corresponding `source_kind` token (e.g. `otlp`, `unifi`).
2. **One reserved key**: `source_kind` at the top level, a string matching §3. This is the only universally-required key when `metadata_json` is `Some`.
3. **Nesting depth ≤ 3.** Parsers MUST NOT produce nested objects more than three levels deep below the namespace key. Keeps `JSON_EXTRACT` queries cheap and bounds the FTS5-trigger surface (logs FTS reindexes on update; deep trees blow up the trigger).
4. **No raw message copies.** Parsers MUST NOT write `message` or `raw` back into `metadata_json` under any key. The original is already in `logs.raw`.
5. **Bounded value sizes.** Free-form strings extracted by parsers (e.g. SWAG `path`, `user_agent`) MUST be truncated at the parser before insertion — per-parser limits live in the enrichment-framework spec §7.

Schema sketch:

```jsonc
{
  "source_kind": "docker-stream",          // §3 token
  "docker": {                              // Docker ingester envelope data
    "container_name": "authelia-main",
    "image": "authelia/authelia:4.38",
    "compose_project": "auth",
    "compose_service": "authelia"
  },
  "authelia": {                            // Parser output (namespace = "authelia")
    "username": "alice",
    "mfa_method": "totp",
    "src_ip": "100.0.0.1",
    "path": "/api/secondfactor/totp",
    "method": "POST"
  },
  "parser": "authelia"                     // Set by dispatcher when a parser ran
}
```

---

## 6. Canonical Rust shape

The struct below is the post-epic-1wjr shape. Use this as the target signature when wiring new ingest sources or extending the enrichment pipeline. Today's `src/db/models.rs::LogBatchEntry` is identical through `metadata_json`; the trailing five fields land with migration 10.

```rust
use serde::{Deserialize, Serialize};

/// The canonical row shape every ingest source produces.
///
/// Source-private fields live in `metadata_json` under a namespace key
/// (see docs/contracts/log-row-shape.md §5). The four indexed columns
/// (`http_status`, `auth_outcome`, `dns_blocked`, `event_action`) and
/// `parse_error` are populated by the enrichment pipeline; ingest sources
/// leave them `None`.
#[derive(Debug, Clone)]
pub struct LogBatchEntry {
    pub timestamp: String,
    // NOTE: `received_at` is NOT a field on this struct. It is a DB-only column
    // populated by SQLite DEFAULT at insert time (unix epoch millis).
    // See docs/contracts/log-row-shape.md §2.
    pub hostname: String,
    pub facility: Option<String>,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub message: String,
    pub raw: String,
    /// URI-style source identifier. See docs/contracts/log-row-shape.md §4.
    pub source_ip: String,
    pub docker_checkpoint: Option<DockerCheckpoint>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_transcript_path: Option<String>,
    pub metadata_json: Option<String>,

    // --- added by enrichment migration 10 (epic cortex-1wjr) -----------
    pub http_status: Option<i32>,
    pub auth_outcome: Option<&'static str>,
    pub dns_blocked: Option<bool>,
    pub event_action: Option<String>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DockerCheckpoint {
    pub host_name: String,
    pub container_id: String,
    pub timestamp: String,
}
```

## Agent Docker identity rows (`agent-docker`)

Host-local cortex agents stream Docker container logs and forward them as
RFC 5424 syslog TCP lines. A syslog APP-NAME over 48 bytes (or empty /
non-graphic) is replaced wholesale with the `cortex-agent` fallback -- not
truncated -- so canonical container identity rides in structured metadata,
not the app label:

```text
Agent Docker identity source: agent-docker.
Structured metadata path: metadata_json.agent_docker.
Required fields: host, container_id, container_name, stream.
Optional fields: compose_project, compose_service, image.
```

Receiver enrichment extracts the agent's `[cortex-agent-docker-meta:{json}]`
message prefix into `metadata_json` and strips it from `message`; `raw`
retains the original line. Canonical resolver proof must use `agent-docker`
structured metadata. `docker://` and `docker-event://` central-pull rows are
not proof for the resolver-backed graph contract.
