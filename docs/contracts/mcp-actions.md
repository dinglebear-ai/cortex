---
title: "MCP Actions Contract — Superpowers Epics D / E / F"
created: 2026-05-16
updated: 2026-07-30
---

# MCP Actions Contract — Superpowers Epics D / E / F

**Status:** Historical epic design contract. The current production MCP surface
is `docs/contracts/mcp-actions-current.md`, with runtime source of truth in
`src/mcp/actions.rs::ACTION_SPECS`. Several RAG/Qdrant/LLM sections below were
design targets and are not active behavior today.

This file is a contract derived from:

- `docs/superpowers/specs/2026-05-16-probe-registry-design.md` (epic D — `cortex-fue9`)
- `docs/superpowers/specs/2026-05-16-digest-notifications-design.md` (epic E — `cortex-h6dg`)
- `docs/superpowers/specs/2026-05-16-rag-incidents-design.md` (epic F — `cortex-h6da`)
- `docs/superpowers/specs/2026-05-16-agent-mode-design.md` (epic A — `cortex-qgnx`) — for `status.agents` extension
- `docs/superpowers/specs/2026-05-16-api-pollers-design.md` (epic C — `cortex-awvr`) — for `status.pollers` extension
- `docs/superpowers/specs/2026-05-16-enrichment-framework-design.md` (epic B — `cortex-1wjr`) — for `search` filter additions

Changing it requires updating the spec first.

## 1. Purpose & Pinning

This contract enumerates every **new** MCP action introduced by the six 2026-05-16 superpowers epics, along with the additive changes to existing actions (`status`, `search`, `correlate`). Every action dispatches through `src/mcp/tools.rs` under the existing `cortex` super-tool via the `action` argument, and every JSON Schema follows the conventions already established in `src/mcp/schemas.rs` (draft 2020-12, additive-only properties, string enums for closed sets).

All actions return the standard envelope:

```json
{
  "ok": true,
  "result": { ... }
}
```

Or on error:

```json
{
  "ok": false,
  "error": { "code": "<machine-readable>", "message": "<human>", "data": {} }
}
```

`code` values used by the new actions: `agent_offline`, `agent_unavailable_capability`, `probe_timeout`, `invalid_params`, `not_found`, `internal`, `rate_limited`, `embed_pending`, `synthesis_unavailable`.

### 1.1 Agent JSON-RPC error → MCP error translation

When an MCP action dispatches to a per-host agent via the agent protocol, the agent's JSON-RPC error response (`agent-protocol.md` §5) is translated into the MCP error envelope above. This table is the contract.

| Agent JSON-RPC code (from agent-protocol.md §5) | Code name | MCP error code | MCP `error.message` template | Notes |
|---|---|---|---|---|
| -32600 | InvalidRequest | `invalid_params` | "Agent rejected request: <detail>" | Server-side bug in our RPC; logs at error level |
| -32601 | MethodNotFound | `internal` | "Agent does not support method <m>" | Indicates protocol-version drift |
| -32602 | InvalidParams | `invalid_params` | "Invalid probe parameters: <detail>" | Pass through param validation issues |
| -32000 | AuthFailed | `internal` | "Agent authentication state invalid" | Operator visibility — agent will reconnect |
| -32004 | UnknownHost | `not_found` | "Host <host_id> not found" | When MCP caller specifies a host_id that has no agents row |
| -32005 | TokenRotationRequired | `internal` | "Agent token rotation in progress" | Transient; retry after agent reconnects |
| -32010 | ProbeUnavailable | `agent_unavailable_capability` | "Agent does not advertise probe <name>" | Capability mismatch |
| -32011 | ProbeTimeout | `probe_timeout` | "Probe <name> on <host> timed out after <ms>ms" | Per-probe timeout from spec D |
| -32007 | BufferOverflow | `internal` | "Agent buffer overflow during operation" | Backpressure leaked through |
| -32008 | PayloadTooLarge | `invalid_params` | "Probe response exceeded size limit" | Spec D's 1 MiB cap |
| (connection lost) | — | `agent_offline` | "Agent <host_id> is not connected" | Wraps WS-level disconnect; not a JSON-RPC error itself |
| (no response within MCP action timeout) | — | `agent_offline` | "Agent <host_id> did not respond within <timeout>s" | MCP-level timeout overrides agent timeout |

MCP `error.data` MAY include the underlying JSON-RPC code in `agent_rpc_code` for diagnostic purposes; clients should not rely on it for control flow.

## 2. Action Index

| Action | Epic | Description | Breaking-change policy |
|---|---|---|---|
| `agent_status` | D (probe registry) | Per-host agent connection state, capabilities, schedule, last probe ts | Additive only |
| `alerts_ack` | E (digest+notifications) | Acknowledge or snooze an active alert by rule_id / fingerprint | Additive only |
| `alerts_active` | E | List all currently unacked alerts | Additive only |
| `ask_history` | F (RAG incidents) | NL question over incident corpus + AI sessions, with LLM synthesis | Additive only |
| `digest_preview` | E | Render the morning digest markdown without delivering | Additive only |
| `disk_blackholes` | D | Bounded sizing of high-churn build-cache paths | Additive only |
| `disk_usage` | D | Per-mountpoint capacity and inode usage (df-equivalent) | Additive only |
| `dns_status` | D | Per-resolver latency + failure tracking | Additive only |
| `mem_top` | D | Top-N processes by RSS | Additive only |
| `network_neigh` | D | ARP / neighbor table with collision detection | Additive only |
| `rules_fire_history` | E | Per-rule fire history (rule_id, fingerprint, when, log excerpt) | Additive only |
| `rules_list` | E | List configured alert rules with their last-fired stats | Additive only |
| `service_health` | D | Combined systemd-failed + docker-health view | Additive only |
| `mark_incident_resolved` | F | Tag an incident as resolved with optional session_id + notes; triggers re-embed | Additive only |
| `similar_incidents` | F | Rank past incidents structurally similar to a seed (log/window/text) | Additive only |
| `suggest_fix` | F | Surface resolution narrative from past resolved priors | Additive only |

