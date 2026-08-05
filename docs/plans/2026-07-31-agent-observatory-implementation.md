# Agent Observatory complete implementation plan

Status: ready for implementation review
Tasks: AO-001 through AO-112 plus ENV-001 through ENV-004 (116 total)
Expected task size: 5-10 focused minutes each
Source baseline: Cortex `bd3c246696312e87fc76720557b7d0dfaae46441`
Aurora baseline: `19e87c6b26dd32eb3be3db4d8b6fb32c99cd1d6d`

## Goal

Implement a production-grade Cortex Agent Observatory that continuously discovers repositories/worktrees/branches, materializes durable agent runs, unifies transcript/command/Git/MCP/hook/skill/LLM/OTLP/host evidence, attributes exact commits, exposes REST/MCP/CLI and authenticated live streaming, and ships a complete static Next.js application using Aurora registry components inside the existing single Rust binary.

## Read before implementation

1. `docs/research/2026-07-31-agent-observatory.md`
2. `docs/design/agent-observatory-architecture.md`
3. `docs/design/agent-observatory-ui.md`
4. `docs/specs/agent-observatory.md`
5. `docs/contracts/agent-observatory.md`
6. every companion SQL/JSON/OpenAPI/Rust/TypeScript/Aurora-lock artifact
7. `docs/plans/agent-observatory/00-execution-contract.md`
8. `docs/plans/agent-observatory/01a-transcript-forward-env-rename.md`

## Plan files

| Phase | Tasks | File | Gate |
| --- | --- | --- | --- |
| execution rules | all | `agent-observatory/00-execution-contract.md` | G0-G8 proof rules |
| contracts/schema/config | AO-001..018 | `agent-observatory/01-foundation-and-schema.md` | G1 Storage |
| transcript-forward env compatibility | ENV-001..004 | `agent-observatory/01a-transcript-forward-env-rename.md` | mandatory compatibility gate |
| projector and Git | AO-019..040 | `agent-observatory/02-projector-and-git.md` | G2 Projection, G3 Git |
| OTLP/API/stream/MCP/CLI | AO-041..064 | `agent-observatory/03-otlp-api-stream-cli.md` | G4 OTLP, G5 API |
| Next/Aurora/embed | AO-065..078 | `agent-observatory/04-nextjs-aurora-foundation.md` | static-shell gate |
| full web application | AO-079..096 | `agent-observatory/05-observatory-ui.md` | G6 Web |
| hardening/docs/release | AO-097..112 | `agent-observatory/06-production-hardening-and-docs.md` | G7 Operations, G8 Release |

## Critical path

```text
AO-001
  -> AO-002..018 schema/config
  -> ENV-001..004 transcript-forward env rename
  -> AO-019..021 identity/lifecycle/evidence
  -> AO-022..040 Git + projection
  -> AO-041..053 OTLP
  -> AO-054..064 queries/API/stream/MCP/CLI
  -> AO-065..078 Next/Aurora/embed
  -> AO-079..096 full UI
  -> AO-097..112 production proof
```

No UI work may invent fields before the REST contract is green. No projector may advance a source cursor before migration and transaction tests are green. No legacy web removal occurs before investigation parity.

## Safe parallel lanes

After AO-018 and ENV-001 through ENV-004, these lanes can proceed in separate worktrees if each rebases at a phase gate:

- **Lane A:** identity, lifecycle, attribution, source projection
- **Lane B:** Git parsers/reconciler/watcher/commit import
- **Lane C:** OTLP traces and metrics
- **Lane D:** query models and cursor codec
- **Lane E:** Next/Aurora build foundation, stopping before live API integration

Merge order remains schema -> DB models -> core domain -> services -> API -> web. Shared-file conflicts in `src/db/pool.rs`, `src/api.rs`, `src/runtime.rs`, `Cargo.toml`, and web lockfiles are resolved only at named integration tasks.

## Feature flags and rollout order

Recommended additive flags:

