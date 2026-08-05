---
title: "`metadata_json` Shape Contract"
created: 2026-05-16
updated: 2026-07-30
---

# `metadata_json` Shape Contract

**Status:** Contract — source of truth
**Date:** 2026-05-16
**Pinning header:**

> Contract for the shape of `logs.metadata_json` across every writer.
> Three concurrent writers: parsers (epic B `cortex-1wjr`), pollers
> (epic C `cortex-awvr`), and the agent (epic A `cortex-qgnx`).
> One reader namespace: the rule engine (epic E `cortex-h6dg`) field
> paths. Supersedes scattered `metadata.X.Y` references in spec B §3 / §7,
> spec C §4 / §5, spec A §4.4, spec E §4, and spec F §6.
> Changing this requires updating all dependents.

---

## 1. Background — the free-for-all

`metadata_json` is referenced as a structured object by every epic, but no
spec defines who owns which top-level key, what the merge order is, or how
the rule engine resolves a dotted path. The audit found:

| Surface | What it writes / reads |
|---|---|
| spec B §3 line 113 / §7 | Parsers namespace under `metadata.<parser>` (`authelia.*`, `swag.*`, `adguard.*`, `kernel.*`, `docker.*`, `fail2ban.*`). |
| spec C §4 line 200 / §5 line 328 | Pollers write `metadata_json.unifi` (UniFi poller) and `metadata_json.adguard` (AdGuard poller) BEFORE the parser dispatcher runs. |
| spec A §4 lines 185–196 (`AgentLogEntry.metadata`) | Agent supplies a free-form `metadata: Option<serde_json::Value>` blob, "stored in logs.metadata_json". |
| spec E §4 (`field_eq`, `field_in` operators) | Rule-engine field paths can address `metadata_json.<key>` with arbitrary depth, no defined namespace. |
| spec F §6 (Qdrant payload schema) | Incident finalizer reads `metadata_json.<source>` to build incident cards. |
| log-row-shape.md §5 lines 96–104 | Documents the namespace convention informally; depth ≤ 3, no raw-message copies, top-level `source_kind`. |

Three writers, one reader pattern, no merge order, no namespace registry,
no field-resolver semantics — this contract fills all four gaps.

---

## 2. Top-level namespace registry (closed)

Only the writers listed below may set the corresponding top-level keys.
Any other writer that touches one of these keys is a contract violation
caught at write time (see §5).

| Top-level key | Owner (writer) | Source spec | Notes |
|---|---|---|---|
| `kernel` | `kernel` parser (Epic B §7.1) | spec B | OOM, link-state, MAC collision details. |
| `docker` | `docker_event` parser (Epic B §7.2) AND `docker-stream` / `docker-event` ingest envelopes | spec B, log-row-shape.md §5 sketch | Both write — the ingester adds `container_name`, `image`, `compose_project`, `compose_service` BEFORE the parser; the parser adds `event_action`, `exit_code`. Both touch the same namespace but disjoint key sets; collisions are a bug. (Parser-id `docker_event` is snake_case per `parser-trait.rs::ParserId`; the source_kind values it consumes are kebab-case `docker-stream` / `docker-event`.) |
| `authelia` | `authelia` parser (Epic B §7.3) | spec B | Auth fields: `username`, `mfa_method`, `src_ip`, `path`, `method`. |
| `swag` | `swag` parser (Epic B §7.4) | spec B | nginx access + error fields. |
| `adguard` | `adguard` parser (Epic B §7.5) AND `adguard-api` poller (Epic C §5) | spec B, spec C | Dual writer — see §3 for the merge convention (`adguard.raw` poller, `adguard.parsed` parser). |
| `fail2ban` | `fail2ban` parser (Epic B §7.6) | spec B | `jail`, `banned_ip`, `reason`. |
| `unifi` | `unifi-api` poller (Epic C §4) | spec C | `key`, `_id`, `ip`, `mac`, `ap`, `ssid`, `user`. No parser in V1. If a future parser lands, it writes under `unifi.parsed`. |
| `otlp` | OTLP ingest (`src/otlp.rs`, existing) | existing code | Carries OpenTelemetry `attributes`, `resource`, `scope` — already populated by today's code. |
| `agent` | `cortex agent` (Epic A §4.1) | spec A | The agent's free-form `AgentLogEntry.metadata` blob is wrapped wholesale under this key by the server-side ingest handler. **The agent CANNOT write to any other namespace.** This boundary is what prevents a compromised agent from forging parser provenance. |
| `shell` | shell history import (`src/command_log.rs`) | command-log source | Shell name, user, history path, line number, duration, and timestamp quality for local shell-history rows. Command text is scrubbed before `message`/`raw`; unsanitized command text must not be stored here. |
| `agent_command` | agent command spool import (`src/command_log.rs`) | command-log source | Claude/agent command metadata: schema version, agent name, command surface, cwd, pid, exit status, duration, finish time, and session id. The wrapper does not capture env/stdout/stderr by default. |
| `parser` | parser dispatcher (Epic B §4) | spec B | Provenance for the parser that ran: `{"name": "authelia", "version": 1, "match_via": "container_name" \| "app_name" \| "compose_service"}`. Always present when a parser ran. |
| `source_kind` | enrichment pipeline | log-row-shape.md §5, source-kinds.md | Denormalised string copy of the row's `source_kind` (reconstructed from `source_ip` scheme). Always present. See source-kinds.md for the closed value set. |