16 new actions total.

## 3. Common Schemas

### 3.1 Freshness envelope (used by all probe-backed actions)

Every probe action wraps its payload with these three fields:

```json
{
  "fetched_at": "2026-05-16T14:00:01Z",
  "from_cache": false,
  "agent_online": true
}
```

When `agent_online` is `false`, the action returns the last `probe_results` row plus `last_seen_at`; if no row exists, response is `{"status": "no_data", "agent_online": false, "last_seen_at": "..."}`.

When the agent is online but does not advertise the probe in its capability handshake, response is `{"status": "unavailable", "reason": "...", "available_probes": [...]}` — no RPC is attempted.

### 3.2 Common probe params

```json
{
  "max_age_secs": { "type": "integer", "minimum": 0, "description": "Per-action default; override per call. If now - fetched_at > max_age_secs, pull fresh." },
  "force_refresh": { "type": "boolean", "default": false, "description": "Bypass cache; force RPC." }
}
```

---

## 4. Action Specifications

### agent_status

**Source:** epic D, `2026-05-16-probe-registry-design.md` §9.7.

**Purpose:** Report per-host agent connection state, advertised capabilities, current probe schedule, and last-seen timestamps. Always live — no caching. Omit `host` for fleet view.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "host": { "type": "string", "description": "Hostname filter; omit for full fleet." }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["agents"],
  "properties": {
    "agents": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["host_id", "hostname", "connection_state", "agent_version"],
        "properties": {
          "host_id": { "type": "string", "format": "uuid" },
          "hostname": { "type": "string" },
          "connection_state": { "type": "string", "enum": ["NeverConnected", "Active", "Disconnected", "Revoked"] },
          "agent_version": { "type": "string" },
          "kernel": { "type": "string" },
          "platform": { "type": "object" },
          "capabilities": {
            "type": "object",
            "properties": {
              "supported_probes": { "type": "array", "items": { "type": "string" } },
              "sources": { "type": "array", "items": { "type": "string" } }
            }
          },
          "schedule": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "probe": { "type": "string" },
                "interval_secs": { "type": "integer" }
              }
            }
          },
          "last_probe_at": { "type": "object", "additionalProperties": { "type": "string", "format": "date-time" } },
          "last_handshake": { "type": "string", "format": "date-time" },
          "last_seen": { "type": "string", "format": "date-time" },
          "last_error": { "type": ["string", "null"] }
        }
      }
    }
  }
}
```

**Example:**

Request:
```json
{ "action": "agent_status", "host": "devhost" }
```

Response:
```json
{
  "ok": true,
  "result": {
    "agents": [{
      "host_id": "9c9c2b6e-4f1a-7f0a-bbb1-3f9d70cd0e91",
      "hostname": "devhost",
      "connection_state": "Active",
      "agent_version": "0.1.0",
      "kernel": "6.8.0-40-generic",
      "capabilities": {
        "supported_probes": ["disk.usage", "disk.blackholes", "mem.top", "mem.pressure", "net.neigh", "systemd.failed", "docker.health"],
        "sources": ["journald", "docker", "file"]
      },
      "schedule": [
        { "probe": "disk.usage", "interval_secs": 60 },
        { "probe": "mem.pressure", "interval_secs": 30 }
      ],
      "last_probe_at": { "disk.usage": "2026-05-16T13:59:32Z", "mem.top": "2026-05-16T13:57:00Z" },
      "last_handshake": "2026-05-16T11:02:14Z",
      "last_seen": "2026-05-16T14:00:00Z",
      "last_error": null
    }]
  }
}
```

**Error modes:** `not_found` (host not present in `agents` table); `invalid_params` (malformed `host`).

**Freshness:** N/A — live read of WS connection state.

**Agent offline behavior:** still returns the row; `connection_state` becomes `Disconnected` and `last_seen` carries the most recent activity timestamp.

---

### alerts_ack

**Source:** epic E, `2026-05-16-digest-notifications-design.md` §9, §12.

**Purpose:** Acknowledge an active alert (`ack_at = now`) or snooze it (`snooze_until = now + duration`) without permanently clearing. Stops `repeat_until_ack` push escalation. References `alert_state.{rule_id, fingerprint}`.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["rule_id"],
  "properties": {
    "rule_id": { "type": "string" },
    "fingerprint": { "type": "string", "description": "Omit to ack all active alerts for this rule." },
    "snooze": { "type": "string", "description": "Duration like '2h', '30m'. When set, suppresses but does not clear." }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["acked", "snoozed"],
  "properties": {
    "acked": { "type": "integer", "minimum": 0 },
    "snoozed": { "type": "integer", "minimum": 0 }
  }
}
```

**Example:**

Request:
```json
{ "action": "alerts_ack", "rule_id": "container_die", "fingerprint": "plex@nashost" }
```

Response:
```json
{ "ok": true, "result": { "acked": 1, "snoozed": 0 } }
```

**Error modes:** `not_found` (no matching row in `alert_state`); `invalid_params` (unparseable snooze duration).

**Freshness:** N/A — writes through to SQLite synchronously.

---

### alerts_active

**Source:** epic E §12.

