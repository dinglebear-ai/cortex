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
- Draft PR: not opened yet; open at the first tested reusable evidence slice

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
8. eventId supplies replay/idempotency identity. Exact replay is a no-op; conflicting reuse fails closed.

## Checkpoints

- [x] Repository/worktree/open-PR inventory
- [x] Governing Phoenix and sibling-lane contract review
- [x] W16 bead phoenix-ek9 created and claimed
- [x] Spec drafted
- [x] Contract drafted
- [x] Implementation plan drafted
- [x] Progress tracker drafted
- [ ] C0 docs committed and pushed
- [ ] C1 typed evidence domain
- [ ] C2 durable append/idempotency slice
- [ ] C3 query/common service
- [ ] C4 REST/MCP projections
- [ ] Adversarial review complete
- [ ] Relevant repository gates green
- [ ] Draft PR opened and current

## Tests / gates

No code has been changed yet. C0 documentation checkpoint pending git diff validation.

## Blockers / dependencies

- Dedicated indexed projection migration is blocked on PR #195 landing migration 48. This does not block the durable first slice.
- No Agent Observatory abstraction dependency is required for the current design.

## Next action

Validate and push C0, then implement the typed evidence domain and focused safety tests.
