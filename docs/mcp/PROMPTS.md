---
title: "MCP Prompts Reference -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# MCP Prompts Reference -- cortex

cortex exposes reusable MCP prompts for common infrastructure debugging
workflows. Prompts do not add new data access paths; they guide MCP clients to
call the existing `cortex` tool actions in a useful order and to report evidence
before conclusions.

## Execution Rules

Every prompt is written as a bounded investigation runbook:

- Start with cheap, scoped actions such as `status`, `errors`, `tail`, `search`,
  and `timeline`.
- Use small result sets first: `limit=5` for searches, `limit=10` for summaries,
  and `before=3` / `after=3` for `context`.
- For `timeline`, use `bucket=minute` for recent windows. The supported bucket
  values are `minute`, `hour`, and `day`.
- Escalate broader or slower actions only after the first pass produces a
  specific question. Examples include `stats`, `anomalies`, `patterns`,
  `compare`, `clock_skew`, `ingest_rate`, and broad `correlate` calls.
- Summarize representative evidence instead of pasting full JSON payloads.
- Return a consistent synthesis with `Verdict`, `Evidence`, `Likely Cause`,
  `Not Supported`, `Next Actions`, and `Telemetry Gaps`.
- Clients that support structured output can read
  `cortex://schema/prompt-output` and validate prompt answers against that
  JSON Schema.

| Prompt | Purpose | Useful arguments |
| --- | --- | --- |
| `infra.incident-triage` | Build a timeline, scope, likely cause, and next actions for an incident | `window`, `host`, `service` |
| `infra.host-health` | Check one host for silence, error spikes, clock skew, noisy apps, and source identity drift | `host`, `window` |
| `infra.service-outage` | Debug a service, application, or container outage from service logs and nearby host events | `service`, `host`, `window` |
| `infra.security-auth-review` | Review auth failures, bans, suspicious IPs, and correlated infrastructure context | `window`, `actor`, `host` |
| `infra.noise-reduction` | Identify repeated patterns and recommend safe alert tuning or source fixes | `window`, `host`, `service` |
| `infra.agent-change-correlation` | Correlate AI agent work with infrastructure errors and regressions | `project`, `session_id`, `window`, `host`, `service` |
| `infra.docker-container-regression` | Investigate container restarts, healthchecks, image pulls, and Compose regressions | `container`, `host`, `service`, `window` |
| `infra.network-dns-failure` | Debug DNS, proxy, firewall, upstream, and reachability failures | `host`, `service`, `window` |
| `infra.storage-pressure` | Investigate disk pressure, DB growth, cleanup, and write-block risk | `host`, `service`, `window` |
| `infra.auth-bruteforce` | Investigate repeated auth failures, bans, suspicious sources, and blast radius | `window`, `actor`, `host`, `service` |
| `infra.syslog-forwarding-gap` | Investigate stale, missing, spoofed, or delayed syslog forwarding | `host`, `window` |
| `infra.after-deploy-check` | Verify post-deploy health and identify regressions quickly | `service`, `host`, `window` |

## Example

```json
{
  "name": "infra.service-outage",
  "arguments": {
    "service": "plex",
    "host": "nashost",
    "window": "last 45 minutes"
  }
}
```

The rendered prompt asks the client to start with bounded `search`, `errors`,
`timeline`, and `context` calls, then escalate to `anomalies`, `compare`,
`correlate`, or Compose diagnostics only when the first pass leaves a concrete
question.
