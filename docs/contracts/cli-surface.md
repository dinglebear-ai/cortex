---
title: "CLI Surface Contract — `cortex` Subcommands Added by Superpowers Epics"
created: 2026-05-16
updated: 2026-07-30
---

# CLI Surface Contract — `cortex` Subcommands Added by Superpowers Epics

This file is a contract derived from:

- `docs/superpowers/specs/2026-05-16-agent-mode-design.md` §16.7 (agent lifecycle CLI)
- `docs/superpowers/specs/2026-05-16-api-pollers-design.md` §13 + §14.4 (`pollers reset`, `pollers status`)
- `docs/superpowers/specs/2026-05-16-digest-notifications-design.md` §12 (digest preview, rules inspection — derived from the MCP actions)
- `docs/superpowers/specs/2026-05-16-probe-registry-design.md` (probe actions, surfaced via existing CLI wrappers; no new top-level commands except `agent_status` which lives under `cortex agent status` for the central operator view)

Changing it requires updating the spec first.

## 1. Purpose & Pinning

`src/cli.rs` already hosts a `clap`-derive standalone CLI for the `cortex` binary. This contract enumerates every new subcommand introduced by the six 2026-05-16 superpowers epics, in the style of that existing surface. All new commands share the existing conventions:

- `clap` derive parser (single binary, nested `Subcommand` enums)
- `--json` flag for machine-readable output; default is a compact human-readable table
- Exit codes: `0` = success, `1` = invocation/usage error, `2` = remote error (server unreachable or returned `ok: false`), `3` = state error (e.g. attempting to revoke an unknown host)
- Environment variables (`CORTEX_*`) take precedence over flags **only for credentials**; for non-credential flags, CLI args override env (per existing `src/config.rs` convention)
- Help text wraps at 100 columns and references the underlying MCP action when applicable

## 2. New Subcommand Groups

| Group | Side | Purpose |
|---|---|---|
| `cortex agent ...` | server + client | Agent lifecycle: server-side enrollment/revocation/tail; client-side daemon/enroll/status |
| `cortex pollers ...` | server | Poller management: reset cursors, dump current state |
| `cortex rules ...` | server | Alert rule introspection (list / test / history) |
| `cortex digest ...` | server | Digest preview / send-now |

All groups are gated on the corresponding feature being enabled in `config.toml`. Calling a gated subcommand on a disabled feature exits with code 1 and a clear message ("digest is disabled in config.toml; enable [notifications.digest] before previewing").

## 3. Server-side vs Client-side Table

This is the most important table for operators — it disambiguates where each command runs.

| Subcommand | Runs on | Talks to | Notes |
|---|---|---|---|
| `cortex agent list` | server (nashost) | local SQLite (`agents` table) | |
| `cortex agent issue` | server | local SQLite (insert pending row) | Prints one-time token to stdout. |
| `cortex agent revoke` | server | local SQLite + active WS connections | Kicks live conn if attached. |
| `cortex agent rotate` | server | local SQLite | Returns new token; old hash in grace for 300s. |
| `cortex agent tail` | server | local SQLite (`logs` view filtered to host) | Convenience wrapper over `cortex search host=...`. |
| `cortex agent status` (server form) | server | local SQLite + live WS state | Same data as MCP `agent_status` action; omit `--host` for fleet. |
| `cortex agent run` | client (per-host) | `wss://syslog.example.internal/ws/agent` | Long-lived daemon. |
| `cortex agent enroll` | client | server WS endpoint | Performs one-time-token handshake. |
| `cortex agent status` (client form) | client | local agent state + WS conn | Local-only; prints buffer depth, errors. |
| `cortex pollers reset` | server | local SQLite (`poller_checkpoints` row delete) | |
| `cortex pollers status` | server | local SQLite + in-memory `PollerObservability` | Same data exposed inside `cortex status` MCP action's `pollers` block. |
| `cortex rules list` | server | TOML config + SQLite (`alert_state` for last-fired) | Wraps MCP `rules_list`. |
| `cortex rules test` | server | TOML config + in-memory rule evaluator | Dry-runs a rule against fixture or sample line. |
| `cortex rules history` | server | SQLite (`alert_state` + audit) | Wraps MCP `rules_fire_history`. |
| `cortex digest preview` | server | local SQLite + Tera template | Wraps MCP `digest_preview`. |
| `cortex digest send-now` | server | local SQLite + apprise HTTP | Renders and POSTs immediately, ignoring schedule. |