**Closed set.** Adding a new top-level namespace is additive (§7) but
requires updating this registry and the writer's source spec. Any key
outside this set is rejected by the validator (§5).

### Reserved top-level keys NOT yet assigned

To leave room for future epics without re-shaping existing rows, the
following keys are **reserved** (writers MUST NOT use them):

- `probe` — reserved for Epic D probe output if a probe is ever folded
  into the log stream (currently probe output goes to `probe_results`
  table, not `logs`).
- `incident` — reserved for back-reference linking from a log row to its
  parent incident (Epic F may add this; currently the link is
  `incidents.sample_log_ids → logs.id`).
- `notify` — reserved for ack/snooze annotations from Epic E.

---

## 3. Dual-writer merge convention (adguard, docker, future shared namespaces)

Two writers may legitimately touch the same top-level namespace when the
data source is "polled API + parsed text of the same source" (adguard) or
"ingest envelope + lifecycle parser" (docker). The convention:

| Namespace | Envelope writer | Parser writer | Sub-key convention |
|---|---|---|---|
| `adguard` | `adguard-api` poller writes `metadata_json.adguard.raw` (full JSON record) and `metadata_json.adguard.client` (denormalised for filtering) | `adguard` parser writes `metadata_json.adguard.parsed` (typed fields: `query`, `qtype`, `upstream`, `reason`, `rule`, `elapsed_ms`, `cached`) | Disjoint: `raw`, `client`, `parsed`. |
| `docker` | `docker-stream` / `docker-event` ingester writes `metadata_json.docker.container_name`, `metadata_json.docker.image`, `metadata_json.docker.compose_project`, `metadata_json.docker.compose_service` | `docker_event` parser writes `metadata_json.docker.event_action`, `metadata_json.docker.exit_code` | Disjoint by key — both writers are flat under `docker.*`. The parser MUST NOT overwrite `container_name`/`image`/`compose_*` (they were authoritative from the ingester). |
| `unifi` | `unifi-api` poller writes flat under `metadata_json.unifi.*` | (no parser in V1) | If a future parser lands, it writes under `metadata_json.unifi.parsed`; the poller migrates to `metadata_json.unifi.raw` symmetric with adguard. |

**Rule:** Two writers may share a namespace **only if their key sets are
disjoint** (`adguard`: top-level keys `raw` / `client` / `parsed` vs
flat-key intersection is empty for `docker`). A new shared writer that
would collide on a key MUST namespace its keys under a new sub-key (e.g.
`adguard.parsed.*`).

---

## 4. Depth, size, and type discipline

These bounds are enforced by the writer-side validator at insert time;
violations result in the row being written with `parse_error = "<writer>:
metadata_json: <reason>"` and the offending namespace **stripped** from
the row. The row itself is never dropped.

### 4.1 Depth & size

