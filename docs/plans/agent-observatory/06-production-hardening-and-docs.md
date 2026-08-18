# Phase 6: production hardening, operations, documentation, and release proof

Prerequisite: Phases 1-5 green.
Phase gates: G7 Operations and G8 clean-room release.

## AO-097 Integrate observatory retention

- **Deliverable:** bounded purge for outbox, metric points, spans, run events, observations, removed worktrees, and orphans in contract order.
- **Files:** `src/db/maintenance.rs`, retention models/tests.
- **RED:** expired/mixed-age fixture leaves rows or deletes retained summaries/evidence incorrectly.
- **GREEN:** bounded transactions with per-table counts and configured cutoffs.
- **Proof:** dry-run and apply counts exact; repeated apply deletes zero; foreign-key check clean.
- **Gate:** canonical logs use existing independent retention.
- **References:** contract §14 and existing retention policy.

## AO-098 Integrate storage-budget enforcement

- **Deliverable:** new table groups appear in storage estimates and detail writes block/degrade without blocking canonical logs.
- **Files:** storage-budget/maintenance/status tests.
- **RED:** oversized span/metric/event writes ignore budget or log ingest fails with them.
- **GREEN:** classify observatory detail storage and return partial/block health.
- **Proof:** tiny-budget fixture blocks detail, accepts normal log, exposes blocked counts.
- **Gate:** no silent data loss; source cursor does not advance past blocked projection without documented retry policy.
- **References:** existing `enforce_storage_budget`.

## AO-099 Add backup, restore, and integrity coverage

- **Deliverable:** maintenance backup includes new tables and restored DB passes integrity/semantic counts.
- **Files:** backup/integrity tests and runbook evidence.
- **RED:** seeded backup/restore loses runs, relations, spans, metrics, or cursor state.
- **GREEN:** adapt existing SQLite backup path if necessary; no special sidecar state outside DB except documented checkpoints.
- **Proof:** source/restored checksums and key counts match; stream outbox expiry semantics documented.
- **Gate:** restore never reuses live file handles.
- **References:** current maintenance backup/integrity code.

## AO-100 Add status, doctor, and health diagnostics

- **Deliverable:** status/doctor report projector lag, Git watcher, OTLP signal support, stream bounds, web/Aurora revisions, schema/projection mismatch, and deprecated/conflicting transcript-forward environment configuration.
- **Files:** doctor/status models/CLI/API/MCP tests.
- **RED:** disabled/stopped/lagged/overflow/stale-manifest plus legacy-only and conflicting transcript-forward env fixtures all look healthy.
- **GREEN:** explicit conditions with remediation text; legacy diagnostics describe remote forwarding and never imply local session ingestion is disabled.
- **Proof:** each injected condition produces one named warning/error and healthy fixture using only `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD` is clean.
- **Gate:** doctor is read-only unless existing `--fix --yes` explicitly supports a safe repair.
- **References:** current doctor and watch-status patterns.

## AO-101 Add internal metrics and structured logs

- **Deliverable:** bounded metrics/logs for projector, Git, OTLP, stream, API, and asset revision without session-ID cardinality.
- **Files:** runtime telemetry, tests/docs.
- **RED:** metric-name/dimension allowlist test detects raw run/session/path labels or missing required counters.
- **GREEN:** aggregate counters/histograms and rate-limited structured errors.
- **Proof:** fixture emits expected names/dimensions; no secret/path/session in labels.
- **Gate:** telemetry cannot recursively ingest itself without existing protections.
- **References:** spec §16 and Claude cardinality research.

## AO-102 Complete privacy and redaction matrix

- **Deliverable:** central policy for prompt/tool/command/path/user/email/author fields applied at ingest, projection, REST, stream, MCP, CLI, and UI.
- **Files:** privacy module/tests; source adapters; docs.
- **RED:** canary secrets/identity/path values leak through at least one surface.
- **GREEN:** reuse scrubber, add field allow/omit/hash decisions and response-level defense.
- **Proof:** matrix test seeds canaries and scans DB/API/SSE/MCP/CLI/HTML output according to each config combination.
- **Gate:** default prompt/tool/user/email detail off; command remains scrubbed.
- **References:** config privacy contract and security docs.