**Purpose:** List all currently unacked alerts (`alert_state` rows where `ack_at IS NULL`). Used by both the digest and the operator's "what's broken right now?" lookup.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "severity_min": { "type": "string", "enum": ["info", "warn", "critical"] },
    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100 }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["alerts"],
  "properties": {
    "alerts": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["rule_id", "fingerprint", "severity", "first_fired_at", "last_fired_at", "fire_count"],
        "properties": {
          "rule_id": { "type": "string" },
          "fingerprint": { "type": "string" },
          "severity": { "type": "string", "enum": ["warn", "critical"] },
          "first_fired_at": { "type": "string", "format": "date-time" },
          "last_fired_at": { "type": "string", "format": "date-time" },
          "fire_count": { "type": "integer", "minimum": 1 },
          "last_log_id": { "type": ["integer", "null"] },
          "snooze_until": { "type": ["string", "null"], "format": "date-time" }
        }
      }
    }
  }
}
```

**Example:**

```json
{ "action": "alerts_active", "severity_min": "warn" }
```
```json
{
  "ok": true,
  "result": {
    "alerts": [{
      "rule_id": "authelia_mfa_bruteforce",
      "fingerprint": "203.0.113.7",
      "severity": "critical",
      "first_fired_at": "2026-05-16T03:42:11Z",
      "last_fired_at": "2026-05-16T03:51:08Z",
      "fire_count": 14,
      "last_log_id": 9874213,
      "snooze_until": null
    }]
  }
}
```

**Error modes:** none expected beyond `internal`.

**Freshness:** real-time SQLite read.

---

### ask_history

**Source:** epic F §8.2.

**Purpose:** Natural-language question over the incident corpus + correlated AI sessions, answered by `axon ask` with synthesis. Used for "what causes qbittorrent to keep dying on edgehost?" style queries. Resolves to a Qdrant collection scoped to `incidents` (locked decision F-§13.1).

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["query"],
  "properties": {
    "query": { "type": "string", "minLength": 1 },
    "since": { "type": "string", "description": "Duration like '90d', '180d'. Default 180d." },
    "host_filter": { "type": "string" },
    "max_context_incidents": { "type": "integer", "minimum": 1, "maximum": 20, "default": 8 },
    "max_context_sessions": { "type": "integer", "minimum": 0, "maximum": 20, "default": 5 }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["answer", "citations"],
  "properties": {
    "answer": { "type": "string" },
    "citations": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["type", "score"],
        "properties": {
          "type": { "type": "string", "enum": ["incident", "session"] },
          "incident_id": { "type": "string" },
          "session_id": { "type": "string" },
          "score": { "type": "number" }
        }
      }
    },
    "axon_job_id": { "type": "string" },
    "diagnostics": { "type": "object" }
  }
}
```

**Example:**

```json
{ "action": "ask_history", "query": "what causes qbittorrent to keep dying on edgehost?", "host_filter": "edgehost" }
```
```json
{
  "ok": true,
  "result": {
    "answer": "qbittorrent on edgehost has died 14 times in the last 90 days, exit_code=137 in 12 of them (OOM-kill). The pattern correlates with sustained memory pressure from Plex transcoding...",
    "citations": [
      { "type": "incident", "incident_id": "inc_2026-04-12T07:18Z_edgehost_b7e2", "score": 0.81 },
      { "type": "session", "session_id": "sess_2026-04-12T07:30Z_jmagar_homelab-ops", "score": 0.65 }
    ],
    "axon_job_id": "job_8f3a",
    "diagnostics": { "axon_hits": 12, "rerank_ms": 18 }
  }
}
```

**Error modes:** `synthesis_unavailable` (axon unreachable; falls back to deterministic ranked hits with `synthesized: false`); `invalid_params`.

**Freshness:** drives off whatever is already embedded in Qdrant. Incidents from the last ~5 minutes may not yet be embedded (`embed_status=pending`); they are excluded from synthesis but mentioned in `diagnostics.skipped_pending`.

---

### digest_preview

**Source:** epic E §10, §12.

**Purpose:** Render the morning digest markdown **without delivering** it via apprise. Used for template tuning and for `cortex digest preview` CLI.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "for_date": { "type": "string", "format": "date", "description": "Local date in digest's configured timezone. Default = today." },
    "per_host": { "type": "boolean", "default": true }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["markdown", "rendered_at"],
  "properties": {
    "markdown": { "type": "string" },
    "rendered_at": { "type": "string", "format": "date-time" }
  }
}
```

**Example:**

```json
{ "action": "digest_preview", "for_date": "2026-05-16" }
```
```json
{
  "ok": true,
  "result": {
    "markdown": "# cortex digest — Fri 2026-05-16\n\n**TL;DR** — 0 critical · 2 warn · 14 errors...",
    "rendered_at": "2026-05-16T14:00:00Z"
  }
}
```

**Error modes:** `internal` (template render error); `invalid_params`.

**Freshness:** N/A — drives off live SQLite at call time.

**Locked ambiguity:** spec §14.5 asked whether `digest_preview` should also send. **Locked: return-only.** A future `dry_run: false` parameter may be added without breaking this contract.

---

### disk_blackholes

**Source:** epic D §8.2, §9.2.

**Purpose:** Bounded sizing of high-churn build-cache paths (`~/.cargo/target`, `node_modules`, `.venv`, `__pycache__`, Docker overlay). Directly addresses the "1TB NVMe full of `target/`" pain.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "top_n": { "type": "integer", "minimum": 1, "maximum": 200, "default": 25 },
    "paths_filter": { "type": "array", "items": { "type": "string" }, "description": "Optional substring filter over result paths." },
    "max_age_secs": { "type": "integer", "default": 3600 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["entries", "fetched_at", "from_cache", "agent_online"],
  "properties": {
    "entries": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["path", "total_bytes", "file_count", "completed"],
        "properties": {
          "path": { "type": "string" },
          "total_bytes": { "type": "integer", "minimum": 0 },
          "file_count": { "type": "integer", "minimum": 0 },
          "dir_count": { "type": "integer", "minimum": 0 },
          "newest_mtime": { "type": ["integer", "null"] },
          "oldest_mtime": { "type": ["integer", "null"] },
          "completed": { "type": "boolean" }
        }
      }
    },
    "truncated_paths": { "type": "array", "items": { "type": "string" } },
    "total_reclaim_estimate_bytes": { "type": "integer" },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" },
    "job_id": { "type": "string", "description": "Set when force_refresh=true kicks an in-flight walk and the cached result is older than 60s." }
  }
}
```

