# Digest + Push Notifications — Design Spec

**Epic**: `syslog-mcp-h6dg`
**Date**: 2026-05-16
**Status**: Draft
**Depends on**: `syslog-mcp-1wjr` (Enrichment Framework — parsed structured fields)

---

## 1. Goal & Non-Goals

### Goal

Turn the existing log firehose into two operator-grade signals:

1. **Real-time push alerts** on critical patterns (OOM kills, container die events, disk pressure, fail2ban bans, Authelia MFA brute-force, AdGuard upstream failures) delivered to the user's phone within seconds of the triggering event.
2. **Morning digest** — a single, scannable summary of overnight activity, categorized per-host with anomaly callouts vs a rolling baseline.

V1 success criterion: the user's three concrete pain points are eliminated.

- "Docker service has been down for hours and I didn't know" → critical alert within 60s of `die` event.
- "/var/log fills up and I find out too late" → warn alert at first disk-pressure log line.
- "Want a morning summary" → digest in gotify inbox by configured wakeup time, mobile-readable.

### Non-Goals (V1)

- Multiple HTTP transports inside syslog-mcp. We ship **one** HTTP target — apprise-api — and apprise handles fan-out to gotify/telegram/email/etc. on its side. A pluggable transport trait (to bypass apprise and hit ntfy/gotify directly) is V2.
- Graphical/chart attachments. Markdown only.
- ML anomaly detection. Threshold + simple rolling-baseline only.
- User-authored rules via UI/MCP write actions. Rules are TOML on disk; restart to reload (or SIGHUP later).
- Per-user/per-channel auth/ACLs inside syslog-mcp. Routing is delegated to apprise via `tag`.

---

## 2. Architecture

```text
                      Enrichment pipeline (Epic B)
                                  │
              parsed LogEntry { fields: {http_status,
                  auth_outcome, event_action, dns_blocked,
                  metadata_json}, severity, tag, host, ts }
                                  │
            ┌─────────────────────┴───────────────────────┐
            ▼                                             ▼
   db::ingest::insert_batch                  rule_eval::evaluate_stream
   (existing write path, unchanged)          (NEW: tap on writer tx)
                                                          │
                                          ┌───────────────┼───────────────┐
                                          ▼               ▼               ▼
                                   instant rules   windowed rules   threshold rules
                                   (single-event)  (sliding count)  (sustained)
                                          │               │               │
                                          └───────┬───────┴───────────────┘
                                                  ▼
                                          alert_state (SQLite)
                                          dedup / cooldown / fingerprint
                                                  │
                                                  ▼
                                         transport::apprise::send
                                                  │
                                                  ▼
                                            apprise-api HTTP
                                                  │
                                ┌─────────────────┼─────────────────┐
                                ▼                 ▼                 ▼
                              gotify           telegram          email/etc.
                                │
                                ▼
                          phone push (Android)

   ┌─────────────────────────────────────┐
   │ digest_scheduler (cron-in-tokio)    │
   │  fires daily at config local time   │──▶ digest::build (queries)
   └─────────────────────────────────────┘         │
                                                   ▼
                                            tera template render
                                                   │
                                                   ▼
                                         transport::apprise::send
                                          (type=info, tag=digest)
```

Both the rule evaluator and the digest scheduler are background tasks owned by `RuntimeCore` (alongside the existing purge/storage handles in `MaintenanceHandles`).

---

## 3. Transport Choice — Apprise (apprise-api)

