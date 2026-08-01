# Agent Observatory implementation proof

## AO-001 Add a planning-contract verification script
commit/worktree SHA: 4b84b406 (task started)
RED: `just --justfile Justfile --working-directory . check-agent-observatory-contracts`
RED result: exit 1, `justfile does not contain recipe check-agent-observatory-contracts`
GREEN: `just --justfile Justfile --working-directory . check-agent-observatory-contracts`
GREEN result: exit 0; JSON contracts ok; SQL integrity ok; Rust contract tests 2 passed; TypeScript 5.9.3 ok; placeholder audit ok
REGRESSION: `bash -n scripts/check-agent-observatory-contracts.sh && git diff --check`
REGRESSION result: shell syntax valid and diff whitespace clean
FILES: `scripts/check-agent-observatory-contracts.sh`, `Justfile`
NOTES: TypeScript resolution is network-free and requires exact 5.9.3 from TSC, future web/node_modules, PATH, or npm's offline cache.

## AO-002 Lock schema and projection version constants
commit/worktree SHA: 06b15c04 (task started)
RED: `cargo test --manifest-path Cargo.toml --locked agent_observatory::tests --lib`
RED result: E0432 unresolved imports for `AGENT_OBSERVATORY_SCHEMA_VERSION` and `AGENT_OBSERVATORY_PROJECTION_VERSION`
GREEN: `cargo --config 'build.rustc-wrapper=""' test --manifest-path Cargo.toml --locked agent_observatory::tests --lib`
GREEN result: 2 passed; target schema 47 and projection version 1 locked; runtime schema remains below target until migrations land
REGRESSION: `cargo test --manifest-path Cargo.toml --locked known_schema_version_matches_migration_head --lib && cargo fmt --all -- --check`
REGRESSION result: runtime schema-head test passed; rustfmt and diff checks clean
FILES: `src/agent_observatory.rs`, `src/agent_observatory_tests.rs`, `src/lib.rs`
NOTES: The planned target constant is intentionally distinct from `db::KNOWN_SCHEMA_VERSION`; runtime version advances only with implemented migrations.

## AO-003 Implement migration 44 repository table
commit/worktree SHA: ef73d297 (task started)
RED: `cargo --config 'build.rustc-wrapper=""' test --manifest-path Cargo.toml --locked init_pool_creates_agent_observatory_repository_schema_scaffold --lib`
RED result: expected repository columns, got an empty list because `repositories` did not exist
GREEN: `cargo --config 'build.rustc-wrapper=""' test --manifest-path Cargo.toml --locked init_pool_creates_agent_observatory_repository_schema_scaffold --lib`
GREEN result: 1 passed; exact columns, indexes, uniqueness, reopen preservation, and no premature migration-44 marker verified
REGRESSION: `cargo test --manifest-path Cargo.toml --locked known_schema_version_matches_migration_head --lib && cargo fmt --all -- --check && git diff --check`
REGRESSION result: runtime schema remains 43; focused formatted test passed; diff clean
FILES: `src/db/pool.rs`, `src/db/pool_tests.rs`
NOTES: The repository DDL is an idempotent migration-44 scaffold. AO-007 will mark migration 44 only after worktrees, observations, and commits are complete.

## AO-004 Add repository worktree table
commit/worktree SHA: 9a7d6c25 (task started)
RED: `env CARGO_TARGET_DIR=.cache/cargo cargo --config 'build.rustc-wrapper=""' test --manifest-path Cargo.toml --locked init_pool_creates_agent_observatory_worktree_schema_scaffold --lib`
RED result: expected worktree columns, got an empty list because `repository_worktrees` did not exist
GREEN: same focused command with the worktree DDL implemented
GREEN result: 1 passed; exact columns, branch/HEAD state, host/path uniqueness, repository cascade, and empty `PRAGMA foreign_key_check` verified
REGRESSION: pinned-target `known_schema_version_matches_migration_head`, `cargo fmt --all -- --check`, and `git diff --check`
REGRESSION result: runtime schema remains 43; formatting and diff checks clean
FILES: `src/db/pool.rs`, `src/db/pool_tests.rs`
NOTES: The worktree DDL remains part of the unmarked migration-44 scaffold; all Cargo proof commands now pin `CARGO_TARGET_DIR` to this worktree to avoid cross-worktree lock pollution.