**Example:**

```json
{ "action": "disk_blackholes", "host": "edgehost", "top_n": 5 }
```
```json
{
  "ok": true,
  "result": {
    "entries": [
      { "path": "/home/jmagar/workspace/cortex/target", "total_bytes": 24863129088, "file_count": 412091, "dir_count": 21873, "newest_mtime": 1747410000, "oldest_mtime": 1746000000, "completed": true },
      { "path": "/home/jmagar/.cargo/registry", "total_bytes": 4112002432, "file_count": 188103, "dir_count": 9032, "completed": true }
    ],
    "truncated_paths": [],
    "total_reclaim_estimate_bytes": 28975131520,
    "fetched_at": "2026-05-16T13:00:01Z",
    "from_cache": true,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`, `agent_unavailable_capability` (Docker overlay read needs docker group), `probe_timeout`.

**Freshness:** default `max_age_secs=3600` (du walks are expensive). `force_refresh=true` is *expensive* — kicks a 60s walk. If a walk is already running, response carries a `job_id` and the latest cached entry set, allowing the caller to poll later.

**Locked ambiguity:** spec §13.1 left the per-user scoping question open. **Locked: scan every `/home/*` when agent runs as root, current user otherwise.** Operator can override via agent config.

**Cross-reference:** maps to `ProbeOutput::DiskBlackholes` in `docs/contracts/probe-trait.rs`.

---

### disk_usage

**Source:** epic D §8.1, §9.1.

