---
title: "Correlation Reference -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Correlation Reference -- cortex

cortex has several correlation-style actions. They all work against the
same SQLite `logs` table, but they use different anchors and inclusion rules.

## Summary

| Action | Anchor | Related corpus | AI transcript rows included? | Main use |
| --- | --- | --- | --- | --- |
| `correlate` | Caller-provided `reference_time`, or one derived from an FTS5 hit in AI transcript sessions when `query` is given alone | All log rows in the time window | Yes | Find events near a known timestamp, or near a query-matched past AI session |
| `ai_correlate` | AI transcript rows | Non-AI rows near each AI row | Anchors yes, related rows no | Link agent activity to infrastructure logs |
| `abuse_investigate` | AI abuse incident anchors | Same-session transcript context plus nearby non-AI rows | Anchors/context yes, nearby rows no | Build deterministic abuse evidence bundles |
| `similar_incidents` | FTS5 hits in non-AI logs | AI sessions overlapping each incident window | Incident clusters no, correlated sessions yes | Find historical log clusters and related sessions |
| `incident_context` | Caller-provided `from`/`to` window | Non-AI aggregates/error rows plus active AI sessions | Aggregates/errors no, sessions yes | Build a complete context bundle for a known window |
| `filter` | Structured field predicates | Matching log rows | Depends on filters | Narrow logs for manual correlation |

`timeline`, `patterns`, `anomalies`, and `compare` are analytics actions rather
than correlation engines. They are often useful before or after correlation, but
they do not join or cross-reference AI sessions with infrastructure logs.

## `correlate`

`correlate` searches all log rows in a window centered on `reference_time`:

- Window: `reference_time ± window_minutes`
- Default `window_minutes`: `5`
- Max `window_minutes`: `60`
- Default `severity_min`: `warning`
- Default `limit`: `500`
- Max `limit`: `999`
- Optional filters: `hostname`, `source_ip`, `query`
- Output: rows grouped by `hostname`

This action uses the row `timestamp`, not `received_at`. Sender clock skew can
move events outside the expected window.

AI transcript rows are not excluded. If transcript imports are in the same
window and pass filters, they can appear in the results.

### Deriving `reference_time` from `query`

`reference_time` is required unless `query` is given alone. When
`reference_time` is omitted:

1. `query` is matched by FTS5 against AI transcript sessions (the same
   grouping used by `search_sessions`), scoped by `hostname` if given.
2. The top-ranked session's `first_seen` becomes `reference_time`.
3. The response includes that session as `matched_session`.
4. If no AI session matches `query`, the action errors rather than silently
   falling back to an empty window — the caller must supply `reference_time`
   explicitly.

`query` still separately filters the correlated log rows themselves (as it
always has), independent of whether it was also used to derive the anchor.

This replaces the older, separate `ask_history` action: `ask_history` did the
same AI-session FTS5 lookup but then ran its own smaller, unconfigurable
log-window query (fixed `LIMIT 20`, no severity filter, no host grouping)
instead of the full `correlate` path. Folding it into `correlate` means a
query-derived anchor now gets the same severity gating, host grouping, and
`limit` up to `999` as a caller-supplied `reference_time` does.

## `ai_correlate`

`ai_correlate` first searches AI transcript rows, then uses each transcript row
as an anchor for a nearby non-AI log search.

Anchor filters:

- `project`
- `tool`
- `session_id`
- `ai_query`
- `from`
- `to`
- `limit`

Related-log filters:

- `log_query`
- `hostname`
- `source_ip`
- `app_name`
- `severity_min`
- `events_per_anchor`

Bounds:

- Default `window_minutes`: `5`
- Max `window_minutes`: `120`
- Default anchor `limit`: `10`
- Max anchor `limit`: `50`
- Default `events_per_anchor`: `25`
- MCP max `events_per_anchor`: `200`
- REST hard cap for `events_per_anchor`: `50`
- Default `severity_min`: `warning`

Related rows explicitly exclude AI transcript rows, so transcript streams do
not correlate with themselves.

## `abuse_investigate`

`abuse_investigate` expands AI abuse incident candidates into deterministic
evidence bundles.

