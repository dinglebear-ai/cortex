# Graph Investigation Workspace Design

## Status

Approved for design by the user on 2026-06-20. This spec defines the product
shape for a live embedded Cortex web app for investigation-graph exploration.
It does not define the implementation task list; that comes after user review.

## Purpose

Cortex already has a typed investigation graph, graph explain responses,
evidence lookup, timeline/log surfaces, host heartbeat state, and homelab map
knowledge. The missing piece is a live operator workspace that turns those
surfaces into an investigation flow.

The workspace should help answer questions like:

- Why did this host suddenly start OOMing?
- What is the strongest supported explanation for this error burst?
- Which graph relationships are evidence-backed, weak, or ambiguous?
- What was the earliest abnormal signal before the visible failure?
- Which services, containers, sessions, or logs should be inspected next?

## Goals

- Build a live embedded Cortex web app for graph-backed investigations.
- Make "Ask + Explain" the primary workflow.
- Make "BAM Mode" the secondary workflow for sudden homelab failures after a
  quiet baseline.
- Anchor BAM Mode on host pressure signals first, then fan outward to candidate
  causes.
- Keep every claim inspectable through graph relationships, source evidence,
  log summaries, timeline context, trust level, confidence, and open questions.
- Use explicit API versioning for new app-facing routes under `/api/v1`.
- Keep existing `/api/*` routes compatible and leave `/v1/logs` reserved for
  OTLP ingest.

## Non-Goals

- Do not replace the existing raw graph APIs.
- Do not create saved investigation cases in the first version.
- Do not make timing-only correlations look causal.
- Do not require a separate web service for the first version.
- Do not build custom graph physics/layout logic if a browser graph library can
  handle rendering and interaction.

## Product Shape

The app is a single live investigation workspace served by Cortex itself. It is
not a general dashboard.

The main loop is Ask + Explain:

1. The operator asks a natural-language question such as
   `Why did edgehost start OOMing around 02:15?`
2. Cortex resolves relevant entities, time hints, graph neighborhoods, and
   evidence-backed explanation chains.
3. The app shows a ranked answer stack, graph canvas, proof panel, and
   timeline/log context.
4. The operator pivots from claims into graph nodes, relationships, evidence
   rows, raw logs, and suggested next queries.

The secondary loop is BAM Mode:

1. Cortex detects or accepts a visible failure window.
2. It compares the window against a quiet baseline.
3. It anchors on the earliest host pressure signals, such as OOM, load, memory,
   disk, network, or reboot pressure.
4. It fans outward to candidate causes: services, containers, restarts, log
   spikes, AI sessions, deploy/config clues, and error signatures.
5. It returns an automatic first-pass explanation plus interactive pivots for
   every claim.

The interaction model is hybrid: Cortex should be opinionated up front, but the
operator must be able to challenge and inspect every claim.

## Route Strategy

Existing Cortex routes remain in place for compatibility. New app-facing routes
should use `/api/v1`.

`/v1/logs` remains the OTLP-compatible ingest endpoint and should not become the
general app API namespace.

Initial route direction:

```text
/app/investigate
/api/v1/graph/entity
/api/v1/graph/around
/api/v1/graph/explain
/api/v1/graph/evidence
/api/v1/investigations/ask
/api/v1/investigations/bam
```

The graph `/api/v1` routes may begin as compatibility wrappers around existing
graph service methods. Investigation routes can be introduced once the app
needs server-side orchestration beyond direct graph/timeline calls.

## UI Regions

The first workspace has five persistent regions.

Ask bar:

- Natural-language prompt.
- Optional host and time hints.
- Mode control for Ask + Explain vs BAM Mode.
- Submission state and error state.

Answer stack:

- Ranked explanations.
- Competing theories.
- Open questions.
- Suggested next moves.
- Confidence, evidence kinds, and ranking rationale for each item.

Graph canvas:

- Focused graph around the selected answer, entity, or relationship.
- Entity type, trust level, relationship label, confidence, and direction.
- Expand, prune, select, and pivot interactions.
- Visual distinction between verified, claimed, inferred, weak, and ambiguous
  connections.

Evidence panel:

- Selected node or edge details.
- Relationship reason codes.
- Trust and confidence.
- Evidence sample rows.
- Source log summaries when available.
- Missing-source reasons when source rows are retained out or unavailable.
- Guardrails that explain when Cortex is showing correlation rather than
  causality.

Timeline/log strip:

- Quiet baseline, first pressure signal, first visible failure, and error burst.
- Raw log strip around selected moments.
- Pressure overlays from heartbeat/fleet state.
- Clickable pivots back into graph explanations.

## Data Flow

Frontend state is case-oriented. One investigation case contains:

- prompt,
- selected mode,
- resolved entities,
- selected time window,
- baseline window,
- ranked explanation candidates,
- expanded graph neighborhoods,
- selected node or relationship,
- selected evidence row,
- timeline buckets,
- raw log excerpts,
- next queries,
- graph projection metadata,
- truncation and degraded-state metadata.

The initial app should consume existing service primitives where possible:

- graph entity/around/explain/evidence for graph, chains, proof, and next
  queries,
- timeline/search/context APIs for raw logs and time-window context,
- host state and fleet state APIs for pressure signals,
- graph projection status metadata for stale/degraded warnings.

When direct calls become too chatty or push too much ranking logic into the
browser, add server-side investigation endpoints:

- `/api/v1/investigations/ask` for prompt resolution and ranked explanation
  assembly,
- `/api/v1/investigations/bam` for baseline comparison, earliest abnormal
  pressure detection, and candidate-cause ranking.

## BAM Mode Ranking

BAM Mode should rank candidate explanations by:

- temporal ordering: candidate cause before pressure before visible failure,
- evidence diversity: multiple source kinds beat one source kind,
- graph trust: verified edges beat claimed or inferred edges,
- confidence values from graph relationships,
- repeated or escalating signals,
- baseline contrast against quiet periods,
- penalties for missing evidence, weak evidence, and ambiguous entities.

Weak paths should appear as open questions instead of asserted root causes.

## Error And Edge States

The UI must show explicit states for:

- stale graph projection,
- degraded graph status,
- graph projection errors,
- payload truncation,
- ambiguous entity resolution,
- missing source rows,
- retained-out evidence,
- empty results,
- auth failures,
- network failures,
- weak evidence,
- correlation-only paths,
- browser/client version skew against the server.

Silent empty panels are not acceptable for investigation workflows.

## Frontend Architecture

The frontend should be embedded in the Cortex deployment. Static assets can be
served by the Rust binary and copied into the Docker image.

The graph renderer should use a proven browser graph visualization library for
layout, selection, viewport, and interaction. The app should not hand-roll graph
physics.

The first implementation can be a compact single-page app. It should use
Aurora design tokens and a dense operator-tool layout rather than a marketing
or dashboard style.

## Rollout

Phase 1 proves the live loop:

- Serve the embedded app from Cortex.
- Add `/api/v1` graph wrappers or aliases needed by the app.
- Implement Ask + Explain with existing graph, timeline, log, and host-state
  primitives.
- Implement an initial pressure-first BAM Mode, even if ranking is simple.
- Render graph, answer stack, evidence panel, and timeline/log strip.

Phase 2 improves investigation intelligence:

- Add `/api/v1/investigations/ask`.
- Add `/api/v1/investigations/bam`.
- Add baseline comparison and earliest-abnormal-signal detection.
- Improve candidate-cause ranking with evidence diversity, ordering, trust,
  confidence, and open-question penalties.

Saved investigation cases should wait until the live investigation loop feels
useful.

## Testing

Testing should cover API behavior and user experience.

Backend:

- route tests for new `/api/v1` handlers,
- contract tests for response shapes,
- degraded and stale projection cases,
- ambiguous entity cases,
- payload truncation cases,
- BAM ranking fixtures for pressure-first ordering.

Frontend:

- Playwright browser tests against a running Cortex server,
- screenshot checks for desktop and smaller widths,
- graph nonblank/rendered-state checks,
- evidence panel state transitions,
- timeline/log strip interactions,
- visible error states for degraded, truncated, ambiguous, and auth-failed
  responses.

## Approval Boundary

This spec is ready for user review. Implementation should not begin until the
user approves the written spec.