**Purpose:** Per-mountpoint capacity and inode usage. Direct replacement for `df -h` over RPC.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "mount": { "type": "string", "description": "Optional exact-match mountpoint filter." },
    "max_age_secs": { "type": "integer", "default": 30 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["mounts", "fetched_at", "from_cache", "agent_online"],
  "properties": {
    "mounts": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["mountpoint", "fs_type", "size_bytes", "used_bytes", "avail_bytes", "used_pct"],
        "properties": {
          "mountpoint": { "type": "string" },
          "fs_type": { "type": "string" },
          "device": { "type": "string" },
          "size_bytes": { "type": "integer" },
          "used_bytes": { "type": "integer" },
          "avail_bytes": { "type": "integer" },
          "used_pct": { "type": "number" },
          "inodes_total": { "type": ["integer", "null"] },
          "inodes_used": { "type": ["integer", "null"] }
        }
      }
    },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" }
  }
}
```

**Example:**

```json
{ "action": "disk_usage", "host": "nashost" }
```
```json
{
  "ok": true,
  "result": {
    "mounts": [
      { "mountpoint": "/", "fs_type": "ext4", "device": "/dev/nvme0n1p2", "size_bytes": 1000000000000, "used_bytes": 380000000000, "avail_bytes": 620000000000, "used_pct": 38.0, "inodes_total": 65536000, "inodes_used": 421032 },
      { "mountpoint": "/var/log", "fs_type": "ext4", "device": "/dev/nvme0n1p2", "size_bytes": 1000000000000, "used_bytes": 380000000000, "avail_bytes": 620000000000, "used_pct": 38.0 }
    ],
    "fetched_at": "2026-05-16T14:00:00Z",
    "from_cache": false,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`, `probe_timeout`.

**Freshness:** 30s default.

**Cross-reference:** `ProbeOutput::DiskUsage` in `docs/contracts/probe-trait.rs`.

---

### dns_status

**Source:** epic D §8.6, §9.5.

**Purpose:** Per-resolver latency + failure tracking. Targets the "DNS flaky on WSL" pain.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "hostnames": { "type": "array", "items": { "type": "string" }, "description": "Optional override of the default probe list." },
    "resolvers": { "type": "array", "items": { "type": "string", "format": "ipv4" }, "description": "Optional override; empty = /etc/resolv.conf." },
    "max_age_secs": { "type": "integer", "default": 30 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["results", "summary", "fetched_at", "from_cache", "agent_online"],
  "properties": {
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["hostname", "resolver", "status"],
        "properties": {
          "hostname": { "type": "string" },
          "resolver": { "type": "string" },
          "status": { "type": "string", "enum": ["ok", "timeout", "nxdomain", "servfail", "error"] },
          "latency_ms": { "type": ["integer", "null"] },
          "answers": { "type": "array", "items": { "type": "string" } },
          "error": { "type": ["string", "null"] }
        }
      }
    },
    "summary": {
      "type": "object",
      "properties": {
        "failures": { "type": "integer" },
        "p50_ms": { "type": "number" },
        "p95_ms": { "type": "number" },
        "slow_resolvers": { "type": "array", "items": { "type": "string" } }
      }
    },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" }
  }
}
```

**Example:**

```json
{ "action": "dns_status", "host": "winhost-wsl" }
```
```json
{
  "ok": true,
  "result": {
    "results": [
      { "hostname": "google.com", "resolver": "172.31.0.1", "status": "ok", "latency_ms": 8, "answers": ["142.250.190.46"] },
      { "hostname": "github.com", "resolver": "172.31.0.1", "status": "timeout", "latency_ms": null, "error": "DEADLINE_EXCEEDED" }
    ],
    "summary": { "failures": 1, "p50_ms": 8.0, "p95_ms": 1000.0, "slow_resolvers": ["172.31.0.1"] },
    "fetched_at": "2026-05-16T14:00:00Z",
    "from_cache": false,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`, `agent_unavailable_capability` (host opted out of `net.dns_check`).

**Freshness:** 30s default.

**Cross-reference:** `ProbeOutput::NetDnsCheck`.

---

### mem_top

**Source:** epic D §8.3, §9.3.

**Purpose:** Top-N processes by RSS. Answers "who's OOMing devhost".

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "n": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
    "max_age_secs": { "type": "integer", "default": 5 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["total_mem_bytes", "avail_mem_bytes", "procs", "interpretation", "fetched_at", "from_cache", "agent_online"],
  "properties": {
    "total_mem_bytes": { "type": "integer" },
    "avail_mem_bytes": { "type": "integer" },
    "procs": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["pid", "comm", "rss_bytes"],
        "properties": {
          "pid": { "type": "integer" },
          "ppid": { "type": "integer" },
          "comm": { "type": "string" },
          "cmdline": { "type": "string", "maxLength": 256 },
          "uid": { "type": "integer" },
          "rss_bytes": { "type": "integer" },
          "vsize_bytes": { "type": "integer" },
          "cgroup": { "type": ["string", "null"] }
        }
      }
    },
    "interpretation": { "type": "string", "description": "Human-readable top-3 summary for fast model digestion." },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" }
  }
}
```

**Example:**

```json
{ "action": "mem_top", "host": "devhost", "n": 5 }
```
```json
{
  "ok": true,
  "result": {
    "total_mem_bytes": 34359738368,
    "avail_mem_bytes": 2147483648,
    "procs": [
      { "pid": 212312, "ppid": 1, "comm": "node", "cmdline": "node /usr/bin/claude-code", "uid": 1000, "rss_bytes": 15246798848, "vsize_bytes": 20000000000, "cgroup": "/user.slice/user-1000.slice" }
    ],
    "interpretation": "node:212312 RSS=14.2GB, claude:9871 RSS=8.4GB, plex:4221 RSS=2.1GB",
    "fetched_at": "2026-05-16T14:00:00Z",
    "from_cache": false,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`, `probe_timeout`.

**Freshness:** 5s default (RAM moves fast).

**Cross-reference:** `ProbeOutput::MemTop`.

---

### network_neigh

**Source:** epic D §8.5, §9.6.

**Purpose:** ARP/neighbor table with server-side collision detection. Catches MAC/IP collisions and stale entries.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "state_filter": { "type": "array", "items": { "type": "string", "enum": ["REACHABLE", "STALE", "DELAY", "FAILED", "PERMANENT", "NOARP"] } },
    "subnet_filter": { "type": "string", "description": "CIDR, e.g. 192.168.1.0/24." },
    "max_age_secs": { "type": "integer", "default": 60 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["entries", "collisions", "fetched_at", "from_cache", "agent_online"],
  "properties": {
    "entries": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["ip", "dev", "state"],
        "properties": {
          "ip": { "type": "string" },
          "mac": { "type": ["string", "null"] },
          "dev": { "type": "string" },
          "state": { "type": "string" },
          "age_secs": { "type": ["integer", "null"] }
        }
      }
    },
    "collisions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "kind": { "type": "string", "enum": ["mac_multiple_ips", "ip_multiple_macs"] },
          "key": { "type": "string" },
          "values": { "type": "array", "items": { "type": "string" } }
        }
      }
    },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" }
  }
}
```

**Example:**

```json
{ "action": "network_neigh", "host": "nashost", "state_filter": ["FAILED", "STALE"] }
```
```json
{
  "ok": true,
  "result": {
    "entries": [
      { "ip": "192.168.1.42", "mac": "00:11:22:33:44:55", "dev": "br0", "state": "STALE", "age_secs": 412 }
    ],
    "collisions": [],
    "fetched_at": "2026-05-16T14:00:00Z",
    "from_cache": true,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`, `probe_timeout`.

**Freshness:** 60s default.

**Cross-reference:** `ProbeOutput::NetNeigh`.

---

### rules_fire_history

**Source:** epic E §12.

**Purpose:** Per-rule fire history with log excerpts. Useful for "why did this rule fire so much last night?"

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "rule_id": { "type": "string", "description": "Omit for all rules." },
    "since": { "type": "string", "format": "date-time" },
    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["fires"],
  "properties": {
    "fires": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["rule_id", "fingerprint", "fired_at", "severity"],
        "properties": {
          "rule_id": { "type": "string" },
          "fingerprint": { "type": "string" },
          "fired_at": { "type": "string", "format": "date-time" },
          "severity": { "type": "string", "enum": ["warn", "critical"] },
          "log_id": { "type": ["integer", "null"] },
          "log_excerpt": { "type": ["string", "null"] }
        }
      }
    }
  }
}
```

**Example:**

```json
{ "action": "rules_fire_history", "rule_id": "container_die", "since": "2026-05-15T00:00:00Z" }
```
```json
{
  "ok": true,
  "result": {
    "fires": [
      { "rule_id": "container_die", "fingerprint": "plex@nashost", "fired_at": "2026-05-15T22:14:08Z", "severity": "critical", "log_id": 9852113, "log_excerpt": "container die abc123 (image=plex:latest, exitCode=137)" }
    ]
  }
}
```

**Error modes:** `not_found` (unknown `rule_id`); `invalid_params`.

**Freshness:** N/A — live SQLite read.

---

### rules_list

**Source:** epic E §12.

**Purpose:** List all configured alert rules with their definition + last-fired stats.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "enabled_only": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["rules"],
  "properties": {
    "rules": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "severity", "trigger_type"],
        "properties": {
          "id": { "type": "string" },
          "description": { "type": "string" },
          "severity": { "type": "string", "enum": ["info", "warn", "critical"] },
          "trigger_type": { "type": "string", "enum": ["instant", "count_over_window", "sustained", "absence"] },
          "last_fired_at": { "type": ["string", "null"], "format": "date-time" },
          "fire_count_24h": { "type": "integer", "minimum": 0 },
          "tag": { "type": ["string", "null"] }
        }
      }
    }
  }
}
```