## AO-103 Add database and query performance benchmarks

- **Deliverable:** deterministic benchmark/measurement harness for 1M logs, 100k events, 10k runs, 10k spans/metrics.
- **Files:** ignored benchmark tests or xtask, JSON output schema.
- **RED:** harness asserts architecture budgets and initially fails or reports baseline.
- **GREEN:** add only missing indexes/query refinements backed by EXPLAIN and measurements.
- **Proof:** run list p95 <150ms, event page p95 <200ms on the documented reference-hardware profile; query plans archived.
- **Gate:** no index added without measured query need and write/storage cost note.
- **References:** architecture performance budgets.

## AO-104 Add projector/Git/SSE load and failure tests

- **Deliverable:** bounded concurrency tests for source bursts, watcher overflow, SQLite busy, 256 streams, slow clients, cancellation, shutdown.
- **Files:** integration/load tests; proof JSON.
- **RED:** injected failure hangs, leaks tasks, blocks ingest, or exceeds queue caps.
- **GREEN:** tune existing bounds/backoff/cancellation only.
- **Proof:** test completes under timeout; task/client counts return to baseline; fast clients receive complete sequence.
- **Gate:** no sleep-based flaky timing where barriers/injected clocks work.
- **References:** execution G2/G3/G5.

## AO-105 Add frontend bundle and interaction performance gates

- **Deliverable:** compressed route budgets, long-task check, bounded timeline DOM, stream burst render measurement.
- **Files:** build analyzer script, Playwright performance tests, thresholds.
- **RED:** intentionally import telemetry eagerly or disable virtualization and see test fail.
- **GREEN:** dynamic imports, memoization, batched reducer updates, route splitting.
- **Proof:** agents initial JS <=350KiB compressed target or approved measured adjustment; no >200ms long task during 100-event burst.
- **Gate:** budget adjustment requires recorded evidence, not convenience.
- **References:** UI performance implementation.

## AO-106 Complete accessibility and visual matrix

- **Deliverable:** automated keyboard, axe, 200% zoom, reduced-motion, desktop/mobile, focus/live-region, chart-alternative tests and deterministic screenshots.
- **Files:** Playwright specs/screenshots.
- **RED:** at least one test fails before each missing state/behavior is implemented.
- **GREEN:** fix semantics/focus/responsive layout using Aurora components/tokens.
- **Proof:** zero serious/critical axe findings and all critical workflows keyboard-only.
- **Gate:** screenshots supplement semantic assertions.
- **References:** UI accessibility and test matrix.

## AO-107 Add CSP, offline, and network security browser gates

- **Deliverable:** browser test fails on CSP violation, external runtime request, source map, token in request URL/log, unsafe script directive.
- **Files:** Playwright security spec and server log capture.
- **RED:** controlled violation fixture is detected.
- **GREEN:** run against release binary and normal routes.
- **Proof:** zero violations/external requests; all API/stream auth in header; offline static shell loads then shows API unavailable safely.
- **Gate:** no exception suppresses browser console CSP errors.
- **References:** contract §17.

## AO-108 Update all public and operator documentation

- **Deliverable:** accurate current-state docs after implementation, not future tense, including the transcript-forward environment rename and deprecation schedule.
- **Files:** README, docs README/INVENTORY/architecture/API/CLI/config/contracts/source kinds/runtime/retention/security/MCP tools/actions, deployment/runbook/release notes, examples.
- **RED:** documentation checker finds missing routes/actions/commands/config/schema/source kinds, old OTLP 404 claims, or presents `CORTEX_AGENT_AI_TRANSCRIPTS` as a current generated setting.
- **GREEN:** update generated inventories through repository tools and hand-written explanations; current examples use only `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`.
- **Proof:** docs/check-generated scripts pass; every public command/route/action has example and auth/cap note; legacy-name occurrences match the explicit compatibility allowlist.
- **Gate:** do not claim unsupported provider fields or performance numbers without proof artifact.
- **References:** planning package and final code.