| Limit | Value | Rationale |
|---|---|---|
| Max nesting depth | **4** (counting `metadata_json` as depth 0, top-level namespace as depth 1) | log-row-shape.md §5 capped at 3 — bumped to 4 here to accommodate `adguard.parsed.<field>` (depth 3) and an optional `adguard.parsed.rules[]` array of objects (depth 4). Beyond 4, `JSON_EXTRACT` queries get expensive and the FTS5 update trigger blows up. |
| Max single string-leaf | **4096 bytes (4 KiB)** | Per-leaf truncation cap. Parsers MUST truncate before insertion; long fields (SWAG `path`, `user_agent`) are pre-truncated per Epic B §7.4. |
| Max total `metadata_json` size | **65536 bytes (64 KiB)** | Serialised JSON length. Beyond this, the writer drops the offending parser/poller output entirely (the row writes with just envelope data + `parse_error`). |

### 4.2 Type discipline

Type fidelity matters because the rule-engine resolver (§6) is
type-aware: `field_eq` with a numeric expected value compares as
number; with a string, compares as string.

| Type | Allowed shapes | Notes |
|---|---|---|
| Integer | `i64` JSON number | `exit_code`, `http_status`, `oom_victim_pid`, `banned_ip_count`. **Do NOT stringify** — `"137"` is not equal to `137` under `field_eq`. |
| Float | `f64` JSON number | `elapsed_ms`, `latency_ms`. |
| Boolean | JSON `true` / `false` | `dns_blocked`, `cached`, `is_filtered`. |
| String | JSON string ≤ 4 KiB | Free-form text fields. Use sparingly. |
| Array of primitives | JSON array of int / float / bool / string | E.g. `unifi.aps: ["Office", "Garage"]`, `fail2ban.all_ips: ["1.2.3.4", "5.6.7.8"]`. |
| Object (nested) | JSON object up to depth 4 | E.g. `adguard.parsed.rule: {"filter_list_id": 1, "text": "\|\|x^"}` (depth 3). |

**Forbidden:** `null` values at any leaf. If a field is absent, omit the
key. The rule engine treats `missing key` and `present-but-null` as the
same (false predicate); the omit form is the canonical one and keeps the
JSON tight.

### 4.3 No raw message copies

Parsers and pollers MUST NOT write the row's `message` or `raw` strings
back into `metadata_json` under any key. The original is already in
`logs.raw`; duplicating it is wasted bytes and (worse) a re-exposure of
pre-scrub content if the parser runs against a row that bypassed the AI
scrubber somehow. Enforced by a writer-side string-equality check at
insert time.

---

## 5. Merge order (writer pipeline)

A row passes through the writer pipeline in a strictly defined order.
Writers MUST NOT touch namespaces they don't own; the server validates
ownership at write time.

```
INGRESS
   │
   ▼
 (1) Transport-specific shaping
     - syslog parse_loose → AgentLogEntry → LogBatchEntry envelope
     - docker_ingest → LogBatchEntry envelope (writes docker.* envelope keys)
     - otlp ingest → LogBatchEntry envelope (writes otlp.*)
     - agent ws handler → LogBatchEntry envelope (wraps AgentLogEntry.metadata
       wholesale under `agent`)
     - unifi-api poller → LogBatchEntry envelope (writes unifi.* flat)
     - adguard-api poller → LogBatchEntry envelope (writes adguard.raw +
       adguard.client)
   │
   ▼
 (2) AI-message scrubber (existing `scrub_secrets` in enrichment.rs)
     - Runs over `message` and any string-typed leaf in `metadata_json`
       reachable up to depth 4. Does not add/remove keys.
   │
   ▼
 (3) Parser dispatch (Epic B)
     - Selected by source_kind + app_name + container_name (see
       enrichment-framework spec §4 / source-kinds.md §5).
     - Writes the parser's namespace (`authelia`, `swag`, `kernel`,
       `docker_event` parser adds keys to existing `docker.*`, `adguard`
       parser adds `adguard.parsed`, `fail2ban`).
     - Writes `parser.*` provenance unconditionally.
   │
   ▼
 (4) Final enrichment
     - Writes `source_kind` (denormalised from URI scheme).
     - Computes the four indexed columns (`http_status`, `auth_outcome`,
       `dns_blocked`, `event_action`) from the parser output (NOT from
       `metadata_json` — the parser returns them directly via
       ParserOutput per parser-trait.rs).
   │
   ▼
 (5) Validator (write-time)
     - Checks every top-level key against the registry (§2).
     - Checks depth, size, type discipline (§4).
     - Strips offending namespaces, sets `parse_error`.
   │
   ▼
INSERT INTO logs
```