**Example:**

```json
{ "action": "rules_list", "enabled_only": true }
```
```json
{
  "ok": true,
  "result": {
    "rules": [
      { "id": "container_die", "description": "Docker container exited unexpectedly", "severity": "critical", "trigger_type": "instant", "last_fired_at": "2026-05-15T22:14:08Z", "fire_count_24h": 3, "tag": "container" }
    ]
  }
}
```

**Error modes:** none expected.

**Freshness:** live config + SQLite.

---

### service_health

**Source:** epic D §9.4.

**Purpose:** Combined systemd-failed + docker-health view. Both sections report `available: bool` from the capability handshake.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["host"],
  "properties": {
    "host": { "type": "string" },
    "kind": { "type": "string", "enum": ["systemd", "docker", "all"], "default": "all" },
    "max_age_secs": { "type": "integer", "default": 30 },
    "force_refresh": { "type": "boolean", "default": false }
  },
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["fetched_at", "agent_online", "from_cache"],
  "properties": {
    "systemd": {
      "type": "object",
      "required": ["available"],
      "properties": {
        "available": { "type": "boolean" },
        "system_state": { "type": "string" },
        "units": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "name": { "type": "string" },
              "load_state": { "type": "string" },
              "active_state": { "type": "string" },
              "sub_state": { "type": "string" },
              "description": { "type": "string" },
              "n_restarts": { "type": ["integer", "null"] }
            }
          }
        }
      }
    },
    "docker": {
      "type": "object",
      "required": ["available"],
      "properties": {
        "available": { "type": "boolean" },
        "daemon_version": { "type": ["string", "null"] },
        "containers": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "id": { "type": "string" },
              "name": { "type": "string" },
              "image": { "type": "string" },
              "state": { "type": "string", "enum": ["running", "exited", "restarting", "paused", "dead"] },
              "health": { "type": ["string", "null"], "enum": ["healthy", "unhealthy", "starting", "none", null] },
              "restart_count": { "type": "integer" },
              "last_exit_code": { "type": ["integer", "null"] },
              "started_at": { "type": ["string", "null"], "format": "date-time" },
              "uptime_secs": { "type": ["integer", "null"] }
            }
          }
        }
      }
    },
    "fetched_at": { "type": "string", "format": "date-time" },
    "from_cache": { "type": "boolean" },
    "agent_online": { "type": "boolean" }
  }
}
```

**Example:**

```json
{ "action": "service_health", "host": "nashost", "kind": "all" }
```
```json
{
  "ok": true,
  "result": {
    "systemd": { "available": true, "system_state": "running", "units": [] },
    "docker": { "available": true, "daemon_version": "26.1.3", "containers": [
      { "id": "abc123", "name": "plex", "image": "plex:latest", "state": "running", "health": "healthy", "restart_count": 0, "started_at": "2026-05-15T10:00:00Z", "uptime_secs": 100800 }
    ]},
    "fetched_at": "2026-05-16T14:00:00Z",
    "from_cache": false,
    "agent_online": true
  }
}
```

**Error modes:** `agent_offline`; per-section `available: false` when the capability isn't advertised (systemd absent on a non-systemd WSL distro, or `/var/run/docker.sock` missing).

**Freshness:** 30s default for systemd; 10s for docker. The combined response uses the stricter of the two for its `from_cache` flag.

**Cross-reference:** `ProbeOutput::SystemdFailed` and `ProbeOutput::DockerHealth`.

---

### similar_incidents

**Source:** epic F §8.1.

**Purpose:** Rank past incidents structurally similar to a seed (log id, time window, or freeform text). Returns deterministic ranked hits — no LLM synthesis on this action.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "log_id": { "type": "integer", "minimum": 1 },
    "time_window": {
      "type": "object",
      "properties": {
        "host": { "type": "string" },
        "from": { "type": "string", "format": "date-time" },
        "to": { "type": "string", "format": "date-time" }
      }
    },
    "query": { "type": "string" },
    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 5 },
    "include_sessions": { "type": "boolean", "default": true },
    "since": { "type": "string", "description": "Duration like '90d'. Default 90d." },
    "host_filter": { "type": "string" },
    "min_score": { "type": "number", "default": 0.35 }
  },
  "oneOf": [
    { "required": ["log_id"] },
    { "required": ["time_window"] },
    { "required": ["query"] }
  ],
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["incidents"],
  "properties": {
    "query_card": { "type": "string" },
    "incidents": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["incident_id", "score", "hostname", "app_name", "source"],
        "properties": {
          "incident_id": { "type": "string" },
          "score": { "type": "number" },
          "hostname": { "type": "string" },
          "app_name": { "type": "string" },
          "source": { "type": "string" },
          "first_seen": { "type": "string", "format": "date-time" },
          "last_seen": { "type": "string", "format": "date-time" },
          "event_count": { "type": "integer" },
          "structured_fields": { "type": "object" },
          "sample_log_ids": { "type": "array", "items": { "type": "integer" } },
          "card_excerpt": { "type": "string" },
          "resolution_session_id": { "type": ["string", "null"] }
        }
      }
    },
    "sessions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "session_id": { "type": "string" },
          "project": { "type": "string" },
          "started_at": { "type": "string", "format": "date-time" },
          "snippet": { "type": "string" },
          "tool": { "type": "string", "enum": ["claude", "codex", "gemini"] }
        }
      }
    },
    "diagnostics": { "type": "object" }
  }
}
```