## AO-005 Add repository observations table
commit/worktree SHA: fd3d8342 (task started)
RED: pinned-target `init_pool_creates_agent_observatory_observation_schema_scaffold`
RED result: expected observation columns, got an empty list because `repository_observations` did not exist
GREEN: same focused command with observation DDL and indexes implemented
GREEN result: 1 passed; exact columns, unique observation keys, invalid JSON rejection, deterministic `(observed_at DESC, id DESC)` ordering, and named repository/worktree indexes verified
REGRESSION: pinned-target `known_schema_version_matches_migration_head`, `cargo fmt --all -- --check`, and `git diff --check`
REGRESSION result: runtime schema remains 43; formatting and diff checks clean
FILES: `src/db/pool.rs`, `src/db/pool_tests.rs`
NOTES: Query-plan proof requires `idx_repository_observations_repo_time`; migration 44 remains deliberately unmarked.

## AO-006 Add exact Git commit table
commit/worktree SHA: 58c180f8 (task started)
RED: pinned-target init_pool_creates_agent_observatory_git_commit_schema_scaffold
RED result: expected exact-commit columns, got an empty list because git_commits did not exist
GREEN: same focused command with exact-commit DDL implemented
GREEN result: 1 passed; exact columns, per-repository SHA uniqueness, cross-repository SHA reuse, JSON rejection, reachability update, and metadata-only storage verified
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt --all -- --check, and git diff --check
REGRESSION result: runtime schema remains 43; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: No patch, diff, blob, or plaintext author-email column exists; migration 44 remains unmarked until AO-007.

## AO-007 Complete migration 44 version bookkeeping
commit/worktree SHA: f50abffc (task started)
RED: pinned-target migration_44_applies_from_schema_43_and_is_idempotent
RED result: schema migration head remained 43 after the simulated schema-43 database reopened
GREEN: same focused command after wrapping all migration-44 DDL and the version marker in one BEGIN IMMEDIATE transaction
GREEN result: 1 passed; schema 43 upgraded to 44, all four tables created, legacy stream state preserved, foreign-key/integrity checks clean, and repeated reopen kept one marker
REGRESSION: pinned-target init_pool_creates_agent_observatory_ suite, known_schema_version_matches_migration_head, cargo fmt, and git diff --check
REGRESSION result: 4 topology tests passed; schema-head test passed at 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: KNOWN_SCHEMA_VERSION now advances truthfully to 44 only after the complete topology migration commits atomically.

## AO-008 Implement migration 45 agent_runs
commit/worktree SHA: 696d60c5 (task started)
RED: isolated pinned-target init_pool_creates_agent_observatory_run_schema_scaffold
RED result: expected run columns, got an empty list because agent_runs did not exist
GREEN: same focused command with the agent-runs table and four indexes implemented
GREEN result: 1 passed; lifecycle status constraints, host/tool/native-session identity, nullable primary worktree, required indexes, and active-run query-plan use verified
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt, and git diff --check
REGRESSION result: runtime schema remains 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: Migration 45 remains unmarked until actors, evidence, events, cursors, and stream outbox are complete.

## AO-009 Add actors and run/worktree evidence
commit/worktree SHA: cb6f7f77 (task started)
RED: isolated pinned-target init_pool_creates_agent_observatory_actor_and_worktree_evidence_schema
RED result: expected actor columns, got an empty list because agent_run_actors and agent_run_worktrees did not exist
GREEN: same focused command with both tables and three indexes implemented
GREEN result: 1 passed; actor identity and JSON checks, confidence/trust constraints, evidence tuple dedupe, deterministic primary ordering, multiple-worktree history, and all indexes verified
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt --all -- --check, git diff --check
REGRESSION result: runtime schema remains 44; formatting and diff clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: Migration 45 remains intentionally unmarked until AO-013.

## AO-010 Add durable run events
commit/worktree SHA: (pending)
RED: `export CARGO_TARGET_DIR=.cache/cargo cargo --config 'build.rustc-wrapper=""' test init_pool_creates_agent_observatory_run_events_schema --lib`
RED result: expected 21 event columns, got empty list because agent_run_events did not exist
GREEN: same focused command with events table and four indexes implemented
GREEN result: 1 passed; exact columns, unique event key, invalid event kind rejection, JSON validation, 1000-event fixture query plan, stable ordering, and all indexes verified
REGRESSION: `export CARGO_TARGET_DIR=.cache/cargo cargo test known_schema_version_matches_migration_head --lib && cargo fmt --all -- --check && git diff --check`
REGRESSION result: runtime schema remains 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: Migration 45 remains unmarked until AO-013; event kind CHECK constraint covers all 18 kinds from the contract.