**Ownership enforcement.** At step (5), the validator walks the JSON
tree and for each top-level key checks the producing step (1)–(4). The
mapping is hardcoded:

| Top-level key | Allowed steps |
|---|---|
| `kernel`, `authelia`, `swag`, `fail2ban` | (3) parser dispatch only |
| `docker` | (1) docker ingester + (3) docker_event parser |
| `adguard` | (1) adguard-api poller + (3) adguard parser |
| `unifi`, `otlp` | (1) transport-specific shaping only |
| `agent` | (1) ws handler only |
| `shell`, `agent_command` | command-log import only |
| `parser` | (3) parser dispatch only |
| `source_kind` | (4) final enrichment only |

Violations are recorded as `parse_error = "ownership: <step> wrote
<namespace>"` and the namespace is stripped. This catches both bugs (a
poller accidentally writing under `authelia`) and threats (a compromised
agent attempting to forge parser provenance).

---

## 6. Rule-engine field-path resolver

The rule engine (Epic E, notification-rules.schema.json `field_eq`,
`field_neq`, `field_in`) addresses `metadata_json` via dotted paths. This
section is the normative resolver semantics.

### 6.1 Path syntax

A field path is a string with the form
`metadata_json.<seg1>.<seg2>...<segN>` where:

- The literal prefix `metadata_json.` MUST be present (without it, the
  path addresses a top-level column on `logs` such as `http_status`,
  `auth_outcome`, `severity`).
- Segments are separated by `.` (period).
- **Max 5 segments after `metadata_json.`** — matches the depth-4 cap
  in §4.1 plus one for the namespace.
- Segments MAY contain alphanumerics, underscore, and hyphen. Segments
  MAY NOT contain `.` (no escaping). If a key inside `metadata_json`
  contains a `.`, it is unaddressable from rules — by policy, writers
  MUST NOT produce such keys.
- Array indexing is **not supported** in V1. To filter on
  `adguard.parsed.rules[0].filter_list_id`, denormalise into a flat
  field at parser time (e.g. `adguard.parsed.first_rule_id`).

### 6.2 Resolution algorithm

```
fn resolve(row: &Row, path: &str) -> Option<JsonValue>:
    if path starts with "metadata_json.":
        let segments = path[14..].split('.').collect()
        if segments.len() > 4: return None
        let mut node = parse(row.metadata_json)?
        for seg in segments:
            node = node.get(seg)?     // None propagates → "not found"
        return Some(node)
    else:
        return row.column_value(path)   // ordinary column resolution
```

### 6.3 Coercion at the leaf

The rule operator (`field_eq` / `field_in` / `field_neq`) decides the
comparison type from the expected value's JSON type, NOT from the
leaf's type:

| Expected value | Leaf comparison |
|---|---|
| `"137"` (JSON string) | String equality on the leaf rendered as string. |
| `137` (JSON integer) | Numeric equality. If the leaf is a string `"137"`, the predicate is **false** — type discipline (§4.2) requires the leaf to be a JSON number. |
| `true` (JSON bool) | Boolean equality. `Some(1)` is not `true`. |
| Array (`field_in`) | Element-wise per-type comparison; the leaf's type must match the array's element type. |

### 6.4 Missing paths are always false

A predicate on a path that:

- Walks into a missing key at any depth,
- Hits a non-object node mid-walk (e.g. `metadata_json.authelia.username.foo`
  where `username` is a string), or
- Exceeds 5 segments after `metadata_json.`,

evaluates to **false**, NOT error. This matches the source-kinds.md §7
convention for unknown variants and prevents a typo in a rule from
turning off the whole rule engine. The resolver logs at `trace` level
on missing paths (bounded by an LRU of 256 path strings to avoid log
spam from a single broken rule).

---

## 7. Worked examples

### 7.1 Authelia failure row (parsed)

Source: docker-stream from an `authelia` container, JSON-mode log line:
`{"level":"error","msg":"Unsuccessful 1FA authentication attempt by user 'bob'","method":"POST","path":"/api/firstfactor","remote_ip":"203.0.113.7","time":"2026-05-15T03:46:11Z"}`.

