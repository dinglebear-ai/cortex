---
title: "Severity Vocabulary Mappings Contract"
created: 2026-05-16
updated: 2026-07-30
---

# Severity Vocabulary Mappings Contract

**Status:** Contract — source of truth
**Date:** 2026-05-16
**Pinning header:**

> Contract derived from cross-cutting audit; three independent severity
> vocabularies overlap across the corpus without a normative mapping. This
> contract defines the canonical mapping function used by every rule
> matcher, every MCP filter that mentions "severity_min", and every alert
> tier classifier. Supersedes implicit conversions in
> `docs/superpowers/specs/2026-05-16-digest-notifications-design.md` §4 / §7,
> `docs/contracts/mcp-actions.md::alerts_active`, and `notification-rules.schema.json::severity_min`.
> Changing this requires updating all dependents.

---

## 1. Syslog severities (8 levels)

The canonical UNIX syslog severity scale (RFC 5424 §6.2.1), ordered by
numeric value. Reference implementation:
`src/db/queries.rs::SEVERITY_LEVELS` (line 880) and the parser at
`src/syslog/parser.rs::164`.

| Code | Name | Conventional usage | Example log lines |
|---|---|---|---|
| 0 | `emerg` | System is unusable. | `kernel: panic - not syncing: Attempted to kill init!` |
| 1 | `alert` | Action must be taken immediately. | `mdadm: DegradedArray event on /dev/md0` |
| 2 | `crit` | Critical conditions. | `kernel: hardware error detected on CPU 3` |
| 3 | `err` | Error conditions. | `dockerd: failed to start container plex: …`, `kernel: Out of memory: Killed process 18422 (plex)` |
| 4 | `warning` | Warning conditions. | `sshd: invalid user 'admin' from 192.0.2.14`, `swag: upstream timed out` |
| 5 | `notice` | Normal but significant. | `systemd: Started plex.service`, `unifi: EVT_WU_Connected` |
| 6 | `info` | Informational. | `cron: starting daily backup`, `swag: 200 GET /` |
| 7 | `debug` | Debug-level messages. | `app: entering function X` |

**Where this vocabulary is used:**

- `logs.severity` column (every row).
- Parser output: `ParserOutput::severity` (`parser-trait.rs::158`).
- MCP filter parameters that take a syslog severity: `errors` action
  threshold, parser dispatch.
- Incident card `severity_max` field (`incident-card.md` §2 placeholder
  list).
- Default severity floor for the `errors` MCP action (everything
  `err` or worse).

**Closed set.** All 8 values are present in `SEVERITY_LEVELS` in code
order; no extensions. Parser output's `severity: Option<&'static str>`
must be one of these 8 string forms.

---

## 2. `auth_outcome` enum (4 values)

A domain-specific enum unrelated to severity. Source:
`docs/contracts/parser-trait.rs::AuthOutcome` (lines 111–118),
`#[serde(rename_all = "lowercase")]`.

| Value | Semantics | Sample log line |
|---|---|---|
| `success` | Authentication completed successfully. The user is now authenticated. | Authelia: `{"level":"info","msg":"Authentication attempt successful","method":"POST","path":"/api/firstfactor","username":"alice"}` → `auth_outcome=success`. SWAG: `200` on a `/login` endpoint when the response indicates success → `auth_outcome=success`. |
| `failure` | Credentials were provided and rejected. Distinct from `denied` — the user attempted to authenticate and got it wrong. | Authelia: `{"level":"error","msg":"Unsuccessful 1FA authentication attempt by user 'bob'", …}` → `auth_outcome=failure`. SWAG: `401` on `/login` → `auth_outcome=failure`. |
| `denied` | Access was refused without an authentication attempt — e.g. an unauthenticated request to a protected endpoint, or an IP-blocked attempt. | SWAG: `403` from the authelia auth_request module before any credential was supplied → `auth_outcome=denied`. fail2ban: a connection from a banned IP → `auth_outcome=denied`. |
| `challenge` | An MFA prompt was issued; the user has not yet completed it. The result is "pending" from the user's perspective. | Authelia: `{"level":"info","msg":"Authentication attempt successful","path":"/api/secondfactor/totp"}` for the **first factor** before the second factor completes — distinct event from the eventual success/failure. |

**Where this vocabulary is used:**