**Decision: apprise-api** (https://github.com/caronc/apprise-api). Rationale:

- **Multi-transport fan-out for free.** Apprise is a notification *gateway*: one HTTP POST from us, N delivery backends on the apprise side (gotify, ntfy, telegram, discord, slack, email, pushover, …). User can change/add backends without touching syslog-mcp.
- Already running in the user's homelab — zero new infrastructure, behind the existing SWAG proxy.
- **Native markdown** via `format=markdown`. No backend-specific extras like gotify's `client::display`.
- **Tag-based routing** maps cleanly onto our per-rule/per-host fan-out: rules emit a `tag` value, apprise's stored config decides which backends subscribe to that tag (e.g. `tag=critical` → gotify + telegram; `tag=digest` → email only).
- Single HTTP target keeps our transport surface tiny: one `reqwest` client, one auth header, one URL.

**ntfy / direct gotify considered**: both viable, but committing to either locks us out of the other. Apprise puts the routing decision in user-space config where it belongs. Direct backends remain a V2 option behind a transport trait.

### Apprise API call shape

We use the **stored-config** endpoint (`/notify/{config_key}`) so the user's backend list lives on the apprise side, not in syslog-mcp config. Form-encoded body (apprise's native form; also accepts JSON — either works, we pick form for simpler `reqwest` ergonomics):

```http
POST {apprise_url}/notify/{config_key}
Content-Type: application/x-www-form-urlencoded
X-Apprise-Token: {token}      # optional, only if apprise-api is auth-protected

title=syslog-mcp%3A+container+die+%E2%80%94+plex+on+nashost
&body=%23%23+container+die%0A...markdown+body...
&type=failure
&format=markdown
&tag=critical%2Ccontainer%2Chost-nashost
```

Severity → apprise `type` mapping:

| Tier | apprise `type` | Repeat behavior (our side) |
|------|----------------|----------------------------|
| `info`     | `info`     | digest only — never pushed as a standalone alert |
| `warn`     | `warning`  | single push, no repeat |
| `critical` | `failure`  | first push + re-pushes every `repeat_until_ack` until acked; each repeat is just another POST with the same `type=failure` |

The escalation/repeat cadence is entirely a syslog-mcp concern — apprise sees N identical POSTs and forwards each. Whether a given backend "escalates" (e.g. gotify priority 10, telegram silent → loud) is the backend's UX, not ours.

Tag composition per push (comma-separated):
- always: severity tier (`info` / `warning` / `critical`) and `host-{host}`
- rule-defined: rule-level `tag` from TOML (e.g. `auth`, `container`, `dns`)
- digest pushes: `digest`

The user configures apprise stored config (YAML) to subscribe each backend URL to whichever tag set they want — fan-out is theirs to tune.

Auth: optional `X-Apprise-Token` header from `notifications.apprise.token` (env: `SYSLOG_MCP_APPRISE_TOKEN`). HTTP client is `reqwest` (already a transitive dep via OTLP).

---

## 4. Rule Format — TOML DSL

Rules live in the main `config.toml` under `[[notifications.rules]]`. Each rule has three sections: `match` (which logs), `trigger` (when to fire), `deliver` (how to notify).

```toml
# === Instant single-event rule ===
[[notifications.rules]]
id = "container_die"
description = "Docker container exited unexpectedly"
severity = "critical"

[notifications.rules.match]
tag = "docker"
field_eq = { event_action = "die" }
# exclude planned shutdowns
field_neq = { "metadata_json.exit_code" = "0" }

[notifications.rules.trigger]
type = "instant"

[notifications.rules.deliver]
dedup_window = "10m"
repeat_until_ack = "30m"      # critical-only
title = "container die: {{ host }} / {{ metadata_json.container_name }}"
tag = "container"             # appended to default tags on outgoing apprise push

# === Windowed count rule ===
[[notifications.rules]]
id = "authelia_mfa_bruteforce"
description = "5+ failed MFA attempts in 10 minutes from same source"
severity = "critical"

[notifications.rules.match]
tag = "authelia"
field_eq = { auth_outcome = "mfa_failed" }

[notifications.rules.trigger]
type = "count_over_window"
window = "10m"
threshold = 5
group_by = ["metadata_json.remote_ip"]

[notifications.rules.deliver]
dedup_window = "1h"

# === Substring rule (fallback for unparsed text) ===
[[notifications.rules]]
id = "varlog_full"
severity = "warn"
[notifications.rules.match]
message_contains = ["No space left on device", "filesystem full", "/var/log: 9"]
[notifications.rules.trigger]
type = "instant"
[notifications.rules.deliver]
dedup_window = "30m"
```