**Example:**

```json
{ "action": "similar_incidents", "query": "qbittorrent OOM", "limit": 3 }
```
```json
{
  "ok": true,
  "result": {
    "query_card": "synthesized query: qbittorrent OOM",
    "incidents": [
      {
        "incident_id": "inc_2026-04-12T07:18Z_edgehost_b7e2",
        "score": 0.81,
        "hostname": "edgehost",
        "app_name": "kernel",
        "source": "kernel.oom",
        "first_seen": "2026-04-12T07:18:14Z",
        "last_seen": "2026-04-12T07:18:34Z",
        "event_count": 4,
        "structured_fields": { "oom_victim_comm": "qbittorrent", "exit_code": 137 },
        "sample_log_ids": [4521000, 4521001],
        "card_excerpt": "host=edgehost app=kernel source=kernel.oom ...",
        "resolution_session_id": "sess_2026-04-12T07:30Z_jmagar"
      }
    ],
    "sessions": [],
    "diagnostics": { "axon_hits": 12, "mnemo_hits": 7, "rerank_ms": 14 }
  }
}
```

**Error modes:** `not_found` (`log_id` absent), `invalid_params` (none of `log_id`/`time_window`/`query` provided), `embed_pending` (the seed is a fresh incident whose card is still queued for embedding).

**Freshness:** drives off Qdrant + SQLite. New incidents become queryable ~5 minutes after they close (epic F §5).

---

### suggest_fix

**Source:** epic F §8.3.

**Purpose:** Surface the resolution narrative from prior AI sessions that closed the same shape of problem. Two-step retrieval per epic F §13.2 (axon query + client-side filter for `resolution_present`, then bespoke LLM synthesis).

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "incident_id": { "type": "string" },
    "log_id": { "type": "integer", "minimum": 1 },
    "query": { "type": "string" },
    "min_resolved_priors": { "type": "integer", "minimum": 0, "default": 1 }
  },
  "oneOf": [
    { "required": ["incident_id"] },
    { "required": ["log_id"] },
    { "required": ["query"] }
  ],
  "additionalProperties": false
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["synthesized"],
  "properties": {
    "synthesized": { "type": "boolean" },
    "suggestion": { "type": ["string", "null"] },
    "based_on": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "incident_id": { "type": "string" },
          "resolution_session_id": { "type": "string" },
          "session_excerpt": { "type": "string" }
        }
      }
    },
    "alternatives": { "type": "array" },
    "reason": { "type": "string", "description": "Populated when synthesized=false." }
  }
}
```

**Example:**

```json
{ "action": "suggest_fix", "incident_id": "inc_2026-05-15T03:17:12Z_jenny_a4f9" }
```
```json
{
  "ok": true,
  "result": {
    "synthesized": true,
    "suggestion": "Last time this happened (2026-04-12, jenny, plex OOM), you increased the docker memory limit to 4G and added oom_score_adj=-500. After that, no recurrences for 33 days.",
    "based_on": [
      { "incident_id": "inc_2026-04-12T07:18Z_jenny_a4f9", "resolution_session_id": "sess_2026-04-12T07:30Z_jmagar", "session_excerpt": "increased mem limit + score_adj" }
    ],
    "alternatives": []
  }
}
```

If no resolved priors exist:

```json
{
  "ok": true,
  "result": {
    "synthesized": false,
    "suggestion": null,
    "based_on": [],
    "alternatives": [],
    "reason": "no resolved priors"
  }
}
```

**Error modes:** `not_found`, `synthesis_unavailable`, `invalid_params`, `embed_pending`.

**Freshness:** synthesis uses live LLM call; retrieval uses Qdrant. Resolution edits via `mark_incident_resolved` propagate within one re-embed cycle (typically ~minutes).

---

### mark_incident_resolved

**Source:** epic F §8.4 (and §13.2 freshness note). Resolves audit bead `cortex-q22k` by promoting the spec-body reference into a first-class action.

**Purpose:** Operator (or an AI agent on the operator's behalf) marks an incident as fixed and optionally pins the AI session where the fix happened. Updates the `incidents` row (`resolution_session_id`, `resolution_notes`, `resolution_present = 1`), then enqueues a re-embed of the incident card so the new resolution boost (epic F §7.4) takes effect on future retrieval.

**Params:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["incident_id"],
  "additionalProperties": false,
  "properties": {
    "incident_id":   { "type": "string", "description": "Stable incident identifier (e.g. inc_2026-05-16T10:42Z_jenny_a4f9)." },
    "session_id":    { "type": ["string", "null"], "description": "AI session that captured the fix; surfaces in suggest_fix.based_on[].resolution_session_id." },
    "notes":         { "type": ["string", "null"], "maxLength": 4096, "description": "Free-form operator note. Embedded into the next card render." },
    "reopen":        { "type": "boolean", "default": false, "description": "When true, clears resolution_present + resolution_session_id + resolution_notes. Use when a 'fixed' incident reappears." }
  }
}
```

**Result:**

```json
{
  "type": "object",
  "required": ["ok"],
  "properties": {
    "ok":              { "type": "boolean" },
    "incident_id":     { "type": "string" },
    "resolution_present": { "type": "boolean" },
    "reembed_queued":  { "type": "boolean", "description": "True when the incident card is queued for re-embedding (see incident-card.md §7)." }
  }
}
```

**Example — mark resolved:**

```json
{ "action": "mark_incident_resolved",
  "incident_id": "inc_2026-05-16T10:42Z_jenny_a4f9",
  "session_id": "sess_2026-05-16T10:50Z_jmagar",
  "notes": "increased docker mem limit to 4G; added oom_score_adj=-500" }
```