- `logs.auth_outcome` column (added by enrichment migration 10).
- Notification rule matcher: `field_eq = { auth_outcome = "failure" }`
  (notification-rules.schema.json `examples[1]` line 267).
- MCP search filter (parser-bound dimension).

**Closed set.** Adding a 5th value (e.g. `locked`, per
enrichment-framework spec §13 open question 2) requires:

1. Update the `AuthOutcome` enum in `parser-trait.rs`.
2. Update this contract.
3. Update any CHECK constraint on the `auth_outcome` column.
4. Bump the relevant parser's `parser.version` int (per
   metadata-json-shape.md §8).

**`auth_outcome` does NOT map onto severity.** A `success` event is
typically `info`, a `failure` is `notice` or `warning`, a `denied` is
`warning`, and a `challenge` is `info` — but these are conventions of
the parsing source, not a normative mapping. Rule writers who care
about both filter on both: `field_eq = { auth_outcome = "failure" }` and
`severity_min = "warn"` (both as top-level rule properties).

---

## 3. Alert tiers (3 values)

A UX-oriented classification used by the notification system. Source:
`docs/contracts/notification-rules.schema.json::$defs.severityTier`
(line 43–46).

| Tier | Push behaviour | Dedup behaviour | Quiet hours? | Repeat? |
|---|---|---|---|---|
| `info` | **Never pushed** as a standalone alert. Surfaces in the morning digest only (digest spec §10). | n/a | n/a | n/a |
| `warn` | Single push on first match. | Identical fingerprint suppressed for `dedup_window` (default 1800 s). | **Suppressed** during quiet hours (queued into the next digest, NOT released afterward, per digest spec §8). | No. |
| `critical` | First push immediately. | First fire pushes; re-fires within `dedup_window` (default 600 s) are coalesced into `fire_count` (digest spec §6, §7). | **Bypasses** quiet hours. Per-rule `deliver.quiet_hours_override.respect = false` lets `warn` rules also bypass. | Yes — every `repeat_seconds` (default 1800 s) until acked via the `alerts_ack` MCP action (digest spec §9 / `mcp-actions.md::alerts_ack`). |

**Where this vocabulary is used:**

- `notification-rules.schema.json` top-level `severity` field (rule
  severity tier, line 23–27).
- `notification-rules.schema.json::matchClause.severity_min` (rule
  matcher; line 69–73).
- `alert_state.severity` column (digest spec §6).
- `mcp-actions.md::alerts_active` request parameter `severity_min`
  (mcp-actions.md line 267, line 305) — filters returned alerts.
- Apprise `type` mapping: `info → info`, `warn → warning`, `critical
  → failure` (digest spec §3).

**Closed set.** Adding a 4th tier (e.g. `panic`) is a **major** version
bump: every notification rule, every quiet-hours rule, every digest
template would need an update. Don't.

---

## 4. Normative `syslog_severity → alert_tier` mapping function

This is the canonical mapping. Implementations:

- The notification rule evaluator, when a rule has `severity_min` in
  its match clause (notification-rules.schema.json line 69).
- The `alerts_active` MCP action, when called with `severity_min` in
  its request (mcp-actions.md line 267).
- The alert classifier, when assigning a tier to an alert based on the
  triggering log row's `severity` if the rule has no explicit
  `severity` field (the rule's top-level `severity` field is the
  preferred path; this mapping is the fallback).

| Syslog severity (in) | Alert tier (out) |
|---|---|
| `debug` | `info` |
| `info` | `info` |
| `notice` | `info` |
| `warning` | `warn` |
| `err` | `warn` |
| `crit` | `critical` |
| `alert` | `critical` |
| `emerg` | `critical` |

**Rationale for the bucket boundaries:**

- `debug` / `info` / `notice` are all "things you can read in the
  morning digest." Nothing here pages a human.
- `warning` / `err` are "things you'd want to know about same-day."
  Both push at the `warn` tier — there is no UX difference between a
  homelab warning and a homelab error from an alerting standpoint;
  the underlying syslog severity is preserved in the row for
  forensics.
- `crit` / `alert` / `emerg` are "things that demand action right
  now." All bypass quiet hours, all repeat until acked.

This mapping is **one-way**. Going backwards (tier → severity) is
ambiguous and not defined.

---

## 5. `severity_min` interpretation rules

