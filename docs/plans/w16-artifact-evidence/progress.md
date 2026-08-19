# W16 Cortex Artifact Evidence Progress

Status: ACTIVE
Workstream: W16 / L06-E
Bead: phoenix-ek9
Updated: 2026-08-19

## Lane identity

- Repository: dinglebear-ai/cortex
- Worktree: /home/jmagar/workspace/cortex/.worktrees/w16-cortex-artifact-evidence-20260819
- Branch: codex/w16-cortex-artifact-evidence-20260819
- Original lane base: origin/main 0ebb3d2bb6c220147dd4ee27f9efb162f3e88ba9 (v3.13.2)
- Current base after docs-only rebase: origin/main ecbd33b8383313c84c5e71a97c06f3a4175e0c6c (#196 merged)
- Draft PR: pending immediate creation after the validated C1-C3 code checkpoint is pushed

## Active-lane inventory

Read-only inspection completed before W16 work:

- PR #193 / codex/graph-projection-lifecycle: active graph-projection lifecycle work in the main checkout.
- PR #195 / feat/agent-observatory-ao039-20260817: Agent Observatory transactional cursors; owns schema migration 48.
- PR #196 / perf/systematic-audit-20260817: inspected while open; it merged to origin/main as ecbd33b8 during C0. W16 rebased onto it before the first commit.
- Agent Observatory AO-040 and the former performance-audit worktree contain local uncommitted work and remain untouched.

The local Cortex main ref is not a safe base because it currently points at graph-projection work ahead of origin/main. W16 was created directly from origin/main.

## Cross-repo evidence reviewed

- Phoenix docs/eight: cortex, artifacts, gateway, depot, axon, deployment, distribution-sharing, identity-policy, security, schemas, and meta-plan. artifact-ecosystem.md was not present.
- Depot W20 PR #37 and /home/jmagar/workspace/depot-w20-artifact-registry: read-only.
- Axon W21 PR #569 and /home/jmagar/workspace/axon-w21-artifact-engine: read-only.
- Phoenix W18 PR #45: read-only.
- Labby origin/main at 85cbedb92: read-only.

## Frozen decisions

1. Cortex owns evidence vocabulary only. It does not own ArtifactCandidate or ArtifactInterchange.
2. Artifact IDs, revisions, digests, provenance, authority/policy/share/lease/deployment refs are opaque evidence dimensions.
3. The first slice is migration-free because PR #195 owns migration 48.
4. Canonical persistence reuses logs + insert_logs_batch_in_tx; no parallel artifact database.
5. event_action is the indexed first-slice event-kind projection; other bounded dimensions use structured metadata queries until a measured migration 49+ optimization is warranted.
6. Caller authority/policy/license/trust fields are source-attributed observations only.
7. Raw artifact/tool/request/result bodies are out of contract; metadata is bounded and secret-safe.
8. Replay/idempotency identity is scoped by (sourceSystem, sourceIssuer, eventId). Exact replay is a no-op; conflicting reuse within the same source fails closed, while unrelated producers may reuse local IDs.
9. Artifact evidence starts an IMMEDIATE SQLite transaction before the replay lookup, closing the check-then-insert race across multiple Cortex processes sharing a DB.

## Checkpoints

- [x] Repository/worktree/open-PR inventory
- [x] Governing Phoenix and sibling-lane contract review
- [x] W16 bead phoenix-ek9 created and claimed
- [x] Spec drafted
- [x] Contract drafted
- [x] Implementation plan drafted
- [x] Progress tracker drafted
- [x] C0 docs committed and pushed as 509d6a14
- [x] C1 typed evidence domain implemented
- [x] C2 durable append/idempotency slice implemented
- [x] C3 query/common service implemented
- [ ] C4 REST/MCP projections
- [x] Adversarial review complete for the C1-C3 slice
- [x] Relevant repository gates green for the C1-C3 slice
- [ ] Draft PR opened and current

## Tests / gates

C0 documentation checkpoint 509d6a14 is pushed to origin.

Validated C1-C3 evidence slice gates:

- cargo fmt -- --check: PASS
- git diff --check: PASS
- cargo clippy --all-targets -- -D warnings: PASS
- focused artifact-evidence tests: PASS, 16 passed / 0 failed / 2317 filtered
- env -u CORTEX_API_TOKEN -u NO_AUTH cargo nextest run --locked: PASS, 2918 passed / 0 failed / 2 skipped in 279.178s
- cargo test --doc --locked: PASS, 0 doctests / 0 failures

Adversarial review fixed two replay issues before checkpointing: cross-process check-then-insert races are closed with an IMMEDIATE SQLite transaction, and eventId idempotency is scoped by sourceSystem + sourceIssuer rather than assuming globally unique producer IDs. Expanded metadata tests cover object/list cardinality, nesting, string bounds, total serialized bytes, secret/raw-body keys, and malformed shapes.

## Blockers / dependencies

- Dedicated indexed projection migration is blocked on PR #195 landing migration 48. This does not block the durable first slice.
- No Agent Observatory abstraction dependency is required for the current design.

## Next action

Commit and push the validated C1-C3 slice, open the draft PR immediately, then add thin REST/MCP projections through CortexService.