### Supported operators

| Operator | Semantics | Example |
|---|---|---|
| `tag` | exact match on enriched `tag` field | `tag = "authelia"` |
| `host` | exact match on `host` | `host = "nashost"` |
| `severity_min` | numeric ≥ | `severity_min = "warn"` |
| `field_eq` | map: field path → expected value (supports `metadata_json.x` dotted paths) | `{ http_status = "401" }` |
| `field_neq` | inverse of `field_eq` | |
| `field_in` | map: field → list of allowed | `{ event_action = ["die", "oom"] }` |
| `message_contains` | list of substrings, OR-joined, case-insensitive | |
| `message_regex` | single anchored regex, opt-in (compiled once at load) | |

### Trigger types

| Type | Semantics |
|---|---|
| `instant` | Fire on every match, subject to `dedup_window`. |
| `count_over_window` | Fire when ≥ `threshold` matches occur within `window`, grouped by `group_by` fields. Re-arms after `dedup_window`. |
| `sustained` | Fire when matches occur in ≥ N of last M evaluation buckets (e.g., disk pressure for 3 of last 5 minutes). |
| `absence` | Fire when an expected match has *not* occurred within `window` (e.g., no heartbeat from `host=jakey` in 15m). Deferred to V1.1 if scope is tight. |

### Real-world rule examples covering user pain

```toml
[[notifications.rules]] # OOM kill
id = "oom_kill"; severity = "critical"
match.tag = "kernel"; match.field_eq = { event_action = "oom_kill" }
trigger.type = "instant"; deliver.dedup_window = "5m"

[[notifications.rules]] # /var/log fill
id = "disk_pressure_varlog"; severity = "warn"
match.message_contains = ["No space left", "/var/log"]
trigger.type = "instant"; deliver.dedup_window = "30m"

[[notifications.rules]] # fail2ban ban
id = "fail2ban_ban"; severity = "warn"
match.tag = "fail2ban"; match.field_eq = { event_action = "ban" }
trigger.type = "instant"; deliver.dedup_window = "15m"

[[notifications.rules]] # AdGuard upstream failure
id = "adguard_upstream_fail"; severity = "warn"
match.tag = "adguard"; match.field_eq = { event_action = "upstream_error" }
trigger.type = "count_over_window"
trigger.window = "5m"; trigger.threshold = 10
deliver.dedup_window = "1h"

[[notifications.rules]] # Repeated 5xx from any host
id = "http_5xx_burst"; severity = "warn"
match.field_eq = { http_status_class = "5xx" }
trigger.type = "count_over_window"
trigger.window = "5m"; trigger.threshold = 25
trigger.group_by = ["host"]; deliver.dedup_window = "30m"

[[notifications.rules]] # Authelia MFA brute
id = "authelia_mfa_bruteforce"; severity = "critical"
match.tag = "authelia"; match.field_eq = { auth_outcome = "mfa_failed" }
trigger.type = "count_over_window"
trigger.window = "10m"; trigger.threshold = 5
trigger.group_by = ["metadata_json.remote_ip"]
deliver.dedup_window = "1h"; deliver.repeat_until_ack = "30m"
```

---

## 5. Rule Evaluation Model — Hybrid

**Decision: hybrid, biased to stream-driven for instant rules; periodic for windowed rules.**

- **`instant` rules**: evaluated **inline** on each parsed `LogEntry` as it passes through a `tokio::sync::broadcast` tap on the writer pipeline. The evaluator does a fast pre-filter on `(tag, severity)` indexed in memory before the field-level checks. Per-message overhead target: < 5 µs.
- **`count_over_window` / `sustained`**: evaluated by a periodic tokio task on a coarse cadence (default 30s). It runs a single SQL query per rule over the rule's window using existing FTS5 + structured indices. Group-by is delegated to SQLite (`GROUP BY` on the indexed columns). At 4.9M rows the cost is bounded because windows are short and queries can leverage the existing `(ts, tag)` index.
- **`absence`** (V1.1): same periodic task, inverted predicate.