For each incident it returns:

- Anchor transcript rows that matched abuse terms.
- Transcript rows before the first anchor in the same session.
- Transcript rows after the last anchor in the same session.
- Nearby non-AI logs around the incident window.

Bounds:

- Default incident `limit`: `3`
- Max incident `limit`: `10`
- Default grouping `window_minutes`: inherited by the incident search path
- Default `correlation_window_minutes`: `5`
- Max `correlation_window_minutes`: `120`
- Transcript context cap: `20` before and `20` after
- Nearby non-AI log cap: `50`

Nearby logs use the first and last incident timestamps expanded by
`correlation_window_minutes`.

## `similar_incidents`

`similar_incidents` is the current FTS5-only historical incident search. It is
not the full Axon/Qdrant semantic RAG design.

Algorithm:

1. Run FTS5 over non-AI log rows.
2. Optionally filter by `hostname`, `app_name`, `severity_min`, `from`, and `to`.
3. Group hits by `(hostname, app_name, floor(timestamp / window_minutes))`.
4. Return representative message snippets and severity peak for each cluster.
5. Attach top AI sessions whose transcript timestamps overlap each cluster window.

Bounds:

- Required `query`
- Default `window_minutes`: `30`
- Clamp `window_minutes`: `5..=120`
- Default `limit`: `10`
- Max `limit`: `50`
- Candidate scan limit before grouping: `5000`
- Correlated sessions per cluster: top `5`

The incident clusters exclude AI transcript rows. The `correlated_sessions`
field contains overlapping AI sessions only as context.


## `incident_context`

`incident_context` builds a context bundle for a caller-supplied time range.

Required:

- `from`
- `to`

Optional:

- `hostname`
- `app_name`
- `severity_min`
- `limit`

Returned data:

- `total_logs`: non-AI log count in the window
- `by_severity`: non-AI counts by severity
- `by_app`: top non-AI app counts
- `error_logs`: non-AI rows at or above `severity_min`
- `ai_sessions`: AI sessions active in the window

Bounds:

- Default `severity_min`: `warning`
- Default `limit`: `50`
- Max `limit`: `200`
- AI session cap: `20`

The request shape accepts `query`, but the current DB implementation ignores it.
Do not rely on `query` for `incident_context` until the v2 FTS5 integration lands.

## `filter` Correlation Aliases

`filter` supports structured correlation aliases without FTS5:

| Alias | Effect |
| --- | --- |
| `source_kind=docker-stream` | Filters `source_ip` by the `docker://` prefix |
| `source_kind=docker-event` | Filters `source_ip` by the `docker-event://` prefix |
| `source_kind=agent-command` | Filters `source_ip` by the `agent-command://` prefix |
| `source_kind=shell-history` | Filters `source_ip` by the `shell-history://` prefix |
| `source_kind=transcript` | Filters transcript rows with `tool`, `project`, or `session_id` refiners |
| `source_kind=claude` / `codex` / `gemini` | Alias for `tool=<name>` |

Docker refiners:

- `docker_host`
- `container`
- `stream`
- `event_action`

AI/session refiners:

- `tool`
- `project`
- `session_id`

`source_kind=syslog-udp`, `source_kind=syslog-tcp`, and `source_kind=otlp` are
rejected in v1 because transport protocol is not indexed separately.

## Trust Boundaries

- `hostname` is useful for grouping but may be claimed by the sender.
- `source_ip` is the persisted source identifier. Network syslog rows use the
  sender address; Docker, command-history, and transcript importers use synthetic
  URI-style identifiers.
- AI metadata (`project`, `tool`, `session_id`) is grouping metadata, not an
  authorization boundary.
- `metadata_json` helps correlation and debugging but is not trusted identity.

## Related Docs

- [SCHEMA.md](SCHEMA.md) -- MCP argument schema reference
- [TOOLS.md](TOOLS.md) -- action reference
- [../contracts/log-filter-surface.md](../contracts/log-filter-surface.md) -- `filter` alias contract
- [../contracts/source-kinds.md](../contracts/source-kinds.md) -- source identity contract