When a rule or MCP filter has `severity_min = <tier>`, the predicate
matches every row whose mapped tier is `>= <tier>` in tier order
`info < warn < critical`.

| `severity_min` value | Matches rows where mapped tier is | Equivalent SQL predicate |
|---|---|---|
| `info` | `info`, `warn`, or `critical` (any) | always true |
| `warn` | `warn` or `critical` | `severity IN ('warning','err','crit','alert','emerg')` |
| `critical` | `critical` only | `severity IN ('crit','alert','emerg')` |

**SQL fragment** for the `severity_min = 'warn'` case used in
`alerts_active` and rule evaluation:

```sql
-- Where :severity_min is one of 'info', 'warn', 'critical':
AND severity IN (
    CASE :severity_min
        WHEN 'info'     THEN 'debug'   ELSE NULL END,
    CASE :severity_min
        WHEN 'info'     THEN 'info'    ELSE NULL END,
    CASE :severity_min
        WHEN 'info'     THEN 'notice'  ELSE NULL END,
    CASE :severity_min
        WHEN 'info'     THEN 'warning'
        WHEN 'warn'     THEN 'warning' ELSE NULL END,
    CASE :severity_min
        WHEN 'info'     THEN 'err'
        WHEN 'warn'     THEN 'err'     ELSE NULL END,
    'crit', 'alert', 'emerg'    -- always included for any tier
)
```

A cleaner (faster, no NULL noise) implementation precomputes the
allowed-severity set at rule load time:

```rust
fn severity_filter(tier: AlertTier) -> &'static [&'static str] {
    match tier {
        AlertTier::Info     => &["emerg","alert","crit","err","warning","notice","info","debug"],
        AlertTier::Warn     => &["emerg","alert","crit","err","warning"],
        AlertTier::Critical => &["emerg","alert","crit"],
    }
}
```

…then emits an `IN (?, ?, …)` clause with the slice expanded. This is
the recommended implementation; the CASE form above is illustrative.

### Worked example

Rule `authelia-mfa-bruteforce` (notification-rules.schema.json
`examples[1]`):

```toml
match.severity_min = "warn"
match.field_eq = { auth_outcome = "failure" }
```

This rule matches a row with:
- `severity = "err"` and `auth_outcome = "failure"` → ✓ (tier=warn)
- `severity = "crit"` and `auth_outcome = "failure"` → ✓ (tier=critical, ≥ warn)
- `severity = "notice"` and `auth_outcome = "failure"` → ✗ (tier=info, < warn)
- `severity = "warning"` and `auth_outcome = "success"` → ✗ (auth_outcome predicate fails)

---

## 6. Why three vocabularies (rationale)

Why we don't collapse to one or two:

| Vocabulary | Why it must exist independently |
|---|---|
| **Syslog severities (8)** | A fixed UNIX standard. Routers, switches, embedded gear, and every userspace daemon on Earth emit syslog severities. Redefining or collapsing them would lose information at ingest. Preserve them on the row as-is. |
| **`auth_outcome` (4)** | A domain-specific enum that is orthogonal to severity. A `success` is `info`, a `failure` is `notice`/`warning`, a `denied` could be either — the dimension is "what happened to the auth attempt," not "how alarming was it." Folding it into severity would lose the auth/non-auth distinction that powers brute-force detection rules. |
| **Alert tiers (3)** | A UX concept. Operators care about three buckets: "I'll read it tomorrow" (info), "I'll deal with it before the day ends" (warn), and "wake me up" (critical). 8 syslog severities is too many for a phone notification UI; 4 auth_outcomes is orthogonal to alerting. The 3-bucket distillation is what notification rules and quiet-hours logic operate on. |

The three vocabularies meet at exactly one point: `severity_min` on a
rule matcher converts an alert-tier value into a set of syslog
severities. That conversion is §4. Nothing else in the system needs
to cross between vocabularies.

---

## 7. Anti-patterns — what NOT to do

The following patterns are forbidden by this contract. Code reviewers
SHOULD reject PRs that introduce them.

1. **Don't filter notification rules on raw syslog severity.** The rule
   schema does not allow `severity = "err"` as a match operator —
   only `severity_min = <tier>` (an alert-tier value) is supported.
   This forces every rule to think in alert tiers and prevents
   bikeshedding on whether `notice` or `info` should fire a push.