### Why hybrid

Pure stream evaluation of windowed rules requires holding in-memory ring buffers per group key — fine for low-cardinality groups, ugly for `group_by = ["metadata_json.remote_ip"]`. SQLite already has the data, indexed, durable. A 30s polling cadence is fine for "5 MFA failures in 10 min" — users do not perceive a 30s delay on a 10-min window.

Pure periodic evaluation of `instant` rules is wasteful and adds latency the user would feel ("container died 25s ago, why no alert?"). Stream is correct here.

### Performance at 4.9M-row scale

- Stream path is O(rules × per-event-checks); at ~50 rules and ~5µs/check that's 250µs per log line. Current peak ingest is single-digit kLPS — well within budget.
- Periodic path: each windowed rule does one `WHERE received_at > now() - window AND app_name = ? AND <field predicates> GROUP BY ...`. The query uses the existing `idx_logs_app_name_received_at` composite index (`docs/contracts/current-schema.sql` §4.2, migration 3 — `(app_name, received_at)`), augmented by the new enrichment partial indices from Epic B (`idx_logs_http_status_time`, `idx_logs_auth_outcome_time`, `idx_logs_dns_blocked_time`, `idx_logs_event_action_time`). Targeting sub-100ms per query at 4.9M-row prod scale. Bounded to ~20 windowed rules in V1; total eval cost < 2s per 30s tick.

---

## 6. State Store — `alert_state` Table

New table, lives in the existing SQLite DB:

```sql
CREATE TABLE alert_state (
    rule_id        TEXT    NOT NULL,
    fingerprint    TEXT    NOT NULL,
    first_fired_at INTEGER NOT NULL,        -- unix ms
    last_fired_at  INTEGER NOT NULL,
    fire_count     INTEGER NOT NULL DEFAULT 1,
    last_log_id    INTEGER,                  -- FK to logs.id, the triggering row
    severity       TEXT    NOT NULL,
    ack_at         INTEGER,                  -- NULL while active
    ack_by         TEXT,                     -- "auto", "mcp:user", "timeout"
    snooze_until   INTEGER,                  -- NULL or unix ms
    PRIMARY KEY (rule_id, fingerprint)
);

CREATE INDEX idx_alert_state_active ON alert_state (ack_at) WHERE ack_at IS NULL;
CREATE INDEX idx_alert_state_rule_lastfired ON alert_state (rule_id, last_fired_at);
```

`fingerprint` = stable hash of `(group_by-projected fields)`. For `container_die` on `plex@nashost`, fingerprint is `plex@nashost`. For `oom_kill` with no group_by, fingerprint is `host`.

Lifecycle:

- First fire: `INSERT`.
- Subsequent fire within `dedup_window`: `UPDATE fire_count, last_fired_at`. No push.
- Fire outside `dedup_window`: push, update.
- `critical` with `repeat_until_ack`: a separate "escalator" task scans `WHERE ack_at IS NULL AND severity='critical' AND last_fired_at < now - repeat_window` and re-pushes.
- Ack: see §9.
- Stale clear: rows with `ack_at IS NOT NULL AND ack_at < now - 30d` are GC'd by the existing purge task.

---

## 7. Severity Tiers + Dedup Semantics

| Tier | Push? | Dedup behavior | Repeat? | Quiet-hours-suppressed? |
|---|---|---|---|---|
| `info`     | no, digest-only            | n/a                                | n/a                              | n/a (never pushes) |
| `warn`     | yes, single                | suppress identical fingerprint for `dedup_window` | no                              | yes |
| `critical` | yes                        | first fire always pushes; re-fires within `dedup_window` are coalesced into `fire_count` | yes, every `repeat_until_ack` until acked | **no** (overrides quiet hours) |

`dedup_window` defaults: `warn` → 30 min, `critical` → 10 min. Per-rule overridable.

---

## 8. Quiet Hours

```toml
[notifications.quiet_hours]
enabled = true
timezone = "America/New_York"
start = "22:30"
end = "07:00"
# applies to: info=N/A, warn=suppressed, critical=NOT suppressed
```

