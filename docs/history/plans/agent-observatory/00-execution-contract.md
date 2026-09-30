# Agent Observatory implementation execution contract

This file defines how every task in the plan is executed and proven. It is normative for implementation.

## Task duration

Each numbered task targets 5-10 focused minutes for an agent that already has the worktree and dependencies available. A task that cannot be completed inside that slice must stop after the failing proof, explain the newly discovered dependency, and split itself before continuing. It must not silently absorb another concern.

## TDD loop

Every code task follows this exact sequence:

1. Read the referenced production and sidecar test files.
2. Add one failing test or verification fixture.
3. Run the smallest command that proves the test fails for the intended reason.
4. Implement the minimum production change.
5. Run the same command and prove it passes.
6. Run the named regression gate.
7. Inspect `git diff --check` and the focused diff.
8. Record evidence in the task proof log.

A task is not complete when the code “looks right.” It is complete when its red and green evidence and regression gate are reproducible.

## Proof log

Create `docs/plans/agent-observatory/proof/PROOF.md` during implementation. Task IDs may use the primary `AO-` sequence or a mandatory cross-cutting prefix defined by the plan, such as `ENV-`. Each task appends:

```text
## AO-000 or ENV-000 Title
commit/worktree SHA: ...
RED: <command>
RED result: <specific assertion or compiler error>
GREEN: <command>
GREEN result: <specific count/output>
REGRESSION: <command>
REGRESSION result: ...
FILES: ...
NOTES: none | bounded deviation with issue/task reference
```

Large generated outputs, Playwright traces, benchmark JSON, screenshots, coverage, and schema dumps go under `target/agent-observatory-proof/<task-id>/` and are not committed unless the repository already commits that artifact class.

## Required per-task fields

Every task in phase files has:

- **Deliverable:** one concrete state change
- **Files:** exact expected files or modules
- **RED:** failing test/command before implementation
- **GREEN:** minimum production code target
- **Proof:** exact command and observable pass condition
- **Gate:** prerequisite and regression condition before moving on
- **References:** code or contract sections to read

## Stop conditions

Stop the current task and split it when:

- more than one migration or public endpoint is changing
- one test requires more than one distinct production behavior
- a dependency not in the research ledger is required
- a command needs destructive access outside the temporary fixture/worktree
- an external contract differs materially from the pinned July 2026 research
- a schema/API field cannot be represented consistently in SQL, Rust, TypeScript, JSON Schema, and OpenAPI
- a test passes before production implementation for a reason other than an already implemented prerequisite

## Branch and commit discipline

Implementation begins in a new implementation worktree or continues this worktree only after the planning package is reviewed. Recommended branch:

```text
feat/agent-observatory
```

Each task should produce one narrow commit when green. Generated Aurora installs may be one commit per coherent registry bundle. Do not mix formatting or unrelated cleanup into feature commits.

## Dependency policy

### Rust

No new Rust runtime crate is approved. A task that proposes one must first add an ADR showing:

- missing capability in current dependencies/std
- maintenance and security review
- MSRV/edition compatibility
- size/build-time impact
- rejected alternatives
- explicit user approval

### Frontend

Approved new direct dependencies are limited to the versions recorded in the research ledger:

- Next 16.2.11
- React and React DOM 19.2.7
- eventsource-parser 3.1.0
- @tanstack/react-virtual 3.14.8
- parse5 8.0.1
- pinned Aurora transitive dependencies
- test dependencies named in the research ledger

Before adding any test package without a frozen exact version, run `pnpm info <name> version`, inspect official release notes, pin the exact selected version, and record it in the proof log.

## Fixture policy

Fixtures must be deterministic and small:

- SQLite fixtures use a temporary database.
- Git fixtures use a temporary repository with fixed author, timestamps, and branch names.
- OTLP fixtures are protobuf requests built in tests or checked-in binary/JSON descriptions with documented provenance.
- Browser API fixtures are deterministic route handlers or a seeded real Cortex database.
- No fixture may depend on the operator's live home, Git identity, network, API token, or current clock.

## Test tiers

### Tier A, focused

One module/test target, normally under 10 seconds.

Examples:

```bash
cargo test agent_observatory::identity::tests --lib
cargo test git_observer::porcelain::tests --lib
pnpm --dir web vitest run tests/stream-reducer.test.ts
```

### Tier B, subsystem

Run after every 3-5 focused tasks and at every phase gate.

```bash
cargo test agent_observatory --lib
cargo test git_observer --lib
cargo test otlp --lib
pnpm --dir web test
pnpm --dir web typecheck
```

### Tier C, repository

Run at phase boundaries and final release:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test
pnpm --dir web build
pnpm --dir web e2e
bash scripts/check-source-size.sh
bash scripts/check-docs.sh
bash scripts/check-generated.sh
```

Use the repository's actual canonical scripts when implementation reaches that task; update this list if canonical command names differ.

## Production-ready master gates

No feature-complete claim is allowed until all gates are green.

### G0 Planning contracts

- SQL applies to empty SQLite
- JSON and OpenAPI parse
- Rust and TypeScript contract declarations compile
- all docs are indexed

### G1 Storage

- fresh DB at latest schema
- schema-43 upgrade
- repeated startup idempotency
- foreign-key and integrity checks
- retention and storage-budget coverage

### G2 Projection

- every source adapter has fixtures
- replay twice emits no duplicates
- crash before/after cursor transaction behaves correctly
- live ingestion continues while projector fails
- backfill progress and resume proven

### G3 Git

- normal, linked, detached, locked, removed, rebase, reset, and overflow fixtures
- exact SHAs and reachability correct
- no watcher per source file
- command inputs cannot be client-controlled

### G4 OTLP

- logs remain compatible
- traces and metrics accept official protobuf shapes
- provider fixtures for Claude, Codex, Gemini
- duplicates, partial success, caps, auth, and privacy proven

### G5 API and stream

- all REST/MCP/CLI contracts match
- pagination stable under concurrent inserts
- bearer negative tests
- replay boundary has no gaps/duplicates
- expired/lagged/reset behavior proven
- load cap and cancellation proven

### G6 Web

- static Next export embedded in real Rust binary
- full Aurora lock audit
- no mutable runtime URLs or source maps
- strict script CSP and zero browser violations
- desktop/mobile/keyboard/axe/zoom/reduced-motion workflows
- 10,000-event timeline remains virtualized
- reconnect/reset/token-clear flows proven

### G7 Operations

- status/doctor/metrics/logs
- backup, restore, retention, integrity, upgrade, rollback
- operator runbook and config docs
- release build and binary smoke test

### G8 Final clean-room verification

From a clean checkout/worktree with empty build caches where practical:

1. install pinned frontend dependencies with frozen lockfile
2. regenerate and audit Aurora assets
3. build static UI and manifest
4. run complete Rust and frontend test suites
5. build release binary
6. start release binary against a seeded temp database
7. run real-browser acceptance and authenticated stream tests
8. run integrity/status/doctor
9. prove Git worktree is clean except intentional proof artifacts excluded by ignore rules

## Final proof bundle

The final implementation report must include:

- source SHA, Aurora SHA, Next version, schema and projection versions
- task count complete/total
- tests by tier and count
- migration and backfill evidence
- performance results against budgets
- browser/accessibility/CSP results
- storage growth and retention evidence
- known limitations, each with an issue or explicitly accepted non-goal
- rollback command and verification

A statement such as “all tests passed” without commands, counts, and revision identifiers is insufficient.