## AO-109 Add deployment, upgrade, rollback, and recovery runbook

- **Deliverable:** production steps for schema upgrade, feature flags, backfill, OTLP producer configs, UI verification, disable/rollback, restore, projector recovery.
- **Files:** `docs/runbooks/agent-observatory.md`, deploy docs/examples.
- **RED:** tabletop test follows runbook and encounters missing command/decision.
- **GREEN:** document exact commands, expected output, failure branches, and safe rollback.
- **Proof:** execute runbook against temp old-version/schema-43 fixture through upgrade, backfill, disable, restore.
- **Gate:** no destructive production command without explicit target/backup/check.
- **References:** architecture deployment sequence.

## AO-110 Add provider configuration examples

- **Deliverable:** current Claude, Codex, and Gemini examples for OTLP HTTP logs/traces/metrics with session/project correlation and privacy defaults, plus Cortex agent forwarding examples using `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`.
- **Files:** `deploy/otel/*`, heartbeat-agent deployment examples, docs.
- **RED:** fixture/config validation or review script finds stale/wrong keys or the deprecated transcript variable in a new-install example.
- **GREEN:** derive examples from pinned official docs/schema and Cortex endpoint/auth contract; use the replacement forwarding variable exclusively.
- **Proof:** parse configs where tooling supports it; send matching fixture signals and observe one correlated run; new-install examples contain zero deprecated-variable occurrences.
- **Gate:** secrets represented only as environment placeholders.
- **References:** July 2026 provider sources in research ledger.

## AO-111 Add release and clean-room verification script

- **Deliverable:** one command performs frozen frontend install, Aurora audit, build/manifest, Rust checks/tests/release, seeded server/browser acceptance, doctor/integrity.
- **Files:** `scripts/verify-agent-observatory-release.sh`, Justfile/CI.
- **RED:** script missing and negative fixtures prove it detects stale manifest/Aurora hash/test failure.
- **GREEN:** strict staged command with logs under proof directory.
- **Proof:** run from clean worktree and capture revision/version/test counts; exits 0.
- **Gate:** no network except frozen dependency install/cache and explicitly pinned Aurora refresh is not part of normal verify.
- **References:** execution G8.

## AO-112 Final production acceptance and legacy cleanup

- **Deliverable:** complete proof bundle, all feature flags enabled in temp production profile, old web/static/deferred OTLP claims removed, no orphan code/docs.
- **Files:** proof log, release notes, cleanup diff.
- **RED:** full verifier or inventory grep finds unresolved marker, placeholder, legacy route, static asset, or deferred 404 message.
- **GREEN:** resolve every finding or document accepted non-goal with issue.
- **Proof:** clean-room verifier passes; `git diff --check`; worktree clean after final commit; release binary reports source/Aurora/Next/schema/projection versions and passes smoke.
- **Gate:** user review of proof bundle before merge/deploy.
- **References:** all master gates G0-G8.

## Phase 6 and release gate

Run the canonical final command introduced by AO-111, then independently confirm:

```bash
git status --short
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
pnpm --dir web install --frozen-lockfile
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test
pnpm --dir web build
node web/scripts/audit-aurora.mjs
pnpm --dir web e2e
cargo build --release
```

Required final evidence:

- schema 48 fresh/upgrade/idempotent/integrity
- projection and backfill no loss/duplicates
- exact Git topology and commits under all fixtures
- OTLP logs/traces/metrics provider fixtures and partial/auth/cap behavior
- REST/MCP/CLI contract parity
- stream replay/reset/load/cancel correctness
- Next/Aurora static app in release binary
- accessibility/CSP/offline/token/privacy/performance gates
- retention/storage/backup/restore/doctor/runbook proof
- updated docs and generated inventories
- clean branch ready for review
