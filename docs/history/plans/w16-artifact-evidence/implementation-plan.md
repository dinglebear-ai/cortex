# W16 Cortex Artifact Evidence Implementation Plan

Status: active
Workstream: W16 / L06-E
Bead: phoenix-ek9
Date: 2026-08-19

## Constraints

- Base only on Cortex origin/main. Do not use the local main checkout as an integration base.
- Do not modify active PR #193, #195, or #196 worktrees.
- Do not claim schema migration 48. PR #195 owns it.
- Do not introduce ArtifactCandidate, ArtifactInterchange, publication, license, or authorization authority into Cortex.
- Keep the first slice transport-neutral below REST/MCP and platform-agnostic.

## C0: Contract and collision freeze

1. Record repo/worktree/PR inventory and the migration-48 collision.
2. Freeze the Cortex-owned artifact evidence envelope and authority boundaries.
3. Define hard limits, redaction rules, idempotency semantics, and exact query dimensions.
4. Checkpoint and push the docs before implementation.

Exit: spec, contract, implementation plan, and progress tracker are committed and pushed.

## C1: Typed evidence domain

1. Add a focused artifact_evidence domain module with serde request/event types and closed enums.
2. Normalize RFC3339 observedAt to UTC.
3. Validate opaque identity/reference bounds and SHA-256 digests.
4. Validate metadata recursively for depth/cardinality/key/string/serialized-byte bounds.
5. Reject secret-shaped and raw-body/content metadata keys.
6. Redact secret-looking metadata string values with the existing Cortex JSON redactor.
7. Generate a bounded persistence summary rather than storing caller raw bodies.
8. Add malformed-input, secret, boundary, and no-panic tests.

Exit: safe normalized event is deterministic and serializable.

## C2: Durable append + replay semantics

1. Add db::artifact_evidence beside existing skill/mcp/hook evidence facets.
2. Under Cortex's existing process-wide write lock and SQLite transaction, resolve eventId replay/collision from canonical durable log metadata.
3. Exact replay returns the original Cortex log id without another write.
4. Same eventId with different canonical evidence returns a conflict and leaves durable state unchanged.
5. New events append through insert_logs_batch_in_tx, preserving existing host/update/transaction semantics.
6. Add DB tests for append, replay, collision, rollback/no-mutation, and persistence mapping.

Exit: first reusable durable evidence slice is tested.

## C3: Shared query/service layer

1. Add bounded exact DB query filters for event kind, artifact/revision/digest, request/correlation, target, source, and observed-at range.
2. Use event_action for indexed event-kind filtering and JSON1 for bounded first-slice opaque dimensions.
3. Return newest-first results with limit + 1 truncation detection.
4. Add CortexService methods for record/list operations; parsing and validation live below transports.
5. Add request/response models and service tests.

Exit: a single transport-neutral common service owns artifact evidence semantics.

## C4: Initial projections

1. Add authenticated REST POST/GET artifact-evidence endpoints using CortexService.
2. Add one MCP artifact_evidence action through ACTION_SPECS/tool dispatch only after the common service is proven.
3. Avoid CLI/MCP/REST behavior divergence: projections expose the same fields, validation, query bounds, and errors.
4. Update generated/user-facing action contract docs only through the repository's established generation/check pattern.

Exit: small external ingest/query surface exists without parallel business logic.

## C5: Adversarial review and gates

1. Run cargo fmt --all -- --check.
2. Run focused artifact evidence tests.
3. Run cargo clippy --all-targets --all-features --locked -- -D warnings.
4. Run relevant repository gates and targeted API/MCP tests.
5. Review diff for authority creep, secret leakage, unbounded inputs, arbitrary raw bodies, path interpretation, transport divergence, migration collision, and Agent Observatory overlap.
6. Fix every finding and rerun affected gates.
7. Commit, push, create/update the draft PR with exact evidence.

## Deferred optimization after PR #195

A dedicated artifact evidence projection/index is deliberately deferred until migration 48 lands. If profiling demonstrates need, introduce a rebuildable migration 49+ projection keyed by artifact/revision/digest/correlation/target. The durable logs facet remains the source event record and the v1 contract does not change.