```json
{
  "source_kind": "docker-stream",
  "docker": {
    "container_name": "authelia-main",
    "image": "authelia/authelia:4.38",
    "compose_project": "auth",
    "compose_service": "authelia"
  },
  "authelia": {
    "username": "bob",
    "mfa_method": "1fa",
    "src_ip": "203.0.113.7",
    "path": "/api/firstfactor",
    "method": "POST"
  },
  "parser": {
    "name": "authelia",
    "version": 1,
    "match_via": "container_name"
  }
}
```

Step (1) wrote `docker.*`; step (3) wrote `authelia.*` and `parser.*`;
step (4) wrote `source_kind`. The indexed column `auth_outcome=failure`
was set on the row from `ParserOutput.auth_outcome` — it is **not**
duplicated into `metadata_json` (use the column, not the JSON).

### 7.2 UniFi `EVT_LAN_IP_Conflict` (poller only)

Source: UniFi controller events poller. No parser in V1.

```json
{
  "source_kind": "unifi-api",
  "unifi": {
    "key": "EVT_LAN_IP_Conflict",
    "_id": "65d3f0a2b8c4d5e6f7a8b9c0",
    "ip": "192.168.1.42",
    "mac": "00:11:22:33:44:55",
    "subsystem": "lan",
    "site_id": "default",
    "datetime": "2026-05-15T20:09:27Z"
  }
}
```

Step (1) wrote `unifi.*`; step (4) wrote `source_kind`. No `parser` key
— no parser ran. A rule that wants to match this event uses
`field_eq = { "metadata_json.unifi.key" = "EVT_LAN_IP_Conflict" }`.

### 7.3 Agent log row with free-form metadata

Source: `cortex agent` on `dookie`, journald source, app `sshd`.
`AgentLogEntry.metadata` = `{"pid": 12345, "comm": "sshd", "user": "alice"}`.

```json
{
  "source_kind": "agent",
  "agent": {
    "pid": 12345,
    "comm": "sshd",
    "user": "alice"
  },
  "parser": {
    "name": "sshd",
    "version": 1,
    "match_via": "app_name"
  }
}
```

Step (1) (the WS handler) wrapped the agent's metadata blob wholesale
under `agent`. The agent CANNOT write to any other namespace — even if
an attacker forges
`AgentLogEntry.metadata = {"authelia": {"username": "root"}}`, the
result on the server is
`metadata_json.agent.authelia.username = "root"`, not
`metadata_json.authelia.username = "root"`. The forgery is impossible
because the wrapping happens server-side before the parser dispatcher
sees the row. A rule looking for Authelia failures
(`field_eq = { tag = "authelia", auth_outcome = "failure" }`) will not
match this row.

Step (3) ran the `sshd` parser (if one exists) and wrote `parser.*`.

---

## 8. Stability rules

| Change | Compatibility | Migration |
|---|---|---|
| **Adding a new top-level namespace** | Additive (backwards-compatible). Existing rules referencing other namespaces continue to work; rules referencing the new namespace begin to match. | None — no migration. Update §2 registry and the owning spec. |
| **Adding a leaf within an existing namespace** | Additive. Existing rules ignore new leaves; new rules can address them. Bump the parser's `parser.version` int (Epic B §7) so consumers can tell when a leaf became available. | None — old data lacks the leaf, rules referencing it evaluate false (§6.4). |
| **Renaming a top-level namespace** | **Breaking.** Rules referencing the old name silently turn always-false. | Major version bump on the contract. Coordinated rename of writer + every rule. |
| **Renaming a leaf within a namespace** | **Breaking** in effect. Bump the owning parser's `parser.version` AND emit a deprecation in CHANGELOG.md. The dispatcher MAY write both old and new names during a transition period (typically one minor release). |
| **Changing a leaf's type** (string → integer, etc.) | **Breaking.** Rules using `field_eq` with the old-type expected value evaluate false against new rows. Bump `parser.version` and update any operator-facing example rules. |

---

## 9. Self-check — every spec reference is covered

