# incidents detailed workflow

Load when the task needs action parameters, evidence fields, report structure, or examples beyond the entrypoint.

# Cortex Incident Management

Triage the error-signature backlog, review recent alert firings, find prior
occurrences of a problem, and pull full context around a time window. This
skill is for incident-response workflows — not general log search (use
`using-cortex` for that) and not connection/service failures (use
`troubleshoot` for that).

## Workflow

### 1. Find what's unaddressed

Start here for "what's still open" / "what haven't we fixed" questions.

`cortex action=unaddressed_errors [limit=N] [include_acknowledged=true]`
(CLI: `cortex alerts signatures [--include-acknowledged] [--limit N]`)

Returns `signatures[]`, each with a `signature_hash` (the stable key for
everything downstream), a `template`/`sample_message`, `severity`,
`sample_hostname`, `first_seen_at`/`last_seen_at`, `total_count`, and
`count_last_1h`. By default only unacknowledged signatures are returned —
pass `include_acknowledged=true` to see the full set including ones already
handled.

Triage priority: high `count_last_1h` (still actively firing) beats high
`total_count` alone (could be a stale, no-longer-occurring pattern).

### 2. Acknowledge or un-acknowledge a signature

Once a signature is understood and either fixed or explicitly accepted as
known/benign, acknowledge it so it drops out of the default
`unaddressed_errors` view:

`cortex action=ack_error signature_hash=<hash> [notes="..."]`
(CLI: `cortex alerts signatures ack <signature_hash> [--notes TEXT]`)

To reverse a bad acknowledgement (e.g. it turned out not to be fixed):

`cortex action=unack_error signature_hash=<hash> [reason="..."]`
(CLI: `cortex alerts signatures unack <signature_hash> [--reason TEXT]`)

Both are **admin-scoped actions** — they require `cortex:admin` (static
bearer tokens need `CORTEX_STATIC_TOKEN_ADMIN=true`). Always state which
signature you're acknowledging and why before calling `ack_error` — this is
a state-changing action, not a read.

### 3. Check what's already been notified

`cortex action=notifications_recent [rule_id=... ] [since=...] [limit=N]`
(CLI: `cortex alerts notifications [--rule-id ID] [--since TIME] [--limit N]`)

Use this to answer "did we already get paged for this" before re-raising
something, or to confirm a notification rule actually fired when expected.

### 4. Check whether this has happened before

`cortex action=similar_incidents query="<free text>" [host=...] [app=...] [severity_min=...] [since=...] [until=...] [window_minutes=N] [limit=N]`
(CLI: `cortex sessions similar "<query>" [--host H] [--app A] [--severity-min S] [--since T] [--until T] [--window-minutes N] [--limit N]`)

This clusters matching logs into `IncidentCluster`s (`window_start`/
`window_end`, `log_count`, `severity_peak`, `representative_messages`,
correlated AI sessions if any). Use the error's `template`/`sample_message`
from step 1 as the `query` when following up on a specific signature.

### 5. Pull full context around a window

`cortex action=incident_context since=<ISO> until=<ISO> [host=...] [app=...] [query=...] [severity_min=...] [limit=N]`
(CLI: `cortex sessions incidentcontext --since <ISO> --until <ISO> [--host H] [--app A] [--query Q] [--severity-min S] [--limit N]`)

Both `since` and `until` are **required** — there is no incident-id concept
that threads automatically from the earlier steps into this one. Build the
window yourself: pad a signature's `first_seen_at`/`last_seen_at` (step 1)
or a cluster's `window_start`/`window_end` (step 4) by a few minutes on
each side, then pass that as `since`/`until` here.

Returns severity/app breakdowns for the window plus the actual `error_logs`
and any correlated `ai_sessions` — this is the "show me everything that was
happening" view once you already know roughly when and where.

## Composing the workflow

A typical incident triage session chains these in sequence: `unaddressed_errors`
to find a candidate signature → `similar_incidents` with that signature's
message to see if it's recurring → `incident_context` with a padded window
from either result to see the full blast radius → `ack_error` once you've
confirmed it's handled (or filed a follow-up) — never ack a signature you
haven't actually investigated just to clear the list.

## Guardrails

- `ack_error`/`unack_error` change state. Confirm the signature and
  rationale with the user before calling them unless they've already asked
  you to acknowledge a specific, named signature.
- Don't ack a signature solely because `count_last_1h` is 0 — a quiet
  signature can still represent an unfixed root cause that simply isn't
  triggering right now.
- Don't claim an incident is "new" or "unprecedented" without running
  `similar_incidents` first — recurring problems are common in this fleet
  and the whole point of this skill is catching that.
- When quoting `sample_message`/`representative_messages`/`error_logs`,
  keep quotes short and paraphrase the rest; don't dump full log bodies
  into the response.