```text
agent_observatory.enabled
agent_observatory.projector_enabled
agent_observatory.git.enabled
agent_observatory.otlp_traces_enabled
agent_observatory.otlp_metrics_enabled
agent_observatory.web_enabled
```

Rollout:

1. ship additive schema with all new supervisors disabled
2. run dry-run/backfill validation
3. enable projector and verify lag/counts
4. enable Git observer and exact commit evidence
5. enable OTLP traces/metrics per host
6. expose read API/MCP/CLI
7. expose Next/Aurora route to operators
8. prove investigation parity and remove legacy app
9. make desired flags default only after operational evidence

Rollback disables optional supervisors/routes and returns to the prior binary. Additive tables remain. No destructive downgrade is required.

## Task completion rule

A task is complete only when its phase file's RED, GREEN, Proof, and Gate entries are recorded in the proof log. “Implemented” without a reproduced failing test and passing command is not complete.

Each task must stay narrow. When a task grows beyond 10 minutes because of a newly discovered concern, create the smallest numbered subtask after the current plan range or split before writing production code. Update this index and the proof log.

## Integration checkpoints

### Checkpoint A after AO-018 and ENV-004

- contract validator green
- schema 47 fresh and schema-43 upgrade green
- new transcript-forward variable is authoritative and generated everywhere
- legacy alias precedence, warnings, doctor migration, and local watcher independence are proven
- no production feature starts yet

### Checkpoint B after AO-040

- repository/worktree topology live in DB
- all existing durable sources project idempotently
- exact commit attribution and backfill recovery green

### Checkpoint C after AO-064

- all three OTLP signals functional
- read APIs, stream, MCP, and CLI contract-compatible
- no browser dependency required to inspect runs

### Checkpoint D after AO-078

- reproducible pinned Next/Aurora shell embedded by release Rust binary
- strict CSP and asset audit green

### Checkpoint E after AO-096

- complete observatory and migrated investigation application
- desktop/mobile/live/reset/accessibility behavior green

### Checkpoint F after AO-112

- retention, storage, backup, recovery, performance, security, documentation, release, and clean-room proof green

## Production acceptance scenario

The final real-binary acceptance fixture must perform this end-to-end sequence:

1. start Cortex against a schema-43 fixture and upgrade to 47
2. create a temporary repository and linked worktree
3. start a synthetic Claude session with transcript, command, hook, MCP, OTLP log/span/metric evidence
4. make two exact commits and change branch state
5. verify one active run appears under the correct repository/worktree
6. open `/app/agents/`, connect with bearer, select the run, and verify all event facets
7. interrupt the stream, append evidence, reconnect, and verify no gap/duplicate
8. force stream cursor expiry and verify reset/snapshot recovery
9. finish the session and verify terminal status
10. run retention and confirm summary remains while expired detail is removed according to policy
11. backup and restore the database and verify identity/count/integrity
12. run status/doctor and receive a healthy report with source/Aurora/Next/schema/projection revisions

The scenario must use the release binary and a real browser. Mock-only evidence cannot close G8.

## Definition of production ready

Production ready means all 116 tasks complete and all master gates in the execution contract green. The merge report must list commands, test counts, revision identifiers, performance results, and any accepted limitations with issue references.

The feature is not production ready when any of the following remains:

- migration/backfill/replay uncertainty
- inferred evidence displayed as verified
- OTLP trace or metric endpoint still deferred
- token persisted or placed in URL
- unbounded queue/query/DOM/cardinality
- mutable Aurora registry dependency
- source maps or external fonts in production
- script CSP unsafe-inline/eval
- undocumented config/route/action/command
- generated configuration still emits `CORTEX_AGENT_AI_TRANSCRIPTS`, or alias precedence/removal gates are unproven
- failed accessibility, performance, backup, restore, doctor, or rollback proof
- legacy investigation functionality lost

## First implementation command

After approval, create or enter the implementation worktree and begin only AO-001:

```bash
just check-agent-observatory-contracts
```

It must fail because the verification script does not exist. Implement AO-001, record red/green proof, commit it, then continue in numeric order unless the safe-parallel-lane rules are explicitly used.