- Timezone resolved via `chrono-tz`. Stored as IANA name; never numeric offsets (DST-correct).
- During quiet hours, `warn` pushes are queued in `alert_state` (no `last_fired_at` push side-effect — the row records suppression with `ack_by = "quiet_hours"`) and **not** released afterward. They appear in the next digest. (Releasing a 40-deep queue at 07:00 is the worst possible UX.)
- `critical` always pushes — that's the contract.

---

## 9. Acknowledgment / Snooze

V1: **two ack mechanisms, both simple**:

1. **MCP action**: `syslog/alerts_ack` with `{rule_id, fingerprint}` or `{rule_id}` (all). Sets `ack_at = now`, `ack_by = "mcp:user"`. Stops repeats.
2. **Auto-clear**: a row with no new matches for `2 × dedup_window` is auto-acked with `ack_by = "timeout"`.

Snooze: `syslog/alerts_ack` with `snooze_until = "2h"` sets `snooze_until` without setting `ack_at`. Re-fires before `snooze_until` are suppressed and counted only.

Gotify reply / click-to-ack: **not in V1.** Gotify's webhook story for inbound is undercooked and would require exposing a callback URL. The MCP action is one line in Claude/Codex chat ("ack the plex alert").

---

## 10. Digest Design

### Schedule

```toml
[notifications.digest]
enabled = true
timezone = "America/New_York"
at = "07:15"
per_host = true              # also include an aggregate "fleet" section
include_ai_session_activity = true
```

Scheduler: `tokio::time::interval` driven, with a small `chrono_tz` step that computes "next 07:15 local" on each loop. Avoid `tokio_cron_scheduler` — overkill for one fire/day and pulls in extra deps.

### Sections (per digest)

1. **TL;DR** — counts: `N criticals, M warns, K errors, host-up=X/Y`
2. **Active alerts** — anything still unacked at digest time
3. **Top errors by host** — top 5 message clusters (existing `db::analytics::top_clusters`)
4. **Auth events** — successful logins, failed logins, MFA prompts, fail2ban bans
5. **DNS / AdGuard** — top blocked domains, upstream error count, query volume vs 7-day avg
6. **Container churn** — restarts, dies, OOMs (using `event_action` from Epic B)
7. **Disk pressure trend** — sparkline as ASCII bar per host based on `/var/log` warn lines
8. **Anomalies** — categories whose 24h count is > 2σ above the rolling 14-day baseline (computed via a single window-fn query)
9. **AI session activity** — uses existing `scanner.rs` data: sessions started, top tools, total messages

### Template engine

`tera` (already in dep ecosystem, simple, escapes for HTML if we ever switch). Single template, ships in `templates/digest.md.tera`. Markdown out.

### Delivery

One apprise POST per digest:

```http
POST {apprise_url}/notify/{config_key}
title=syslog-mcp digest — 2026-05-16
body=<rendered markdown>
type=info
format=markdown
tag=digest
```

The user configures their apprise stored config so that `tag=digest` routes to whichever backend(s) they want (e.g. email-only for the morning summary, while alert tags hit gotify + telegram). No SMTP code in syslog-mcp — apprise does email if the user asks it to.

### Sample digest

