# W16 Cortex Artifact Evidence Contract v1

Status: frozen for first implementation slice
Schema: dinglebear.cortex-artifact-evidence/v1
Date: 2026-08-19

## Wire principles

The envelope is Cortex-owned evidence vocabulary, not an ArtifactCandidate or ArtifactInterchange schema. JSON uses camelCase field names and denies unknown top-level fields. Unsupported schema versions fail closed.

## Required fields

- schemaVersion: exactly dinglebear.cortex-artifact-evidence/v1
- eventId: caller-stable opaque idempotency token
- eventKind: closed v1 evidence-kind enum
- sourceSystem: bounded source token such as axon, depot, labby, or phoenix
- sourceIssuer: bounded source/issuer identity reference
- observedAt: RFC3339 timestamp

At least one of artifactId, revisionId, contentDigest, or provenanceRef is required.

## Optional opaque dimensions

- artifactId
- revisionId
- contentDigest as sha256:<64 lowercase hex>
- provenanceRef
- requestId
- correlationId
- causationId
- targetId
- targetKind
- loadoutId
- shareGrantId
- capabilityLeaseId
- deploymentPlanId
- runtimeId
- pluginId
- operationRef
- outcome
- metadata

All IDs/references are evidence labels. Cortex does not dereference them or infer authority from them.

## Event kinds

V1 accepts:

- discovery_observed
- intake_observed
- imported
- installed
- uninstalled
- updated
- forked
- followed
- added_to_gateway
- loadout_bound
- gateway_lifecycle
- runtime_lifecycle
- runtime_call
- deployment_planned
- deployment_staged
- deployment_verified
- deployment_failed
- deployment_rolled_back
- target_lifecycle
- phoenix_plugin_lifecycle
- approval_recorded
- capability_lease_issued
- capability_lease_used
- capability_lease_revoked
- share_grant_created
- share_grant_used
- share_grant_revoked
- security_finding
- license_finding
- trust_finding
- quarantine_finding
- failed
- retried
- cancelled

## Outcome

Optional outcome is one of success, failure, denied, cancelled, pending, or unknown. It is evidence supplied by the source, not Cortex policy state.

## Bounds

First-slice hard limits:

- REST wire body before parsing: 32 KiB
- envelope after safe canonicalization: 16 KiB
- opaque ID/reference: 256 bytes
- source system: 64 bytes
- source issuer: 256 bytes
- metadata keys: 64 bytes each
- metadata string leaves: 1024 bytes each
- metadata object entries: 32 per object
- metadata array entries: 32 per array
- metadata nesting depth: 4
- metadata serialized bytes: 8 KiB
- query limit: default 50, maximum 500

## Secret and raw-content policy

Metadata keys are compared case-insensitively after removing punctuation. Keys containing credential/token/secret/password/private-key/API-key style names fail validation.

Raw-content keys such as raw, body, requestBody, responseBody, arguments, prompt, artifactContents, or equivalent normalized forms are rejected. Runtime-call evidence stores bounded metadata such as operation/tool identity, duration, result size, status, error class/code, and retry/cancellation state only.

String values are passed through Cortex's existing secret redactor before canonical serialization. Top-level IDs and references that themselves look secret-like are rejected rather than persisted redacted because they are identity dimensions.

## Durable representation

Each accepted event is projected to one canonical logs row:

- app_name = artifact-evidence
- timestamp = observedAt normalized to UTC RFC3339
- hostname = cortex-artifact-evidence (explicit synthetic owner required by the canonical log schema; producer systems must not pollute host inventory)
- source_ip = artifact-evidence://<sourceSystem>/<sourceIssuer>
- event_action = eventKind
- metadata_json = canonical safe v1 envelope
- message/raw contain only a bounded generated summary, never caller raw bodies

The append uses Cortex's existing write lock, SQLite transaction, and insert_logs_batch_in_tx path.

## Idempotency and collision semantics

eventId is scoped to its source namespace. The idempotency key is the tuple (sourceSystem, sourceIssuer, eventId), so independent producers do not collide when they use similar local identifiers.

- first append for a source tuple: insert and return inserted=true plus Cortex log id;
- exact replay from the same source tuple: return existing log id and inserted=false;
- same source tuple and eventId with different canonical evidence: fail with conflict and do not mutate the existing event;
- the same eventId from a different sourceSystem/sourceIssuer is a distinct observation.

The first slice takes the existing process-wide DB write lock and starts an SQLite IMMEDIATE transaction before the replay lookup, closing the check-then-insert race across multiple Cortex processes sharing one database. A later indexed projection may make the uniqueness check O(log n) without changing semantics.

## Query contract

Exact filters are ANDed. Supported filters: eventKind, artifactId, revisionId, contentDigest, correlationId, requestId, targetId, sourceSystem, from, to, limit. Results are newest-first by observed time then Cortex log id. Query uses limit + 1 and returns truncated explicitly.

## Authority statement

No event can grant publication, redistribution, trust, approval, policy, capability, share, or deployment authority. Fields carrying those concepts describe what a source reported. Consumers must consult the owning authority for current state.