```json
{ "ok": true,
  "result": { "ok": true,
              "incident_id": "inc_2026-05-16T10:42Z_jenny_a4f9",
              "resolution_present": true,
              "reembed_queued": true } }
```

**Example — reopen:**

```json
{ "action": "mark_incident_resolved",
  "incident_id": "inc_2026-05-16T10:42Z_jenny_a4f9",
  "reopen": true }
```

**Error modes:** `not_found` (no incidents row matches `incident_id`), `invalid_params` (e.g. both `reopen=true` AND non-null `session_id`).

**Freshness:** the row is updated synchronously; the re-embed runs in the next on-close finalizer pass (~5 min latency per epic F §5). The result's `reembed_queued: true` confirms the queue insertion, not the embed completion.

**Side effects:**
- Increments `incidents.schema_version_change_count` (per incident-card.md §7) implicit re-embed trigger.
- No notification fires (this action is the user closing the loop, not opening one).

---

## 5. Modifications to Existing Actions

### `status`

Existing `tool_get_status` (in `src/mcp/tools.rs` per spec C §10) gains two new top-level blocks. The existing keys (`status`, `db_ok`, `runtime_observability`, `otlp`) remain unchanged.

**`pollers` block** (per epic C §10):

```json
{
  "pollers": {
    "unifi-events": {
      "enabled": true,
      "healthy": true,
      "last_tick_at": "2026-05-16T14:00:00Z",
      "last_tick_age_seconds": 12,
      "lag_seconds": 1,
      "rows_emitted_total": 14283,
      "rows_dropped_total": 0,
      "consecutive_failures": 0,
      "saturated_last_tick": false,
      "last_error": null
    },
    "unifi-alarms":     { "...": "..." },
    "adguard-querylog": { "...": "..." }
  }
}
```

**`agents` block** (per epic A §9 + §16.7):

```json
{
  "agents": {
    "active": 5,
    "disconnected": 1,
    "revoked": 0,
    "fleet": [
      { "hostname": "devhost", "connection_state": "Active", "last_seen": "2026-05-16T14:00:00Z", "lag_seq": 0 },
      { "hostname": "edgehost", "connection_state": "Disconnected", "last_seen": "2026-05-16T11:42:11Z", "lag_seq": 1240 }
    ]
  }
}
```

Both blocks are additive and present only when the corresponding feature is enabled in config. Clients that didn't know about them continue to work.

### `search`

Filter params expand to include the four new indexed columns from epic B's enrichment framework (§5):

```json
{
  "http_status": { "type": "integer", "description": "Exact HTTP status filter (e.g. 401, 500)." },
  "http_status_class": { "type": "string", "enum": ["2xx", "3xx", "4xx", "5xx"], "description": "HTTP status class filter; computed server-side from http_status." },
  "auth_outcome": { "type": "string", "enum": ["success", "failure", "denied", "challenge"] },
  "dns_blocked": { "type": "boolean" },
  "event_action": { "type": "string", "description": "Closed enum across kernel/docker/authelia/fail2ban/adguard; see enrichment spec §7." }
}
```

`http_status IS NULL` is the implicit "no filter" semantic when no `http_status` is provided (epic B §11). All four columns use partial indexes (`WHERE <col> IS NOT NULL`), so filter performance is `O(non-null rows)`.

### `correlate`

No API change. `correlate` transparently benefits from epic B's new structured columns: the correlation join key set is extended internally to consider `(host, event_action)` and `(host, http_status)` proximity, but the request/response shapes are unchanged.

---

## 6. Versioning & Compatibility

- **Additive-only policy.** New optional params and new response fields may be added without bumping any version. Required params and existing enum members are immutable.
- **No version bump required.** This contract assumes the MCP tool dispatch table grows by 15 entries; the JSON Schema for the `cortex` tool's `action` enum is extended to include all 15. No clients need to opt in — they simply gain new actions they may invoke.
- **Breaking changes** (e.g. renaming an action, removing a field) require coordinated updates to: the source spec, this contract, `src/mcp/schemas.rs`, and `docs/mcp/SCHEMA.md`. Such a change is reflected by extending the action's name (`mem_top` → `mem_top_v2`) and keeping the old action live for one release cycle.
- **Enum extensions** (e.g. adding `locked` to `auth_outcome` per epic B §13.2) are additive but require client tolerance to unknown enum values; clients SHOULD treat unknown enum values as opaque strings rather than erroring.
- **Capability handshake** (epic D §3) protects probe-backed actions from clients calling probes that aren't compiled into the agent; this is enforced server-side via `agent_unavailable_capability`.

---

## 7. Cross-Contract Dependencies

- `disk_usage`, `disk_blackholes`, `mem_top`, `service_health`, `dns_status`, `network_neigh`, `agent_status` consume `ProbeOutput::*` variants from `docs/contracts/probe-trait.rs`.
- `alerts_active`, `alerts_ack`, `rules_list`, `rules_fire_history` consume the `alert_state` table defined in `docs/contracts/db-additions.sql`.
- `similar_incidents`, `ask_history`, `suggest_fix` consume the `incidents` table and the `IncidentCard` template defined in `docs/contracts/incident-card.md` and `docs/contracts/db-additions.sql`.
- `search`'s four new filters consume the `http_status`, `auth_outcome`, `dns_blocked`, `event_action` columns defined by epic B migration 10 (`docs/superpowers/specs/2026-05-16-enrichment-framework-design.md` §5).
- `status.pollers` block consumes the `poller_checkpoints` table defined in epic C §4.
- `status.agents` block consumes the `agents` table defined in epic A §9.
- See `docs/contracts/cli-surface.md` for the CLI equivalents that wrap these actions.
- See `docs/contracts/notification-rules.schema.json` for the validation contract on the TOML rules that feed `rules_list` and `alerts_active`.