```markdown
# syslog-mcp digest — Fri 2026-05-16

**TL;DR** — 0 critical · 2 warn · 14 errors · hosts up 7/7 · 4.91M total rows · 92k overnight

## Active alerts
_None._ All clear since 02:14.

## Top errors by host
| host    | count | top cluster                                            |
|---------|------:|--------------------------------------------------------|
| nashost  |   312 | `plex: HTTP 500 from /library/sections (×287)`         |
| jakey   |    44 | `sshd: invalid user 'admin' from 192.0.2.14 (×39)`     |
| unraid  |    12 | `smbd: STATUS_LOGON_FAILURE (×9)`                      |

## Auth events
- 18 successful logins (Authelia)
- 3 failed MFA, all from `192.0.2.14` — flagged by fail2ban at 03:42, banned 1h
- 2 sudo invocations (jmagar@nashost)

## DNS / AdGuard
- 88,412 queries (+12% vs 7-day avg)
- 6,103 blocked (6.9% block rate, stable)
- Top blocked: `doubleclick.net` (412), `app-measurement.com` (308), `graph.facebook.com` (211)
- Upstream errors: 0

## Container churn
| event   | container        | host   | count |
|---------|------------------|--------|------:|
| restart | watchtower       | nashost | 1     |
| die     | radarr           | nashost | 1 (exit=0, planned via systemd) |

## Disk pressure
| host   | /var/log | trend |
|--------|---------:|-------|
| nashost |     38%  | `▁▁▂▂▂▁▁` |
| jakey  |     71%  | `▃▄▅▆▆▆▇` ← rising, investigate |
| unraid |     22%  | `▁▁▁▁▁▁▁` |

## Anomalies (vs 14-day baseline)
- `jakey` error rate +2.4σ — driven by sshd brute-force attempts (now fail2banned)

## AI session activity
- 3 Claude sessions (jmagar, ~14k tool calls)
- 1 Codex session (jmagar, debugging syslog-mcp)
```

---

## 11. Config Schema

```toml
[notifications]
enabled = true
transport = "apprise"            # only valid value in V1

[notifications.apprise]
url        = "https://apprise.example.invalid"
config_key = "syslog-mcp"        # named stored-config on apprise side
token      = "${SYSLOG_MCP_APPRISE_TOKEN}"  # optional X-Apprise-Token
default_tag = "syslog"           # appended to every push in addition to severity+host tags
timeout_ms = 5000
retry_attempts = 3

[notifications.quiet_hours]
enabled = true
timezone = "America/New_York"
start = "22:30"
end = "07:00"

[notifications.digest]
enabled = true
timezone = "America/New_York"
at = "07:15"
per_host = true
include_ai_session_activity = true
tag = "digest"                    # apprise tag for routing the digest

[[notifications.rules]]
# (see §4) — each rule may override `tag` to steer specific alerts
# to specific backends on the apprise side
```