2. **Don't render an alert tier as a syslog severity.** When emitting
   a push, the `type` field is apprise's enum (`info` / `warning` /
   `failure`), not a syslog severity. Don't map `critical` to `crit`
   in an apprise payload — they are different vocabularies that
   happen to share some letters.

3. **Don't introduce a 4th vocabulary.** "Effective severity,"
   "incident urgency," "page priority" — pick one of the three above
   and use it. Adding a 4th overlapping vocabulary forces every
   downstream consumer to learn one more mapping function.

4. **Don't write `auth_outcome` to the `severity` column.** They look
   superficially similar (both 4-ish values) but they mean different
   things. The DB schema enforces this — `auth_outcome` is its own
   column added by enrichment migration 10.

5. **Don't expose syslog severities in alert-targeted UIs.** The
   morning digest, the apprise push, the `alerts_active` MCP response
   — all of these speak alert tiers. The forensic search surface
   (`cortex search`, `cortex errors`) speaks syslog severities. Keep
   the two surfaces consistent within themselves.

6. **Don't introduce `severity = "warn"` (the abbreviated form) into
   the syslog vocabulary.** The syslog name is `warning` (full word);
   the alert-tier name is `warn` (abbreviated). The asymmetry is
   intentional — it makes typos cheap to spot in review.

---

## 8. Self-check — every spec reference is covered

| Spec / contract reference | Resolved by this contract |
|---|---|
| `notification-rules.schema.json::severityTier` enum | §3 (the canonical 3-tier definition). |
| `notification-rules.schema.json::matchClause.severity_min` | §4, §5 (mapping + interpretation + SQL fragment). |
| `mcp-actions.md::alerts_active` request `severity_min` | §5 — same predicate as rule matchers. |
| `mcp-actions.md` error code list mentions `agent_offline` / `rate_limited` / etc. (line 36) | Out of scope (those are MCP error codes, not severity vocabularies). |
| `digest-notifications-design.md` §3 apprise `type` mapping | §3 tier table covers it. |
| `digest-notifications-design.md` §7 severity tiers table | §3 — same definitions. |
| `parser-trait.rs::AuthOutcome` enum | §2. |
| `parser-trait.rs::ParserOutput::severity: Option<&'static str>` | §1 — must be one of the 8 syslog severity strings. |
| `incident-card.md` §2 `severity_max` placeholder | §1 — must be one of the 8 syslog severity strings (the **maximum** severity, i.e. lowest numeric value, of the sample logs in the incident). |
| `src/db/queries.rs::SEVERITY_LEVELS` (line 880) | §1 — same 8 values in the same order. |
| `src/app/correlate.rs::13` (severity threshold slicing) | Uses §1 as the underlying scale. |
| `enrichment-framework-design.md` §7.3 Authelia → `auth_outcome` mapping | §2. |
| `enrichment-framework-design.md` §13 open question 2 (`locked`) | §2 stability note — extending the enum is a tracked process. |

---

## 9. Required downstream contract updates

| File | Required change |
|---|---|
| `docs/contracts/notification-rules.schema.json` line 71 (`severity_min` description) | Add: "Maps to the underlying syslog severity per docs/contracts/severity-mappings.md §4." |
| `docs/contracts/mcp-actions.md::alerts_active` (line 267 / 305) | Cross-link to severity-mappings.md §5 for the `severity_min` predicate semantics. |
| `docs/superpowers/specs/2026-05-16-digest-notifications-design.md` §7 (Severity Tiers table) | Cross-link to severity-mappings.md §3, §4 — the spec table's `info`/`warn`/`critical` definitions are repeated here. |
| `docs/superpowers/specs/2026-05-16-enrichment-framework-design.md` §7.3 (Authelia `auth_outcome`) | Cross-link to severity-mappings.md §2 — the four-value enum is normative here. |
| `docs/contracts/parser-trait.rs::AuthOutcome` doc comment | Add: "See docs/contracts/severity-mappings.md §2 for value semantics and sample log lines." |
| `docs/contracts/incident-card.md` §2 `severity_max` row | Already correct; this contract makes the constraint explicit (must be one of the 8 syslog severity strings). |
| `src/mcp/schemas.rs` (when `alerts_active` action lands) | Use the closed enum `["info","warn","critical"]` for `severity_min`, identical to notification-rules.schema.json. Document the mapping. |