Whether `cortex agent status` is the server or client form is disambiguated by the binary's runtime context: the client agent binary uses a distinct entrypoint (`bin/cortex-agent` symlinked to the same crate), or — equivalently — `cortex agent status` on a host that has no `/var/lib/syslog-agent/host_id` file produces the server-side view.

## 4. Subcommand Specifications

### syslog agent list

```
cortex agent list [--state <state>] [--json]
```

List all agents known to the server. Reads from the `agents` table populated by epic A's WS handshake flow.

- `--state <state>`: filter — one of `NeverConnected | Active | Disconnected | Revoked`. Optional.
- `--json`: emit JSON array; otherwise a fixed-width table with columns: `HOSTNAME`, `HOST_ID`, `STATE`, `VERSION`, `LAST_HANDSHAKE`, `LAST_SEEN`.

Exit codes: `0` success.

### syslog agent issue

```
cortex agent issue --host <host> [--ttl <duration>] [--json]
```

Generate a one-time enrollment token. Inserts a pending row in `agents` with `connection_state=NeverConnected` and the token's BLAKE3 hash. Prints the raw token **once** on stdout — operator must copy it now; the server does not retain it in plaintext.

- `--host <host>` (required): the canonical hostname this token will be bound to. Server checks that the hostname is not already `Active` with a different `host_id` (error code 3 if conflict).
- `--ttl <duration>` (optional, default `15m`): how long the one-time token is valid before its hash is purged from `agents` (a never-claimed token shouldn't sit on disk forever). Parsed by `humantime`.
- `--json`: emit `{"token": "...", "host_id": "...", "expires_at": "..."}`.

Exit codes: `0`, `3` (hostname conflict).

### syslog agent revoke

```
cortex agent revoke <host_id> [--reason <text>] [--confirm] [--json]
```

Revoke a host's tokens. Sets `connection_state=Revoked`, NULLs both `token_hash` columns, and — if the agent is currently `Active` — sends `agent.shutdown { reason: Revoked }` and closes the WS.

- `<host_id>` (positional, required): UUID v4 from `agents.host_id`.
- `--reason <text>` (optional): freeform, recorded in `last_disconnect_reason`.

Exit codes: `0`, `3` (unknown host_id).

### syslog agent rotate

```
cortex agent rotate <host_id> [--grace <duration>] [--json]
```

Issue a new token and keep the old hash valid for `--grace` (default `5m`) so the agent has a window to reconnect with the new token. Prints the new raw token once.

Exit codes: `0`, `3` (unknown host_id).

### syslog agent tail

```
cortex agent tail <host_id_or_hostname> [--follow] [--severity-min <sev>] [--lines <n>]
```

Server-side `tail -f` of recent log rows from that host. Convenience wrapper over `cortex search host=<h>` with `--follow` mapping to a 2-second polling loop. Honors the same flags as the existing `cortex tail` command.

- `--lines <n>` default 50.

Exit codes: `0`, `3` (host not found).

### syslog agent status (server form)

```
cortex agent status [--host <host>] [--json]
```

Per-host agent connection state, capabilities, schedule, last probe ts. Equivalent to MCP `agent_status`. Omit `--host` for fleet table.

Exit codes: `0`, `2` (server unreachable).

### syslog agent run

```
cortex agent run [--config <path>] [--token-file <path>]
```

Long-lived agent daemon (client side). The existing wire protocol entry point.

- `--config <path>`: agent config file; default `/etc/syslog-agent/config.toml`.
- `--token-file <path>`: override token location; default `/etc/syslog-agent/token`.

Exit codes: never returns on success. `1` on config error; `2` if token file missing or unreadable.

### syslog agent enroll

```
cortex agent enroll <token> [--server-url <url>] [--host-id <uuid>]
```

Accept a one-time token, perform handshake, store the rotated long-lived token in `~/.config/cortex/agent-token` (or `/etc/syslog-agent/token` if running as root).

- `<token>` (positional, required): the one-time string printed by `cortex agent issue`.
- `--server-url <url>` (optional): defaults to the value in config, or `wss://syslog.example.internal/ws/agent`.
- `--host-id <uuid>` (optional): supply an existing host_id; otherwise generate fresh and persist to `/var/lib/syslog-agent/host_id`.

Exit codes: `0`, `1` (bad flags), `2` (handshake failed — token invalid or expired), `3` (host_id already enrolled with a different token).

### syslog agent status (client form)

```
cortex agent status [--json]
```

Local-only: prints connection state, last successful push, buffer queue depth (redb), recent errors. No network calls beyond reading WS conn state.

Output (table form):

```
state          : Active
server         : wss://syslog.example.internal/ws/agent
session_id     : 9f3a-...
last_push_at   : 2026-05-16T13:59:58Z
acked_seq      : 14820112
buffer_entries : 4
buffer_bytes   : 1.2 KiB
recent_errors  : (none)
```

Exit codes: `0`, `1` (agent not running).

### syslog pollers reset

```
cortex pollers reset (--source <name> | --all) [--json]
```

Per epic C §14.4. Deletes the `(poller, instance)` row from `poller_checkpoints`; next tick treats the source as cold-start with the configured `backfill_hours`.

- `--source <name>`: one of `unifi` (covers both events + alarms), `adguard`. Exactly one of `--source` or `--all` is required.
- `--all`: reset every poller.

Exit codes: `0`, `1` (no `--source` or `--all` provided), `3` (unknown source name — matches the global "state error" definition).

### syslog pollers status

```
cortex pollers status [--json]
```

Per-source dump: `enabled`, `last_tick_at`, `cursor`, `lag_seconds`, `consecutive_failures`, `last_error`. Equivalent to the `pollers` block inside `cortex status` MCP action.

Exit codes: `0`, `2` (server unreachable).

### syslog rules list

```
cortex rules list [--enabled-only] [--json]
```

Wraps MCP `rules_list`. Table output: `ID`, `SEVERITY`, `TRIGGER`, `LAST_FIRED`, `24H_COUNT`.

Exit codes: `0`, `2`.

### syslog rules test

```
cortex rules test --rule <id> (--fixture <path> | --line <text>) [--json]
```

Dry-run a rule against a sample. Loads the rule from the live config, runs the evaluator against `--fixture` (JSONL of fake log entries) or a single `--line` (raw text), prints which events would fire and which would be deduped.

- `--rule <id>` (required): the rule ID from `[[notifications.rules]]`.
- `--fixture <path>`: JSONL file of log entries.
- `--line <text>`: a single log line; severity/tag inferred from active config defaults.

Exit codes: `0` (test completed; including "0 fires" outcome), `1` (rule not found, fixture unreadable).

### syslog rules history

```
cortex rules history [--rule <id>] [--since <duration>] [--limit <n>] [--json]
```

Wraps MCP `rules_fire_history`. Table output: `RULE`, `FINGERPRINT`, `FIRED_AT`, `SEVERITY`, `LOG_EXCERPT` (truncated to 80 chars).

Exit codes: `0`, `2`.

### syslog digest preview

```
cortex digest preview [--for-date <YYYY-MM-DD>] [--per-host] [--no-per-host] [--json]
```

Wraps MCP `digest_preview`. Prints rendered markdown to stdout. `--json` emits `{"markdown": "...", "rendered_at": "..."}`.

Exit codes: `0`, `2`.

### syslog digest send-now

```
cortex digest send-now [--for-date <YYYY-MM-DD>] [--per-host] [--json] [--confirm]
```

Renders the digest and POSTs it to apprise immediately, ignoring the schedule. Requires `--confirm` to actually fire (default behavior is to print what would be sent and exit 0); this prevents accidental delivery during dev.

Exit codes: `0`, `2` (apprise unreachable; HTTP error code in JSON output).

## 5. Conventions

- **`--json` flag**: most subcommands listed here accept `--json`; see individual entries. JSON output is a single object per command (never a stream). Errors in JSON mode still set the exit code and emit `{"ok": false, "error": {"code": "...", "message": "..."}}` on stdout.
- **Exit codes**: `0` = success, `1` = invocation/usage error (bad flag, conflict between flags, file unreadable), `2` = remote/server error (server unreachable, MCP `ok:false`, apprise non-2xx), `3` = state error (unknown host_id, hostname conflict, source unknown).
- **Env precedence**: `CORTEX_*` env vars override config.toml for credentials only; CLI flags override env for everything else. This matches the existing `src/config.rs:load_*` ordering.
- **Confirmation prompts**: destructive commands (`agent revoke`, `digest send-now`) accept `--confirm` to bypass; without it, the command prints a preview and exits 0.
- **Color**: respects `NO_COLOR` and `--no-color` per the rest of `src/cli.rs`. JSON output is never colorized.
- **Inherited flags**: `--config <path>`, `--server-url <url>`, `--db-path <path>` from the existing top-level CLI continue to apply to all new subcommands.

## 6. Cross-Contract Dependencies

- `cortex agent status` / `agent list` consume the `agents` table from epic A §9.
- `cortex pollers status` / `pollers reset` consume the `poller_checkpoints` table from epic C §4.
- `cortex rules list` / `rules history` / `digest preview` consume the rule TOML defined by `docs/contracts/notification-rules.schema.json` and the `alert_state` table from epic E §6.
- `cortex rules list` and the alert subcommands wrap MCP actions specified in `docs/contracts/mcp-actions.md` (`rules_list`, `rules_fire_history`, `alerts_active`, `alerts_ack`, `digest_preview`).
- `cortex agent run` (client-side) implements the wire protocol defined by `docs/contracts/agent-protocol.md`.

## 7. Locked Ambiguities

- **Probe-result CLI wrappers (epic D)**: spec D §9 only defines the MCP actions, leaving open whether each probe action should also get a `cortex probe disk-usage --host <h>` CLI wrapper. **Locked: no per-probe CLI in V1.** Operators script against `cortex mcp-call <action>` (existing) or use the chat-driven MCP tool. Adding per-probe CLI is purely additive and can land in a follow-up without breaking this contract.
- **`agent status` ambiguity**: a single subcommand serves both server-side and client-side contexts. **Locked: the running binary's environment disambiguates** — presence of `/var/lib/syslog-agent/host_id` plus a configured agent daemon implies client form; absence implies server form (queries DB). An explicit `--server` flag is reserved for future use if the heuristic ever fails.
- **`pollers reset` granularity**: spec C §14.4 lists `unifi`, `adguard`, `all` but is silent on whether `unifi` resets events + alarms together. **Locked: `--source unifi` resets BOTH `unifi-events` and `unifi-alarms` instances.** Use the lower-level SQL (`DELETE FROM poller_checkpoints WHERE poller = 'unifi-alarms'`) for surgical resets.
- **`digest send-now` confirm**: spec E doesn't specify whether `send-now` is dangerous. **Locked: requires `--confirm` to actually POST** — protects against an operator wiring it into a cron entry by mistake before they've validated the template.

## 8. Surface Parity Additions (2026-05-21)

These commands add CLI surface for actions that already existed in MCP and the
service layer. Pure plumbing — no new behaviour. Added by the surface-parity
plan (`docs/superpowers/plans/2026-05-21-surface-parity.md`).

| Subcommand | Side | Talks to | Mirrors MCP action |
|---|---|---|---|
| `cortex hosts sources [--limit N] [--offset N]` | both | local SQLite / REST `/api/source-ips` | `source_ips` |
| `cortex timeline [--bucket ...] [--group-by ...] [filters]` | both | local SQLite / REST `/api/timeline` | `timeline` |
| `cortex analysis patterns [filters] [--scan-limit N] [--top-n N]` | both | local SQLite / REST `/api/patterns` | `patterns` |
| `cortex stats ingestrate [--by-host]` | both | local SQLite / REST `/api/ingest-rate` | `ingest_rate` |
| `cortex alerts signatures [--include-acknowledged] [--limit N]` | both | local SQLite / REST `/api/errors/unaddressed` | `unaddressed_errors` |
| `cortex alerts signatures ack HASH [--notes TEXT]` | both | local SQLite / REST `/api/errors/ack` | `ack_error` |
| `cortex alerts signatures unack HASH [--reason TEXT]` | both | local SQLite / REST `/api/errors/unack` | `unack_error` |
| `cortex alerts notifications [--rule-id ID] [--since TIME] [--limit N]` | both | local SQLite / REST `/api/notifications/recent` | `notifications_recent` |
| `cortex alerts notifications test [--body TEXT]` | client only (`--http`) | REST `POST /api/notifications/test` | `notifications_test` |

Notes:

- `alerts notifications test` is HTTP-only because the apprise URL configuration is owned by
  the running server process. Local-mode CLI is missing the runtime config; we
  fail closed rather than send a notification from a process that has no
  apprise URLs.
- `sig ack`/`sig unack` set `actor = "cli"` in local mode and `actor = "api"`
  in HTTP mode (the REST handler hard-codes "api" since bearer auth does not
  carry per-user identity).