Env equivalents (all `SYSLOG_MCP_` prefixed, flat):
`SYSLOG_MCP_NOTIFICATIONS_ENABLED`, `SYSLOG_MCP_APPRISE_URL`, `SYSLOG_MCP_APPRISE_CONFIG_KEY`, `SYSLOG_MCP_APPRISE_TOKEN`, `SYSLOG_MCP_APPRISE_DEFAULT_TAG`, `SYSLOG_MCP_DIGEST_AT`, `SYSLOG_MCP_DIGEST_TZ`, `SYSLOG_MCP_DIGEST_TAG`, `SYSLOG_MCP_QUIET_HOURS_START`, `SYSLOG_MCP_QUIET_HOURS_END`. Rules are TOML-only (env doesn't model arrays-of-tables sensibly).

---

## 12. MCP Actions

All added to `src/mcp/tools.rs` dispatch under the existing `cortex` tool.

| Action | Params | Response |
|---|---|---|
| `rules_list`         | `{ enabled_only?: bool }` | `[{ id, description, severity, trigger_type, last_fired_at?, fire_count_24h }]` |
| `rules_fire_history` | `{ rule_id?: string, since?: rfc3339, limit?: int }` | `[{ rule_id, fingerprint, fired_at, severity, log_id, log_excerpt }]` |
| `alerts_active`      | `{}` | `[{ rule_id, fingerprint, first_fired_at, last_fired_at, fire_count, severity }]` |
| `alerts_ack`         | `{ rule_id: string, fingerprint?: string, snooze?: duration }` | `{ acked: int, snoozed: int }` |
| `digest_preview`     | `{ for_date?: date, per_host?: bool }` | `{ markdown: string, rendered_at: rfc3339 }` |

`digest_preview` runs the same builder as the scheduled job — extremely useful for tweaking the template without waiting 24h.

---

## 13. Test Plan

### Unit

- `rule_eval::evaluate` — table-driven tests per operator (`field_eq`, `field_in`, regex bounds, `metadata_json` dotted path).
- `fingerprint` — stability across reorderings of group_by fields.
- `quiet_hours::is_quiet` — DST boundary cases (spring forward, fall back).
- `digest::build_section_*` — each section against a fixture DB snapshot.

### Integration

- **Fixture log stream**: a `tests/fixtures/notification_streams/` directory with replayable `.jsonl` event sequences. Each test loads a fixture, ingests it, asserts the alerts produced.
  - `oom_single.jsonl` → 1 critical
  - `mfa_burst_below_threshold.jsonl` → 0
  - `mfa_burst_above_threshold.jsonl` → 1 critical, fingerprinted on remote_ip
  - `container_die_exit0.jsonl` → 0 (excluded by `field_neq`)
  - `disk_full.jsonl` → 1 warn
- **Mock apprise-api server**: `wiremock`-based, asserts request shape — `POST /notify/{config_key}`, `X-Apprise-Token` header presence, form-encoded body containing `title`, `body`, `type` (one of `info|warning|failure`), `format=markdown`, and the expected `tag` set (severity tier + `host-{host}` + rule-defined tag).
- **Quiet hours**: fixture clock injection (existing `app::time::Clock` trait); send a `warn` during quiet hours, assert no HTTP call; send a `critical`, assert HTTP call.
- **Dedup**: send same trigger 5× in `dedup_window`, assert 1 HTTP call and `fire_count=5`.
- **Tag composition**: rule with `tag="container"` on host `nashost` at severity `critical` produces `tag=critical,host-nashost,container,syslog` (order-insensitive assertion).

### Digest snapshot tests

- `insta` snapshot of digest markdown against fixture DB. Re-run with `cargo insta review` on intentional changes.

### Live smoke

- Extend `scripts/smoke-test.sh` with `--notifications` flag: starts a local mock apprise-api (wiremock), drops a known-bad log line, asserts the mock received a `POST /notify/{config_key}` within 5s with `type=failure` and the expected tag set.

---

## 14. Open Questions

1. **Rule reload signaling**: V1 requires restart. Add `SIGHUP` reload in V1 or V1.1? Lean V1.1 — rules are stable enough that the friction is tolerable.
2. **Stored-config vs ad-hoc URLs**: V1 uses the stored-config endpoint (`/notify/{config_key}`) so backends live in apprise. Should we also support ad-hoc `urls=...` mode for one-off rules that need a specific Slack webhook? Not in V1 — adds config surface for an unproven need; users can just add a tagged backend in apprise.
3. **Multi-config fan-out granularity**: a single apprise stored config with tag-based routing is enough for V1. If the user later wants disjoint configs (e.g. one for prod hosts, one for lab hosts), we'd add `notifications.apprise.config_key` as a per-rule override. Flagged but not specced.
4. **Apprise markdown rendering coverage**: confirm during implementation that `format=markdown` produces sane output on each backend the user actually uses. Markdown-aware: telegram, discord, slack, email; degraded but readable: gotify (some clients render, some show raw); not supported: SMS/Pushover (apprise auto-strips). Mitigation: digests are markdown-only, single-target; alerts keep titles + bodies short enough to read raw.
5. **Should `digest_preview` actually deliver to apprise when called from MCP, or only return markdown?** Spec says return-only. A `dry_run: false` param could be added if useful.
6. **`absence` rules in V1?** Tempting (heartbeat alerts cover the "host went dark" case) but adds a second eval mode. Pencilled in for V1.1.
7. **Anomaly section in digest**: σ-threshold (currently 2.0) — empirically tune after first week of production runs.
8. **`metadata_json` dotted-path performance**: depends on Epic B's choice of JSON1 vs generated columns. If JSON1, the windowed rules grouping by `metadata_json.remote_ip` need an `expression index` to stay fast — flagged as an Epic-B-side requirement.
9. **Ack via apprise inbound** — apprise has no standard inbound webhook story, so this stays out-of-band. MCP-action ack is fine; user lives in Claude/Codex chat.