## AO-011 Add run/commit evidence and projection cursors
commit/worktree SHA: (pending)
RED: `cargo test init_pool_creates_agent_run_commits_and_projection_cursors --lib`
RED result: expected agent_run_commits and agent_projection_cursors tables, got empty lists
GREEN: same focused command with both tables, indexes, and seeded cursor rows implemented
GREEN result: 1 passed; exact columns, relation-key uniqueness, trust/confidence constraints, 8 seeded cursors, cursor preservation across reopens, and all indexes verified
REGRESSION: `cargo test known_schema_version_matches_migration_head --lib && cargo fmt --all && git diff --check`
REGRESSION result: runtime schema remains 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: Migration 45 remains unmarked until AO-013; INSERT OR IGNORE ensures cursor values persist across reopens; 8 cursor types match contract.

## AO-012 Add durable stream outbox
commit/worktree SHA: (pending)
RED: `cargo test init_pool_creates_agent_stream_outbox --lib`
RED result: expected 7 outbox columns, got empty list because agent_stream_outbox did not exist
GREEN: same focused command with outbox table and two indexes implemented
GREEN result: 1 passed; exact columns, unique outbox key, JSON validation, cascade delete, 100-event fixture query plan, stable ordering, and all indexes verified
REGRESSION: `cargo test known_schema_version_matches_migration_head --lib && cargo fmt --all && git diff --check`
REGRESSION result: runtime schema remains 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: Migration 45 remains unmarked until AO-013; cascade delete ensures outbox cleanup when run is deleted.

## AO-013 Complete migration 45 bookkeeping
commit/worktree SHA: (pending)
RED: `cargo test migration_45_completes_transactionally_and_is_idempotent --lib`
RED result: expected upgrade from schema 44 to 45, but migration was not transactional
GREEN: wrapped migration 45 DDL and version marker in BEGIN IMMEDIATE transaction, advanced KNOWN_SCHEMA_VERSION to 45
GREEN result: 2 passed; transactional upgrade from 44 to 45, idempotent reopen, seeded cursors preserved, foreign key and integrity checks pass
REGRESSION: `cargo test known_schema_version_matches_migration_head --lib && cargo fmt --all && git diff --check`
REGRESSION result: runtime schema now at 45; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: KNOWN_SCHEMA_VERSION now truthfully at 45; migration 45 is atomic and idempotent; G1 Storage gate partially satisfied.

## AO-016 Add schema-43 upgrade fixture
commit/worktree SHA: 22626a72 (task started)
RED: pinned-target schema_43_fixture_upgrades_to_47_and_preserves_legacy_rows
RED result: compile failed because tests/fixtures/schema-43.sql did not exist
GREEN: same focused command after adding the deterministic synthetic fixture
GREEN result: 1 passed; fixture opened at schema 43, upgraded to 47, and preserved seeded legacy log and AI session-rollup rows
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt --all -- --check, and git diff --check
REGRESSION result: runtime schema-head test passed at 47; formatting and whitespace checks clean
FILES: tests/fixtures/schema-43.sql, src/db/pool_tests.rs
NOTES: Fixture SQL is generated from the migration contract, contains deterministic reserved-example values only, and explicitly rejects user/home-path leakage.

## AO-017 Add DB domain models
commit/worktree SHA: f5d722be (task started)
RED: pinned-target observatory_text_enums_round_trip_and_reject_unknown_values
RED result: unresolved imports for db::agent_observatory, db::otlp_traces, and db::otlp_metrics
GREEN: pinned-target agent_observatory_models_tests
GREEN result: 2 passed; strict enums round-trip known values, reject unknown values, and row models preserve string API keys with internal integer IDs
REGRESSION: cargo clippy --locked --lib --tests -- -D warnings, cargo fmt, and git diff --check
REGRESSION result: Clippy completed without warnings after marking staged row-only model modules as intentionally unused until projector/API integration; formatting and diff checks clean
FILES: src/db.rs, src/db/agent_observatory.rs, src/db/otlp_traces.rs, src/db/otlp_metrics.rs, src/db/agent_observatory_models_tests.rs, src/agent_observatory_tests.rs
NOTES: This task adds strict text conversions and row structs only. Query and persistence methods remain deferred to later tasks. The schema lock test now requires runtime schema 47 because migrations 44 through 47 are complete.

