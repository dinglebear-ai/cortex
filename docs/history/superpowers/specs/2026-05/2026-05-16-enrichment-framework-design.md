# Enrichment / Parser Framework — Design Spec

**Epic:** `syslog-mcp-1wjr`
**Status:** Design — not implemented
**Author:** Architecture pass, 2026-05-16
**Companion:** AdGuard API poller (`syslog-mcp-awvr`) — shares parser code paths.

---

## 1. Goal & non-goals

**Goal.** Extract structured fields from a fixed set of recognised log sources (kernel OOM/network, Docker events, Authelia, SWAG/nginx, AdGuard, fail2ban) into four new indexed columns plus the existing `metadata_json` blob, so the MCP `cortex` tool can answer questions like *"show me every 5xx upstream from SWAG in the last hour"*, *"which IPs got banned by fail2ban yesterday"*, or *"how many OOMs on rkx in the last week"* with cheap SQL instead of FTS scans. The framework must coexist with raw syslog ingestion (today's hot path), the Docker socket-proxy ingester, and the AdGuard API poller — one parser per source, dispatched once, regardless of how the log entered the system.

**Non-goals.** Generic user-defined parsers (no VRL/grok DSL — V1 is hard-coded Rust). No retroactive backfill of the existing 4.9M rows (lazy/opt-in only). No replacement of the existing AI-message scrubbing pipeline (it stays). No new exfiltration surface — parsers read only what the writer already holds. No multi-line or stateful parsing across records in V1 (each record stands alone). No regex-DSL configurability — pattern tweaks ship as code changes.

---

## 2. Architecture overview

The new framework sits between syslog parsing and the SQL `INSERT` — at the same point in the pipeline as the existing `enrich_entry` in `src/syslog/enrichment.rs`. Every entry, regardless of origin (RFC syslog, Docker stream, Docker event, OTLP, AdGuard API poller), is already shaped into a `LogBatchEntry` before it reaches the writer. The dispatcher therefore needs no new transport — it runs on the writer hot path against an already-decoded envelope.

```text
+-----------------+   +---------------+   +-----------------+
| syslog UDP/TCP  |   | docker stream |   | adguard API     |
| (parse_syslog)  |   | (docker_ingest|   | poller          |
|                 |   |  ::parser)    |   | (epic awvr)     |
+--------+--------+   +-------+-------+   +--------+--------+
         |                    |                    |
         v                    v                    v
              +---------------------------+
              |   LogBatchEntry envelope  |
              | (app_name, message, raw,  |
              |  source_ip, metadata_json)|
              +-------------+-------------+
                            |
                            v
              +---------------------------+
              | EnrichmentPipeline        |
              |  1. AI scrub (existing)   |
              |  2. dispatch(envelope)----+---> select Parser
              |  3. apply ParserOutput    |     by (app_name, container)
              +-------------+-------------+
                            |
                            v
              +---------------------------+
              | http_status, auth_outcome,|
              | dns_blocked, event_action |  (indexed columns)
              | + merged metadata_json    |  (free-form fields)
              | + parse_error column      |  (on failure)
              +-------------+-------------+
                            |
                            v
                      INSERT into logs
```

A parser is a pure function from `&ParserInput` to `Result<ParserOutput, ParserError>`. Dispatch is a static table keyed on a normalised source identifier; lookup is `O(1)` and runs once per row. Parser failure does **not** drop the row — it returns `Err`, the framework records the error in `parse_error`, and the row is written with whatever bits the parser managed to populate before failing (or none).

---

## 3. Parser trait / interface

```rust
// src/enrich/parser.rs

/// Input handed to every parser. Borrowed — parsers do not mutate the envelope
/// directly; they return a ParserOutput that the dispatcher applies.
#[derive(Debug)]
pub struct ParserInput<'a> {
    /// Already-normalised app_name from envelope (lowercased, trimmed).
    pub app_name: Option<&'a str>,
    /// Container name when source is Docker, otherwise None.
    pub container_name: Option<&'a str>,
    /// Free-form message body. For Docker rows this is the post-timestamp
    /// portion; for syslog rows it is whatever `syslog_loose` produced.
    pub message: &'a str,
    /// Raw line as received, for fallback regex on multi-segment events.
    pub raw: &'a str,
    /// Closed enum per `docs/contracts/source-kinds.md`. Wire-form strings
    /// are kebab-case: `syslog-udp`, `syslog-tcp`, `docker-stream`,
    /// `docker-event`, `otlp`, `adguard-api`, `unifi-api`, `agent`.
    pub source_kind: SourceKind,
    /// Existing severity (parsers MAY overwrite via ParserOutput.severity).
    pub severity: &'a str,
}

/// What a parser produces. Every field is optional — the dispatcher merges
/// non-None values onto the entry. `metadata` is shallow-merged into the
/// existing `metadata_json` blob under the namespace key the parser declares.
#[derive(Debug, Default)]
pub struct ParserOutput {
    pub http_status: Option<i32>,
    pub auth_outcome: Option<&'static str>, // "success" | "failure" | "denied" | "challenge"
    pub dns_blocked: Option<bool>,
    pub event_action: Option<String>,
    pub severity: Option<&'static str>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParserError {
    #[error("structural: {0}")]
    Structural(&'static str),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("regex no match: {0}")]
    NoMatch(&'static str),
}

pub trait Parser: Send + Sync + 'static {
    /// Stable name used in metadata under `metadata_json.parser`.
    fn name(&self) -> &'static str;
    /// Namespace key the parser's structured fields go under in metadata_json
    /// (e.g. "swag", "authelia"). Avoids key collisions across sources.
    fn namespace(&self) -> &'static str;
    fn parse(&self, input: &ParserInput<'_>) -> Result<ParserOutput, ParserError>;
}
```

Parsers are zero-state singletons (compiled regexes are `LazyLock<Regex>` at module scope, mirroring `enrichment.rs`). The dispatcher is constructed once at startup; the trait object lives behind `&'static dyn Parser` so dispatch is a cheap pointer chase.

---

## 4. Dispatch precedence rules

Two identifiers can point at a parser: `app_name` (the syslog APP-NAME or the Docker `app_name()` derived from container metadata in `docker_ingest::models::ContainerMeta::app_name`) and `container_name` (only populated for Docker sources). The matrix:

| `source_kind`               | Lookup key (in order)                                | Notes |
|-----------------------------|------------------------------------------------------|-------|
| `docker-stream`             | (1) `container_name` exact, (2) `app_name` exact, (3) `compose_service` exact | Docker is authoritative — operators rename containers, the parser must follow. |
| `docker-event`              | always `docker_event` parser, regardless of container | Lifecycle events are uniform; the per-container parser does not apply to "die" / "oom". The `docker_event` token here is the parser name (snake_case parser-id, see `parser-trait.rs::ParserId`), not the source_kind. |
| `syslog-udp` / `syslog-tcp` | `app_name` exact, case-insensitive                   | Most homelab syslog senders set APP-NAME; for the few that don't, no parser runs. UDP and TCP go through the same dispatch — transport distinction lives only in `source_kind` / `source_ip` for observability. |
| `adguard-api`               | always `adguard` parser                              | Bypasses dispatch — the API poller writes `app_name=adguard-query` and we route directly. |
| `unifi-api`                 | always `unifi` parser (epic C)                       | Same pattern as adguard-api — poller-tagged, single parser. |
| `otlp`                      | `app_name` (which is OTLP `service.name`)            | Same lookup as syslog. |
| `agent`                     | `app_name` exact                                     | Agent-streamed rows follow the same app_name dispatch as syslog. |

**Precedence within a source:** more specific key wins. For Docker, `container_name == "authelia-main"` beats a generic `app_name == "authelia"` only if both are registered — in practice, parsers register under canonical names (`authelia`, `swag`, `adguard`, `fail2ban`), and a `container_to_canonical` lookup folds operator-specific names (`authelia-main`, `authelia-prod`, `swag`) onto canonical keys. Unknown container/app → no parser runs, row is written unchanged, no error. The dispatcher emits a `tracing::debug!` once per never-seen `app_name` (bounded by an LRU of 256 entries) so operators can see what's slipping through without log spam.

**Conflicts:** if both `container_name` and `app_name` resolve to *different* parsers (rare — would require operator misconfiguration), `container_name` wins and a `parse_warning` field is added to metadata noting the disagreement. We do not error — silent degradation matches the "never drop data" rule.

---

## 5. Indexed-column migration

Migration 10, additive, runs on next startup:

```sql
-- Migration 10
ALTER TABLE logs ADD COLUMN http_status   INTEGER;
ALTER TABLE logs ADD COLUMN auth_outcome  TEXT;
ALTER TABLE logs ADD COLUMN dns_blocked   INTEGER;  -- 0/1; NULL = N/A
ALTER TABLE logs ADD COLUMN event_action  TEXT;
ALTER TABLE logs ADD COLUMN parse_error   TEXT;     -- see §6

CREATE INDEX IF NOT EXISTS idx_logs_http_status_time
    ON logs(http_status, timestamp) WHERE http_status IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_logs_auth_outcome_time
    ON logs(auth_outcome, timestamp) WHERE auth_outcome IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_logs_dns_blocked_time
    ON logs(dns_blocked, timestamp) WHERE dns_blocked IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_logs_event_action_time
    ON logs(event_action, timestamp) WHERE event_action IS NOT NULL;
INSERT INTO schema_migrations (version) VALUES (10);
```

All four indexes are **partial** (mirroring migration 8's AI-metadata pattern) — they index only non-NULL rows, keeping size proportional to recognised traffic rather than the whole 4.9M-row corpus. SQLite ≥ 3.37 `ALTER TABLE ADD COLUMN` on a populated table is metadata-only — no row rewrite, no write lock beyond the schema flip.

**FTS reindex:** not needed. The four new columns are not added to `logs_fts` (FTS5 still indexes `message` only). Operators who want to search on, say, `http_status:500` use a structured SQL filter via the MCP layer, not FTS. The existing `INSERT` trigger (`logs_ai`) remains unchanged.

**Index build cost on the production DB:** the four partial indexes are empty at migration time (every existing row has NULL for the new columns), so creation is `O(1)`. They only grow as new rows arrive post-migration.

---

## 6. Parse-error storage

**Decision: column on `logs`, not a separate table.**

Rationale:
- Most parser failures are sparse (recognised app_name reached the parser but a field was missing — say, a SWAG line where nginx didn't log upstream timing). One row, one error. A side table doubles the write count and adds a join for the common diagnostic query *"show me the message and what went wrong"*.
- The existing `transcript_parse_errors` table is the right model for *batch import* errors where the source isn't yet a row. Live ingest parses inline; the row exists; carry the error with it.
- Operators query this through the MCP layer; `SELECT message, parse_error FROM logs WHERE parse_error IS NOT NULL` is one statement.

Schema:

```sql
ALTER TABLE logs ADD COLUMN parse_error TEXT;
-- No index. parse_error is a diagnostic, not a filter dimension. If sampling
-- becomes a problem we add a partial index later.
```

Format: `"{parser_name}: {ParserError::Display}"`, truncated to 512 bytes. Mirrors the truncation discipline already in `parser.rs::truncate`. The parser's namespace metadata is still emitted (whatever fields it managed before failure), so partial extractions don't get thrown away.

---

## 7. Per-parser V1 specifications

### 7.1 `kernel` parser

**Triggers on:** `app_name == "kernel"`, `facility == "kern"`, or `source_kind == syslog` with a message starting with `Out of memory:`, `martian source`, `device .* entered`, etc. (cheap prefix check before regex).

**Real input examples:**
- `Out of memory: Killed process 2475067 (postgres) total-vm:2484556kB, anon-rss:143224kB, file-rss:0kB, shmem-rss:452kB, UID:1011 pgtables:588kB oom_score_adj:900`
- `eth0: link up, 1000Mbps, full-duplex, lpa 0x45E1`
- `eth0: link down`
- `br0: received packet on eth1 with own address as source address (addr:aa:bb:cc:dd:ee:ff, vlan:0)`

**Extracted fields → indexed columns:**
- `event_action = "oom_kill" | "link_up" | "link_down" | "mac_collision"`

**Extracted fields → metadata under `kernel.*`:**
- OOM: `pid: i64`, `comm: String`, `total_vm_kb: i64`, `anon_rss_kb: i64`, `uid: i32`, `oom_score_adj: i32`
- Link flap: `interface: String`, `state: "up" | "down"`, `speed_mbps: Option<i32>`
- MAC collision: `interface: String`, `colliding_mac: String`, `vlan: Option<i32>`

**Edge cases:** non-ASCII process names (UTF-8 stable in nom/regex), kernel messages with embedded newlines (we treat as single-line; rsyslog already framed it). Multi-line OOM dumps where only the "Killed process" line gets routed — fine, that's the line with the structured fields.

**Test fixtures:** five fixture files in `tests/fixtures/parsers/kernel/`: `oom_killed.txt`, `link_up.txt`, `link_down.txt`, `mac_collision.txt`, `unknown_kern.txt` (should NOT match — verifies prefix discrimination).

### 7.2 `docker_event` parser

**Triggers on:** `source_kind == "docker-event"` (kebab-case wire form, per `docs/contracts/source-kinds.md`). Bypasses app_name dispatch — Docker events are routed by source kind, not name.

**Real input examples (already shaped by `src/docker_ingest/parser.rs::docker_event_to_entry`):**
- Message: `docker container event: die container=postgres image=postgres:16 compose_project=stack compose_service=db exit_code=137`
- Raw: serialised `bollard::models::EventMessage`.

**Extracted fields → indexed columns:**
- `event_action = "create" | "start" | "stop" | "die" | "kill" | "oom" | "restart" | "rename" | "pause" | "unpause" | "destroy" | "health_status_healthy" | "health_status_unhealthy"`

**Metadata under `docker.*`:**
- `container_name: String`, `image: String`, `exit_code: Option<i32>`, `compose_project`, `compose_service`. The Docker ingester already writes most of these into `metadata_json`; the parser hoists `action` into the indexed column and normalises the verb.

**Edge cases:** `health_status: unhealthy` arrives with a colon and space — the existing `docker_event_source_action` already normalises to `health_status_unhealthy`; we adopt its output verbatim. `exit_code` may be on either `exitCode` or `exit_code` attribute (mixed across bollard/Docker versions); already handled.

**Test fixtures:** `tests/fixtures/parsers/docker_event/` — one JSON per Docker event verb, sourced by running `docker events --format '{{json .}}'` against a test rig and recording.

### 7.3 `authelia` parser

**Triggers on:** `app_name == "authelia"` OR `container_name` resolving to canonical `authelia`.

**Real input examples (JSON mode, the default on modern Authelia):**
- `{"level":"info","msg":"Authentication attempt successful","method":"POST","path":"/api/firstfactor","remote_ip":"100.0.0.1","time":"2026-05-15T03:46:03Z","username":"alice"}`
- `{"level":"error","msg":"Unsuccessful 1FA authentication attempt by user 'bob'","method":"POST","path":"/api/firstfactor","remote_ip":"203.0.113.7","time":"2026-05-15T03:46:11Z"}`
- `{"level":"info","msg":"Authentication attempt successful","path":"/api/secondfactor/totp","remote_ip":"100.0.0.1","time":"...","username":"alice"}`

**Indexed columns:**
- `auth_outcome = "success" | "failure" | "challenge"` (TOTP/Duo prompts → `challenge`; explicit `Unsuccessful` → `failure`; `Authentication attempt successful` → `success`).
- `severity` overwritten from JSON `level` (existing enrichment already does this — we move the logic into the parser).

**Metadata under `authelia.*`:**
- `username: String` (best effort — Authelia logs username inconsistently; extract from `username` field when present, fall back to extracting `'<name>'` from `msg`).
- `mfa_method: "totp" | "duo" | "webauthn" | "1fa"` derived from `path` (`/api/firstfactor` → `1fa`, `/api/secondfactor/totp` → `totp`, etc.).
- `src_ip: String` from `remote_ip`.
- `path: String`, `method: String`.

**Edge cases:** text-mode Authelia (older deployments) — fall through with `ParserError::Structural("not json")`. The row keeps its existing severity from the legacy `extract_authelia_level` regex path, which we keep as a fallback inside the parser. Username field absent on health checks — skip auth_outcome assignment (these aren't auth events).

**Test fixtures:** `tests/fixtures/parsers/authelia/`: `1fa_success.json`, `1fa_failure.json`, `totp_success.json`, `totp_failure.json`, `health_probe.json`, `text_mode_legacy.txt`.

### 7.4 `swag` / `nginx` parser

**Triggers on:** `app_name == "swag"` or container name resolving to `swag` / `nginx`. The default SWAG access log format is nginx's `combined` plus upstream timing — but operators routinely override; the parser handles both.

**Real input examples (combined + upstream):**
- `192.0.2.55 - - [15/May/2026:14:22:11 +0000] "GET /api/movies HTTP/2.0" 200 1432 "https://example.com/" "Mozilla/5.0" "203.0.113.7" 0.041`
- Combined-only: `192.0.2.55 - alice [15/May/2026:14:22:11 +0000] "POST /login HTTP/1.1" 401 87 "-" "curl/8.0"`
- Error log: `2026/05/15 14:22:11 [error] 17#17: *4321 upstream timed out (110: Connection timed out) while reading response header from upstream, client: 192.0.2.55, server: example.com, request: "GET / HTTP/2.0", upstream: "http://10.0.0.5:3000/"`

**Indexed columns:**
- `http_status: i32` (3-digit code).
- `event_action = "http_request"` for access lines, `"upstream_error"` for error.log lines that mention `upstream`.

**Metadata under `swag.*`:**
- Access: `method: String`, `path: String` (truncate at 2048 — URL injection vector if not bounded), `bytes_sent: i64`, `referrer: Option<String>`, `user_agent: Option<String>` (truncate at 512), `forwarded_for: Option<String>`, `client_ip: String`, `latency_ms: Option<i32>` (multiply `$request_time` by 1000), `upstream_latency_ms: Option<i32>`.
- Error: `upstream: String`, `error_class: String` (e.g. `timeout`, `connrefused`), `client: String`.

**Edge cases:** path contains a literal `"` — nginx escapes it as `\x22`; the parser uses a tolerant tokenizer (nom or a hand-rolled scanner), not a naive regex, to walk `"..."` segments. IPv6 in `$remote_addr` (square brackets). Lines wider than 8 KB (existing truncation at the writer handles this — we operate on whatever the writer kept).

**Test fixtures:** `tests/fixtures/parsers/swag/`: `access_combined.txt`, `access_combined_upstream.txt`, `access_ipv6.txt`, `access_escaped_quote.txt`, `error_upstream_timeout.txt`, `error_no_upstream.txt`.

### 7.5 `adguard` parser

**Triggers on:** `app_name == "adguard-query"` (set by either the container path's enrichment classifier OR the API poller). **Single code path** — see §8.

**Real input example (matches AdGuard's `querylog/json.go` shape):**
```json
{
  "T": "2026-05-15T14:22:11.123Z",
  "QH": "doubleclick.net",
  "QT": "A",
  "QC": "IN",
  "CP": "",
  "Answer": "...",
  "Result": {"IsFiltered": true, "Reason": "FilteredBlackList", "Rule": "||doubleclick.net^", "FilterID": 1},
  "Elapsed": "0.000234s",
  "Upstream": "https://dns.cloudflare.com/dns-query",
  "Client": "192.168.10.55"
}
```

**Indexed columns:**
- `dns_blocked: bool` (`Result.IsFiltered`, but only when `Reason` starts with `Filtered` — Rewrite is not a block).
- `event_action = "dns_query"`.

**Metadata under `adguard.*`:**
- `query: String` (`QH`), `qtype: String` (`QT`), `client: String`, `upstream: String`, `reason: String` (`Result.Reason`), `rule: Option<String>`, `elapsed_ms: f64` (parse `"0.000234s"` → 0.234), `cached: bool` (when present).

**Edge cases:** AdGuard versions earlier than v0.107 use lowercase keys; the parser tries PascalCase first, falls back to camelCase. Truncated rotated-out entries from the container log (AdGuard logrotates aggressively) — `serde_json::from_str` fails cleanly → `ParserError::Json`, row kept raw with `parse_error`.

**Test fixtures:** `tests/fixtures/parsers/adguard/`: `block.json`, `allow.json`, `rewrite.json`, `dnssec_failure.json`, `cached_hit.json`, `legacy_camelcase.json`, `truncated_invalid.txt`.

### 7.6 `fail2ban` parser

**Triggers on:** `app_name == "fail2ban"` or container name resolving to it. Also matches syslog rows from a non-containerised fail2ban (jail running on a bare-metal host pointing at our syslog port) via APP-NAME `fail2ban`.

**Real input examples (fail2ban ≥ 1.0):**
- `2026-05-15 14:22:11,037 fail2ban.actions [992]: NOTICE [sshd] Ban 203.0.113.7`
- `2026-05-15 14:22:26,259 fail2ban.actions [992]: NOTICE [sshd] Unban 203.0.113.7`
- `2026-05-15 14:31:14,420 fail2ban.filter [9599]: INFO [sshd] Found 203.0.113.7 - 2026-05-15 14:31:14`
- `2026-05-15 14:35:01,001 fail2ban.actions [992]: NOTICE [authelia] Restore Ban 198.51.100.4`

**Indexed columns:**
- `event_action = "ban" | "unban" | "found" | "restore_ban"`.

**Metadata under `fail2ban.*`:**
- `jail: String`, `banned_ip: String` (parse and store the bare IP — strip CIDR/port if present), `reason: Option<String>` (`Found` lines may carry a timestamp suffix).

**Edge cases:** jail names with hyphens (`apache-auth`) — fine, brackets delimit. Multi-IP bans (`Ban 1.2.3.4 5.6.7.8`) — rare; the parser stores the first IP in `banned_ip` and appends the full list under `fail2ban.all_ips`.

**Test fixtures:** `tests/fixtures/parsers/fail2ban/`: `ban.txt`, `unban.txt`, `found.txt`, `restore_ban.txt`, `multi_ip_ban.txt`, `error_line.txt`.

---

## 8. AdGuard dual-path note

The AdGuard parser must work identically against two inputs that arrive shaped differently:

1. **Container log path.** AdGuard writes JSON query records to stdout; the Docker socket-proxy ingester (`src/docker_ingest/parser.rs::log_output_to_entry`) splits off the timestamp prefix and hands us a `message` that is the raw JSON object.
2. **API poller path** (`syslog-mcp-awvr`). A separate task polls AdGuard's `/control/querylog` REST endpoint and writes one row per query result. The poller normalises its output so the row arrives with `app_name="adguard-query"` and `message` = the same JSON shape as the container log.

The parser sees the same `ParserInput.message` in both cases — there is only one code path. The two ingestion mechanisms differ in `source_kind` (`docker_stream` vs `adguard_api`) and `source_ip` (`docker://...` vs `adguard-api://<host>/<poller-id>`). The parser does not branch on those; it parses JSON, populates fields, returns. This means the API poller's responsibility is exactly: *produce a row that looks like a container row except for the source identifier.* Anything else is a contract violation and gets caught by the same `tests/fixtures/parsers/adguard/` golden fixtures — we add an `api_poller_normalised.json` fixture that is byte-identical to `block.json` (different envelope, same `message`) and assert parser output equality.

---

## 9. Privacy & redaction

No new secrets surface. The parser framework runs **after** the existing AI-message scrubber (`enrich_entry` → `scrub_secrets`), against the post-scrub `message`. The four indexed columns are typed — `http_status: i32`, `dns_blocked: bool`, `event_action` and `auth_outcome` come from a closed enum — so they cannot leak free text. The `metadata_json` namespace blobs (`authelia.*`, `swag.*`, etc.) carry the same fields the writer would have stored as raw substrings anyway; if scrubbing matters for, say, an Authelia username that's actually an email address, the existing redaction set already covers `password=`/`api_key=`/JWT patterns. Adding scrub regexes for new shapes (e.g. session cookies in nginx logs) is a separate concern handled inside `SECRET_PATTERNS` in `enrichment.rs` — not in the parsers.

The parser **must not** put the raw `message` back into `metadata_json` under any key. The original is already in `logs.raw`; duplicating it is wasted bytes and a re-exposure of pre-scrub content if the parser runs against a row that bypassed scrub somehow.

---

## 10. Performance budget

**Target overhead.** Existing batch flush at ~1500 rows / 5 s. Adding the parser dispatch must not move p99 flush latency by more than 10 % under steady state (i.e. < ~50 ms additional on a 500-row batch). Target per-row parser cost: **< 30 µs average, < 200 µs p99.**

**Batching.** Parser dispatch runs inside the existing `flush_batch` loop in `src/syslog/writer.rs:124-129` (right where `enrich_entry` is already invoked). No new spawn — staying on the async writer task keeps the spawn_blocking SQL window untouched. If a single parser ever exceeds 1 ms p99 (e.g. a future XML log), we move *that* parser into a `spawn_blocking` slice; the framework is opt-in for that.

**Hot-path discipline.**
- Every regex is `LazyLock<Regex>` at module scope (mirrors existing `enrichment.rs`).
- JSON parsers use `serde_json::from_str` against `&str`; no intermediate `String` allocations beyond the value tree.
- `ParserOutput::metadata` is `serde_json::Map`, merged into the entry's `metadata_json` via a single `serde_json::to_string` at the end, not value-by-value.
- The dispatch table is a `phf::Map<&'static str, &'static dyn Parser>` (or `HashMap` initialised once). No per-row allocation.

**Micro-benchmark plan.** Add `benches/parsers.rs` using `criterion`:
- One bench per parser, against the golden fixture set.
- One end-to-end bench: `enrich_batch_with_parsers(1000 entries)` against a mixed fixture set, compared to `enrich_batch_without_parsers(1000 entries)` (today's path).
- CI gate at warn level if mean cost regresses > 20 % vs `main`; fail at > 50 %.

---

## 11. Backwards compatibility

The 4.9M existing rows stay as-is. No backfill script in V1.

- All four new columns default to `NULL`. Queries that filter on them naturally return only post-migration rows; this is correct — we have no parser output for pre-migration data, and inferring it would be lossy guesswork.
- The MCP `cortex` tool's `search` action treats `http_status IS NULL` the same as "no filter" when the user did not pass `http_status`; we do not change query semantics.
- A future opt-in `cortex backfill --since <ts>` CLI subcommand can replay `raw` through the parser dispatcher and `UPDATE` rows in place. This is explicitly out of scope for V1 — we don't write to a live multi-million-row table without a clear operator ask.
- The `metadata_json` blob's shape changes only by adding new namespace keys (`authelia.*`, `swag.*`, etc.). Existing consumers that do `JSON_EXTRACT(metadata_json, '$.source_type')` keep working — we don't remove or rename anything.

---

## 12. Test plan

**Unit tests (per parser).** Sidecar `*_tests.rs` files following the existing convention. Each parser ships with:
- Happy-path: golden fixture → exact `ParserOutput` equality.
- Edge cases listed in §7 (one test per case).
- Malformed input → expected `ParserError` variant, not panic.
- "Wrong source" input (e.g. an Authelia line fed to the kernel parser) → returns `Err(NoMatch)`, never partial output.

**Integration tests.** New `tests/enrich_pipeline.rs`:
- Spin up an in-memory pool (`init_pool` against a tmp path).
- Push fixture rows through `enrich_entry` + parser dispatch.
- Insert via the real `insert_logs_batch`.
- Assert: column values match, `metadata_json` parses and contains expected namespace.
- One test per source that goes through both `source_kind="syslog-tcp"` and `source_kind="docker-stream"` to verify dispatch precedence.

**Golden-file fixtures.** Under `tests/fixtures/parsers/<source>/`. Each fixture is the literal `message` string (no envelope), so the fixture loader can drive every parser without knowing which one will match. A `metadata.json` per directory pins the expected `ParserOutput`.

**Smoke test.** Extend `scripts/smoke-test.sh` to assert the new columns appear non-NULL in a `SELECT COUNT(*) WHERE http_status IS NOT NULL` after a synthetic SWAG line is forwarded. Catches the "framework is wired but dispatch is broken" regression.

---

## 13. Open questions

1. **`container_to_canonical` map source.** Hard-code in source (one Rust constant), or expose via config? Recommend hard-code for V1 — operators will rename containers rarely, and a config knob adds a failure mode. Revisit if real-world deployments routinely fork the naming.
2. **`auth_outcome` cardinality.** Should `"locked"` (user temporarily disabled) be a fifth value, or fold under `failure`? Authelia distinguishes them; the MCP query layer might want to. Default proposal: keep four values in V1, add `locked` in V1.1 if operators ask.
3. **AdGuard `Reason` taxonomy.** Versions differ on whether a CNAME rewrite produces `Reason="Rewrite"` or `Reason="RewriteRule"`. Should `dns_blocked = false` always for any `Reason` starting with `Rewrite`, or do we want a third state (e.g. `dns_blocked = NULL` for rewrites)? Proposal: rewrites set `dns_blocked = NULL` so the dimension means strictly "filtered/blocked", and rewrite shows in `adguard.reason` only.
4. **Backfill CLI priority.** Operators on the 4.9M-row prod DB have legitimately useful historical data. Is a `cortex backfill` subcommand a V1.1 follow-up, or do we leave it indefinitely? Needs user confirmation before scheduling.
5. **OTLP path.** OTLP entries have richer attributes than syslog — should the parser framework also read `attributes` on the OTLP envelope (e.g. `http.status_code` is already there for OTel-instrumented services)? Proposal: yes, OTLP parsers are a thin shim that pulls from already-decoded attributes instead of re-parsing — but call this out as a follow-up epic rather than smuggling it into V1.
6. **Parser ordering / chaining.** Can two parsers ever run on the same row (e.g. SWAG access line that's also flagged by an HTTP-status-specific rule)? V1 says no — one parser per row, picked by dispatch. If a downstream rule needs cross-cutting logic, it consumes the indexed columns, not raw messages.