| Spec reference | Resolved by this contract |
|---|---|
| spec B §3 `metadata.<parser>` namespace convention | §2 registry — every Epic B parser has an owned namespace. |
| spec B §7 per-parser metadata.X.Y schemas | §2 — each namespace's sub-key set is the parser's concern; this contract bounds depth/size/types. |
| spec C §4 `metadata_json.unifi` | §2 — `unifi` registered to the unifi-api poller. |
| spec C §5 `metadata_json.adguard` (poller side) | §2, §3 — `adguard` is a dual-writer namespace; poller writes `adguard.raw` + `adguard.client`. |
| spec A §4.4 agent free-form `metadata` | §2 — `agent` wraps the blob; §5 enforces server-side wrapping. |
| spec E §4 `field_eq` / `field_in` on `metadata_json.<key>` | §6 resolver — fully specified, including missing-path semantics. |
| spec E §4 `field_eq = { http_status_class = "5xx" }` | NOT addressed here — `http_status_class` is a derived integer/string on the row, not a `metadata_json` field. The rule schema example in notification-rules.schema.json `examples[6]` uses this as a top-level field; treat it as a virtual column the rule engine synthesises from `http_status`. Outside this contract's scope. |
| spec F §6 incident-finalizer reads `metadata_json.<source>` | §2 — each Epic B source has a single owned namespace the finalizer can read deterministically. |
| log-row-shape.md §5 informal rules | Superseded by this contract. Cross-link from log-row-shape.md §5 to here. |
| `parser-trait.rs::Parser::namespace()` method | §2 — the returned namespace MUST be a member of the registry. |

---

## 10. Required downstream contract updates

| File | Required change |
|---|---|
| `docs/contracts/log-row-shape.md` §5 lines 96–124 | Replace the informal "namespace key conventions" with a one-line pointer to `metadata-json-shape.md` and remove the embedded schema sketch (now in §7.1 here). |
| `docs/contracts/parser-trait.rs` `Parser::namespace()` doc comment | Add: "Must be a registered namespace per docs/contracts/metadata-json-shape.md §2." |
| `docs/contracts/notification-rules.schema.json` `fieldMap` description (line 105) | Cross-link to `metadata-json-shape.md` §6 for resolver semantics. |
| `docs/contracts/incident-card.md` §4 / §5 (allowlist references) | Note that the allowlist applies to the OUTPUT (Qdrant payload), not to `metadata_json` itself — `metadata_json` is the input source. |
| `docs/superpowers/specs/2026-05-16-agent-mode-design.md` §4.1 / §10 (WS ingest handler) | Document the wholesale wrapping of `AgentLogEntry.metadata` under the `agent` top-level key. Currently the spec says "stored in logs.metadata_json" without specifying the wrap; the wrap is a security boundary. |
| `src/db/ingest.rs` (insert path) | Implement the writer-side validator (§5 step 5). Today there is no validator; this contract requires one to land alongside enrichment migration 10. |

## Agent Docker identity namespace (`agent_docker`)

The host-local agent forwards Docker container logs as syslog with a
structured identity prefix; receiver enrichment merges it into
`metadata_json`:

```json
{
  "source_kind": "agent-docker",
  "agent_docker": {
    "host": "tootie",
    "container_id": "abcdef1234567890",
    "container_name": "plex",
    "compose_project": "plex",
    "compose_service": "plex",
    "image": "lscr.io/linuxserver/plex:latest",
    "stream": "stdout"
  }
}
```

Required fields: `host`, `container_id`, `container_name`, `stream`.
Optional fields: `compose_project`, `compose_service`, `image`. The
`agent_docker` namespace has a single writer (receiver enrichment from the
agent prefix); the legacy central-pull `docker.*` namespace is separate and
is not resolver proof. Canonical resolver proof must use `agent-docker`
structured metadata; `docker://` and `docker-event://` rows are not proof
for the resolver-backed graph contract.

**Trust boundary:** the marker travels in the unauthenticated syslog
message body, so its payload is sender-controlled — without the source
gate, any port-1514 sender can forge agent-docker identity (same class as
the CEF `UNIFIdeviceName` gotcha). Receiver enrichment therefore scopes the
merge: only the `agent_docker` object is taken from the payload, existing
`metadata_json` keys (including a pre-existing `agent_docker`) are never
overwritten, and `source_kind` is set from the receiver constant, not the
payload. Operators SHOULD configure `agent_docker_source_prefixes`
(`CORTEX_AGENT_DOCKER_SOURCE_PREFIXES`, comma-separated) to restrict
extraction to hosts running the cortex agent; unset keeps
extract-from-anywhere compatibility.