## AO-018 Add configuration types, defaults, and validation
commit/worktree SHA: 5564d05c (task started)
RED: pinned-target agent_observatory_defaults_are_safe_and_disabled
RED result: compile failed because Config had no agent_observatory field, AgentObservatoryConfig did not exist, and no validator or env mappings were defined
GREEN: pinned-target agent_observatory_ test filter after adding strict nested config, safe defaults, env precedence, and validation
GREEN result: 13 focused observatory config/model/storage tests passed, including disabled defaults, full TOML round-trip, unknown-field rejection, environment overrides, and unsafe-bound rejection
REGRESSION: full config::tests, runtime::tests, corrected focused validation test, cargo clippy --lib --tests -- -D warnings, cargo fmt, and git diff --check
REGRESSION result: 96 config tests passed; 20 runtime tests passed; corrected validation test passed; Clippy, formatting, and diff checks clean
FILES: src/config.rs, src/config_tests.rs, src/runtime_tests.rs, docs/contracts/config-schema.md
NOTES: Feature remains explicitly disabled by default. The nested agent_observatory block denies unknown fields and all documented CORTEX_AGENT_OBSERVATORY_* variables override TOML.

## ENV-001 Add the new resolver and compatibility alias
commit/worktree SHA: 2e22dc19 (task started)
RED: pinned-target env_new_only_true_enables_forwarding
RED result: new CORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true was ignored and forwarding remained false
GREEN: pinned-target transcript_forward_env_ test filter
GREEN result: 4 passed; precedence matrix, warning codes, authoritative replacement, legacy-only compatibility, and local sessions-watch independence verified
REGRESSION: pinned-target heartbeat_agent::tests plus cargo fmt and git diff --check
REGRESSION result: 46 passed; formatting and diff clean
FILES: src/heartbeat_agent.rs, src/heartbeat_agent_tests.rs
NOTES: The deprecated name is centralized in AI_TRANSCRIPT_FORWARD_LEGACY_ENV; from_env emits exactly one warning selected by the pure resolver.


## ENV-002 Switch all generated and deployed configuration to the new name
commit/worktree SHA: 06c64c09 (task started)
RED: setup generation, persisted env resolution, and Linux deployment fixtures expecting CORTEX_AGENT_AI_TRANSCRIPT_FORWARD only
RED result: all three failed because output preserved CORTEX_AGENT_AI_TRANSCRIPTS or omitted the replacement
GREEN: the same three focused tests after setup/deploy normalization
GREEN result: 3 passed; legacy-only persisted/process environment values are emitted under CORTEX_AGENT_AI_TRANSCRIPT_FORWARD and the legacy key is absent
REGRESSION: setup::heartbeat_agent::tests, agent_deploy::tests, cargo fmt, git diff --check, production occurrence audit
REGRESSION result: 12 setup tests and 32 deployment tests passed; the only non-test source occurrence of CORTEX_AGENT_AI_TRANSCRIPTS is the compatibility constant
FILES: src/setup/heartbeat_agent.rs, src/setup/heartbeat_agent_tests.rs, src/agent_deploy.rs, src/agent_deploy_tests.rs
NOTES: Replacement values are authoritative; legacy persisted values are normalized instead of copied verbatim, and generated files never contain both names.

## ENV-003 Add doctor detection and safe automatic migration
commit/worktree SHA: 67fea1c2 (task started)
RED: temporary env-file fixtures for legacy-only, both-equal, conflicting, authorization, permission, and idempotency behavior
RED result: doctor had no transcript-forwarding migration diagnostic or safe rewrite path
GREEN: pinned-target transcript_forward_migration_ test filter
GREEN result: 11 passed; legacy-only rename, equal-alias cleanup, conflict no-write, missing/current-only no-op, explicit --fix --yes authorization, private 0600 permissions, and idempotent second run verified
REGRESSION: pinned-target setup::doctor::tests, cargo fmt --all -- --check, git diff --check
REGRESSION result: 23 doctor tests passed; formatting and diff checks clean
FILES: src/setup/doctor.rs, src/setup/doctor_tests.rs
NOTES: Migration uses temporary-file write, sync_all, atomic rename, and parent-directory fsync. Conflicting values are never guessed or rewritten.
