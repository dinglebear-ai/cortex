# Phase 1: contracts, schema, models, and configuration

Prerequisite: G0 planning package approved.
Phase gate: G1 Storage.

## AO-001 Add a planning-contract verification script

- **Deliverable:** `scripts/check-agent-observatory-contracts.sh` parses JSON/OpenAPI, applies SQL to an empty SQLite DB, compiles contract Rust/TypeScript declarations, and fails on placeholders outside the example Aurora lock.
- **Files:** new script; `Justfile` target `check-agent-observatory-contracts`.
- **RED:** run the missing script and capture “No such file.”
- **GREEN:** implement strict `set -euo pipefail`; use `jq empty`, `sqlite3`, `rustc --test`, and pinned frontend `tsc --noEmit` or a temporary minimal tsconfig.
- **Proof:** `just check-agent-observatory-contracts` exits 0 and prints SQL integrity `ok` plus two Rust tests passed.
- **Gate:** no network and no live DB access.
- **References:** all `docs/contracts/agent-observatory*` artifacts.

## AO-002 Lock schema and projection version constants

- **Deliverable:** failing tests establish planned schema 47 and projection version 1 before migration code exists.
- **Files:** `src/db/pool_tests.rs`, new `src/agent_observatory.rs`, sidecar test file.
- **RED:** tests expect latest schema 47 and projection constant 1; current schema assertion fails at 43.
- **GREEN:** add constants only, without tables or runtime startup.
- **Proof:** focused constants tests pass; existing schema tests still fail only where intentionally awaiting migrations.
- **Gate:** contract SQL remains source reference, not runtime-included SQL.
- **References:** `src/db/pool.rs`, contract §13.

## AO-003 Implement migration 44 repository table

- **Deliverable:** additive `repositories` table and indexes.
- **Files:** `src/db/pool.rs`, `src/db/pool_tests.rs`.
- **RED:** fresh DB test queries `PRAGMA table_info(repositories)` and required unique/index definitions; it fails.
- **GREEN:** add only repository DDL and migration record 44 scaffold.
- **Proof:** focused migration test plus repeated-open idempotency passes.
- **Gate:** no worktree table yet; foreign-key check remains clean.
- **References:** SQL contract migration 44, existing migration style in `src/db/pool.rs`.

## AO-004 Add repository worktree table

- **Deliverable:** `repository_worktrees` with branch/head/status/lock/prune/removal fields and indexes.
- **Files:** same migration and tests.
- **RED:** insert two worktrees for one repository, reject duplicate host/path, and validate delete cascade.
- **GREEN:** implement only the worktree DDL and indexes.
- **Proof:** test shows duplicate constraint and cascade work; `PRAGMA foreign_key_check` empty.
- **Gate:** field names match SQL and JSON contracts.
- **References:** `docs/contracts/agent-observatory.sql`, JSON `Worktree` definition.

## AO-005 Add repository observations table

- **Deliverable:** append-only `repository_observations` with unique deterministic key.
- **Files:** migration 44 and tests.
- **RED:** duplicate observation-key insertion must fail or no-op according to repository helper contract.
- **GREEN:** add table and repo/worktree time indexes.
- **Proof:** chronological query plan uses named index and returns deterministic ID order.
- **Gate:** payload JSON has `json_valid` check.
- **References:** architecture “Repository observer,” SQL contract.

## AO-006 Add exact Git commit table

- **Deliverable:** `git_commits` with exact SHA uniqueness per repository and bounded metadata fields.
- **Files:** migration 44 and tests.
- **RED:** same SHA in one repository deduplicates; same SHA in another repository is allowed.
- **GREEN:** implement table/indexes only.
- **Proof:** fixture proves reachability can update without deleting historical row.
- **Gate:** no full diff/blob/email plaintext column.
- **References:** spec §7.4-7.5.

## AO-007 Complete migration 44 version bookkeeping

- **Deliverable:** schema 43 upgrades exactly once to 44 and fresh DB includes migration record.
- **Files:** `src/db/pool.rs`, tests and schema snapshot helper.
- **RED:** schema-43 fixture open reports missing migration or wrong version.
- **GREEN:** wire migration transaction and `schema_migrations` row.
- **Proof:** fresh, upgrade, and second-open tests all pass with schema 44.
- **Gate:** integrity and foreign-key checks pass after each path.
- **References:** existing migration dispatcher and schema-version tests.

## AO-008 Implement migration 45 agent_runs

- **Deliverable:** durable `agent_runs` table and indexes.
- **Files:** `src/db/pool.rs`, tests.
- **RED:** reject invalid status; enforce host/tool/native-session uniqueness; permit nullable primary worktree.
- **GREEN:** add table and activity/status/worktree/tool indexes.
- **Proof:** query-plan test for active-status/activity list uses expected index.
- **Gate:** integer IDs remain internal; API contract uses strings later.
- **References:** SQL contract migration 45, spec §4.

## AO-009 Add actors and run/worktree evidence

- **Deliverable:** `agent_run_actors` and `agent_run_worktrees`.
- **Files:** migration 45 and tests.
- **RED:** confidence outside [0,1] and unknown trust enum are rejected; duplicate evidence tuple deduplicates.
- **GREEN:** add tables/indexes.
- **Proof:** primary-evidence ordering fixture can query confidence/trust/time deterministically.
- **Gate:** relation history permits multiple worktrees.
- **References:** contract §5.

## AO-010 Add durable run events

