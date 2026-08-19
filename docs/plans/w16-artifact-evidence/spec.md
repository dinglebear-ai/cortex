# W16 Cortex Artifact Evidence Specification

Status: implementation active
Workstream: W16 / L06-E
Bead: phoenix-ek9
Date: 2026-08-19

## Mission

Cortex is the durable observability, evidence, and history plane for the Labby + Depot/Bazaar + Axon + Phoenix artifact ecosystem. It records what was observed to happen, when, and under which source/configuration/correlation context. It does not crawl sources, publish artifacts, decide license/publication state, grant authorization, deploy Phoenix-specific payloads, or become the knowledge/memory layer.

## Authority boundaries

- Axon owns crawl/index/enrichment and may emit discovery/enrichment evidence.
- Depot owns hosted Artifact Registry/Bazaar publication and hosted license/publication authority.
- Labby owns the personal Artifact + MCP/runtime gateway and local artifact/runtime lifecycle.
- Phoenix owns Unraid control-plane policy/integration and Phoenix-native authorization.
- Cortex stores evidence from all of them without upgrading any caller claim into authority.

Cortex MUST NOT define ArtifactCandidate or ArtifactInterchange. Artifact identity, revision, digest, provenance, publication, license, trust, ShareGrant, CapabilityLease, loadout, deployment, target, runtime, and plugin references are opaque evidence dimensions.

## First vertical slice

The first slice reuses the canonical Cortex log/evidence/query path rather than adding a parallel artifact database:

1. Accept a typed dinglebear.cortex-artifact-evidence/v1 envelope.
2. Validate IDs, SHA-256 digests, RFC3339 observed-at timestamps, source identity, event kind, cardinality, depth, and byte bounds.
3. Reject secret-shaped metadata keys and raw-body/content fields; recursively redact secret-looking string values using Cortex's existing redactor.
4. Canonicalize the safe envelope and append it through the existing SQLite logs transaction path with app_name = artifact-evidence.
5. Use the existing indexed event_action column for event-kind projection.
6. Query exact artifact/revision/digest/correlation/target/source dimensions from the bounded structured metadata, with limit + 1 truncation semantics.
7. Provide idempotent append by caller-supplied eventId: identical replay returns the original log row; same ID with different canonical evidence fails closed as a conflict.
8. Expose ingest/query through the common CortexService; REST and MCP projections call that shared layer rather than duplicating logic.

## Why no migration in the first slice

Current origin/main is schema v47. Active Cortex PR #195 owns migration 48 for Agent Observatory transactional cursors. W16 therefore MUST NOT claim migration 48, create a placeholder 48, or modify Agent Observatory migration/cursor state. A future optimized artifact-evidence projection/index may land as migration 49 or later after #195 is assimilated.

This is intentionally a correctness-first bridge: all first-slice writes remain durable and queryable, while the later migration can add dedicated indexes without changing the wire contract.

## Evidence coverage

The v1 event-kind vocabulary covers bounded evidence for:

- discovery and intake observation;
- import, install, uninstall, update, fork, and follow;
- add-to-gateway, loadout binding, and gateway/runtime lifecycle;
- MCP/Skill/agent runtime calls with metadata only, never arbitrary request/result bodies;
- deployment planned, staged, verified, failed, and rolled back;
- target and Phoenix plugin/runtime lifecycle;
- approvals and CapabilityLease issued/used/revoked;
- ShareGrant created/used/revoked;
- security, license, trust, and quarantine findings as source-attributed observations;
- retries, failures, and cancellations.

## Safety invariants

- Every write has an explicit supported schema version, stable event ID, source system, issuer/source identity, observed-at timestamp, and event kind.
- At least one artifact/provenance subject reference is required.
- Content digests are exactly sha256:<64 lowercase hex>.
- IDs are bounded opaque tokens and are never interpreted as filesystem paths or authorization principals.
- Metadata is bounded by total serialized bytes, object/list cardinality, nesting depth, key length, and string length.
- Credential/secret-shaped metadata keys are rejected. Secret-looking string values are redacted before persistence.
- Raw artifact contents, prompts, tool arguments, request/response bodies, tokens, credentials, private keys, and secret bindings are never accepted as metadata fields.
- Caller-supplied publication/license/trust/policy/approval fields are evidence context only.
- Query limits are clamped and truncation is explicit.
- Malformed inputs return typed validation errors and do not panic.
- Cortex remains independently deployable and has no Phoenix/Unraid runtime dependency.

## Query dimensions

The first slice supports exact filters for event kind, artifact ID, revision ID, content digest, correlation ID, request ID, target ID, source system, observed-at range, and bounded result limit.

The durable row ID is returned as Cortex evidence identity. Source event IDs remain caller-owned idempotency identities.

## Follow-on optimization boundary

After PR #195 lands, add an indexed artifact-evidence projection only if measured query volume warrants it. The projection must be rebuildable from canonical durable log rows, preserve the same v1 envelope, and never become a second source of truth.