- **Deliverable:** `agent_run_events` with unique event key and run-order indexes.
- **Files:** migration 45 and tests.
- **RED:** duplicate event key does not create a second row; invalid event kind fails.
- **GREEN:** add table/indexes.
- **Proof:** 1,000-event fixture query returns stable observed_at/id order and indexed plan.
- **Gate:** payload JSON validated and content-scrubbed flag required.
- **References:** spec §5.

## AO-011 Add run/commit evidence and projection cursors

- **Deliverable:** `agent_run_commits` and seeded `agent_projection_cursors`.
- **Files:** migration 45 and tests.
- **RED:** expected eight cursor rows absent; duplicate run/commit evidence violates uniqueness.
- **GREEN:** add tables, indexes, and `INSERT OR IGNORE` seeds.
- **Proof:** repeated open keeps exactly eight cursor rows and preserves advanced cursors.
- **Gate:** migration must never reset cursor values.
- **References:** architecture “Source cursors.”

## AO-012 Add durable stream outbox

- **Deliverable:** `agent_stream_outbox` with expiry and run indexes.
- **Files:** migration 45 and tests.
- **RED:** invalid stream event rejected; deleting run cascades its scoped outbox rows.
- **GREEN:** add table/indexes.
- **Proof:** replay query by ID and expiry uses index and returns ascending order.
- **Gate:** payload JSON cap is enforced later by write helper, not schema.
- **References:** contract §7.9.

## AO-013 Complete migration 45 bookkeeping

- **Deliverable:** fresh and 44-to-45 migrations are transactional/idempotent.
- **Files:** pool migration dispatcher/tests.
- **RED:** injected SQL failure fixture must leave schema at 44 with no partial tables.
- **GREEN:** wrap migration and version record in one transaction.
- **Proof:** failure rollback and successful retry tests pass.
- **Gate:** G1 partial schema gate through version 45.
- **References:** ADR 001 single-writer and current pool migration style.

## AO-014 Implement migration 46 OTLP span table

- **Deliverable:** `otel_spans` and required run/session/trace/service indexes.
- **Files:** `src/db/pool.rs`, tests.
- **RED:** duplicate trace/span pair deduplicates; invalid hex lengths fail.
- **GREEN:** add migration DDL/version record.
- **Proof:** fresh and 45-to-46 paths plus query-plan tests pass.
- **Gate:** span JSON columns require valid JSON.
- **References:** spec §8.2, SQL contract.

## AO-015 Implement migration 47 OTLP metric-point table

- **Deliverable:** `otel_metric_points` and indexes.
- **Files:** pool/tests.
- **RED:** unknown instrument kind and invalid JSON fail; duplicate point key deduplicates.
- **GREEN:** add DDL/version record.
- **Proof:** fresh and 46-to-47 paths pass integrity checks.
- **Gate:** no metric-name-specific columns or indexes.
- **References:** spec §8.3-8.5.

## AO-016 Add schema-43 upgrade fixture

- **Deliverable:** deterministic test fixture representing pre-feature schema 43.
- **Files:** `tests/fixtures/schema-43.sql` or existing schema-fixture location; migration tests.
- **RED:** upgrade test fails because fixture is absent.
- **GREEN:** generate fixture from known schema contract, not current live DB.
- **Proof:** fixture opens at 43, upgrades to 47, preserves seeded legacy logs/session rows.
- **Gate:** fixture contains no host/user data.
- **References:** `docs/contracts/current-schema.sql`, pool tests.

## AO-017 Add DB domain models and enum conversions

- **Deliverable:** Rust DB structs/enums for repository, worktree, run, event, evidence, span, metric, cursor, outbox.
- **Files:** new `src/db/agent_observatory.rs`, `src/db/otlp_traces.rs`, `src/db/otlp_metrics.rs`, sidecar tests, module exports.
- **RED:** round-trip enum/string tests and invalid-value tests fail to compile.
- **GREEN:** implement strict `as_str`/parse conversions and row structs only.
- **Proof:** focused model tests pass under clippy warnings-as-errors.
- **Gate:** no query methods in this task.
- **References:** contract type fixtures and existing DB model style.

## AO-018 Add configuration types, defaults, and validation

- **Deliverable:** nested observatory/Git/stream/privacy/retention config with TOML and env overrides.
- **Files:** `src/config.rs` or existing config modules, tests, `config.toml` example only after tests.
- **RED:** default values and zero/unsafe-cap rejection tests fail.
- **GREEN:** implement serde/default/env mapping and validation; feature remains disabled by explicit default until rollout decision.
- **Proof:** TOML round-trip, env override, unknown-field, and invalid-combination tests pass.
- **Gate:** update `docs/contracts/config-schema.md` only in documentation phase after names are final. Complete mandatory compatibility tasks ENV-001 through ENV-004 immediately after this task and before heartbeat-agent/deployment configuration changes merge.
- **References:** contract §10, `01a-transcript-forward-env-rename.md`, and current config patterns.

## Phase 1 gate

Run:

```bash
just check-agent-observatory-contracts
cargo test db::pool --lib
cargo test db::agent_observatory --lib
cargo test config --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Required proof:

- fresh schema and schema-43 upgrade end at 47
- repeated open is unchanged
- `PRAGMA foreign_key_check` returns no rows
- `PRAGMA integrity_check` returns `ok`
- contract SQL table/index set matches runtime table/index set
- no approved dependency set changed
