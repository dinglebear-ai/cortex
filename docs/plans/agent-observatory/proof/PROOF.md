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

## AO-008 Implement migration 45 agent_runs atomically
commit/worktree SHA: 696d60c5 (task started)
RED: isolated pinned-target init_pool_creates_agent_observatory_run_schema_scaffold
RED result: expected run columns, got an empty list because agent_runs did not exist
GREEN: same focused command with the agent-runs table and four indexes implemented
GREEN result: 1 passed; lifecycle status constraints, host/tool/native-session identity, nullable primary worktree, required indexes, and active-run query-plan use verified
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt, and git diff --check
REGRESSION result: runtime schema remains 44; formatting and diff checks clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: The foundation PR removed its partial unversioned scaffold. This implementation lands agent_runs only as part of the complete, versioned migration 45 transaction.

## AO-009 Add actors and run/worktree evidence atomically
commit/worktree SHA: cb6f7f77 (task started)
RED: isolated pinned-target init_pool_creates_agent_observatory_actor_and_worktree_evidence_schema
RED result: expected actor columns, got an empty list because agent_run_actors and agent_run_worktrees did not exist
GREEN: same focused command with both tables and three indexes implemented
GREEN result: 1 passed; actor identity and JSON checks, confidence/trust constraints, evidence tuple dedupe, deterministic primary ordering, multiple-worktree history, and all indexes verified
REGRESSION: pinned-target known_schema_version_matches_migration_head, cargo fmt --all -- --check, git diff --check
REGRESSION result: runtime schema remains 44; formatting and diff clean
FILES: src/db/pool.rs, src/db/pool_tests.rs
NOTES: The foundation PR proved partial migration-45 tables stayed absent. This implementation adds all migration-45 tables and advances the schema head in one atomic migration.

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

## PR-173 adversarial remediation
RED: review beads `syslog-mcp-2axwk`, `syslog-mcp-e5o51`, `syslog-mcp-ftiu1`, and `syslog-mcp-34jai`
RED result: startup created an unversioned migration-45 subset; doctor inspected only the final duplicate, replaced symlinks, and could overwrite a concurrently changed file; validation allowlisted whole files and ignored extensionless tracked text.
GREEN: focused database and doctor tests plus both transcript-forward validator scripts
GREEN result: migration-45 tables remain absent; ambiguous legacy/duplicate configuration fails closed for manual editing; conflicting duplicates and symlinks fail without mutation; tracked occurrence validation rejects both extensionless fixtures and invalid occurrences inside an otherwise approved file.
FILES: `src/db/pool.rs`, `src/db/pool_tests.rs`, `src/setup/doctor.rs`, `src/setup/doctor_tests.rs`, `scripts/validate-transcript-forward-env-rename.sh`, `scripts/test-validate-transcript-forward-env-rename.sh`, `Justfile`
NOTES: Contract, OpenAPI, schema, Rust/TypeScript type, architecture, research, specification, validator, and golden-fixture artifacts were resolved to the versions already validated and merged through PR #172.

## PR-173 independent re-review remediation
RED: review beads `syslog-mcp-afx03` and `syslog-mcp-ya4a7`
RED result: reread-before-rename still allowed a noncooperative edit or symlink swap in the final compare/replace window; documentation context accepted the substring `red` inside unrelated words such as `configured`.
GREEN: fail-closed doctor migration plus injected post-read mutation tests; whole-word documentation context grammar plus an executable-assignment rejection and code-fence negative fixture.
GREEN result: no automatic rewrite path remains, so forced noncooperative edits and symlink swaps are preserved; executable legacy assignments are rejected even in allowlisted documentation, and unrelated substrings grant no exception.
REGRESSION: post-merge privacy scanner and adversarial fixtures, render-template hostile-input test, Agent Observatory contracts, transcript validator fixtures, doctor tests, workflow/Kache contracts, full clippy, and full nextest.
REGRESSION result: all hermetic gates passed; full nextest ran 2,754 tests with 2 skipped and no failures. The live deployment-host check requires a deployment-local `hosts.env`, which is intentionally absent from this worktree.
FILES: `src/setup/doctor.rs`, `src/setup/doctor_tests.rs`, `src/setup/doctor_transcript_forward_tests.rs`, `scripts/validate-transcript-forward-env-rename.sh`, `scripts/test-validate-transcript-forward-env-rename.sh`
NOTES: Current `origin/main` through PR #174 was merged without conflicts, preserving the privacy scanner/config/runbook changes and the validated PR #172 Agent Observatory artifacts.

## ENV-003 Keep legacy environment migration fail-closed
RED: legacy-only and conflicting environment fixtures exercised the setup doctor
RED result: earlier implementation could rewrite a deployment file under an automated fix path
GREEN: doctor reports the deprecated alias and replacement guidance without modifying the file
GREEN result: legacy-only, equal-alias, and conflicting states remain byte-for-byte unchanged; no automatic write path exists
REGRESSION: setup doctor tests, transcript-forward rename validator, private-identifier scan, cargo fmt, and git diff --check
REGRESSION result: compatibility reads remain supported while deployment mutation requires an explicit operator edit
FILES: src/setup/doctor.rs, src/setup/doctor_tests.rs, scripts/validate-transcript-forward-env-rename.sh
NOTES: ENV-003 is intentionally fail-closed. Detection and guidance are safe; Cortex does not auto-write deployment environment files.

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

## AO-014 Implement migration 46 OTLP span table
commit/worktree SHA: bffe249c (independent compliance verification started)
RED: replace the shallow table-existence check with the complete migration-46 SQL contract fixture
RED result: the expanded test initially failed to compile at its JSON fixture, proving the new contract path had not previously been exercised
GREEN: pinned-target migration_46_creates_otel_spans_table_and_indexes
GREEN result: exact columns, four indexes, trace/span dedupe, identifier lengths, duration/JSON/scrub constraints, deterministic run ordering, run and trace query plans, run-FK SET NULL, integrity, and idempotent reopen all passed
REGRESSION: known_schema_version_matches_migration_head, cargo fmt --all -- --check, git diff --check
REGRESSION result: schema head remains 47; formatting and diff checks clean
FILES: src/db/pool_tests.rs, docs/plans/agent-observatory/proof/PROOF.md
NOTES: Migration-46 DDL originally landed in the combined 22626a72 runner commit; this independent task commit locks the full contract without rewriting later unpublished descendants.

## AO-015 Implement migration 47 OTLP metric-point table
commit/worktree SHA: cebcafb6 (independent compliance verification started)
RED: replace the shallow table-existence check with the complete migration-47 SQL contract fixture
RED result: the expanded test initially failed to compile because its JSON value fixture lacked Rust string escaping
GREEN: corrected the fixture and reran migration_47_creates_otel_metric_points_table_and_indexes in an isolated Cargo target
GREEN result: 1 passed; exact columns and all three indexes, point-key dedupe, instrument/JSON/boolean constraints, deterministic ordering, run/name query plans, run deletion preservation, integrity, foreign keys, and idempotent reopen verified
REGRESSION: known_schema_version_matches_migration_head, cargo fmt --all -- --check, git diff --check
REGRESSION result: schema head test passed at 47; formatting and diff checks clean
FILES: src/db/pool_tests.rs
NOTES: The original migration DDL remains unchanged; this task adds full independent contract proof after AO-014 and AO-015 were originally combined in one implementation commit.

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

## AO-019 Implement versioned identity helpers
commit/worktree SHA: ccfd024c (task started)
RED: sidecar contract vectors imported canonical tool, run, repository, worktree, actor, and event identity helpers before the module exposed them
RED result: compile failed with unresolved imports for every required helper, IdentityError, and MAX_EVENT_KEY_BYTES
GREEN: focused identity contract suite after implementing version-one length-prefixed identities and deterministic event-key validation
GREEN result: 10 focused tests passed, covering the frozen run-key example, UTF-8 byte lengths, delimiter-bearing components, whitespace trimming, unknown-tool normalization, nested actor keys, strict ASCII lower snake case, exact 1024-byte acceptance, 1025-byte rejection, empty components, and repeated deterministic generation
REGRESSION: Agent Observatory contracts, complete agent_observatory module tests, workspace Clippy with warnings denied, formatting, diff check, and dependency-lock audit
REGRESSION result: JSON/SQL/Rust/TypeScript contract checks passed; 12 Agent Observatory tests passed; Clippy, rustfmt, git diff --check, and unchanged Cargo.toml/Cargo.lock gates passed
FILES: src/agent_observatory.rs, src/agent_observatory/identity.rs, src/agent_observatory/identity_tests.rs
NOTES: Unknown provider labels are trimmed and Unicode-lowercased under unknown:<source>; original provider labels remain a future metadata concern. No random identity or new dependency was introduced.

## AO-020 Implement lifecycle reducer
commit/worktree SHA: 96451127 (task started)
RED: fixed-time lifecycle tests imported the reducer evidence, window, wait, reason, state, and decision surfaces before implementation
RED result: compile failed with unresolved imports for every required reducer type and reduce_lifecycle
GREEN: focused pure reducer suite with fixed 2026-08-02T16:00:00Z clock and no sleeps
GREEN result: 12 tests passed; explicit failure and success precedence, permission/tool waits, starting without activity, exact active/idle/stale/abandoned boundary seconds, process-live and unavailable behavior, future timestamp clamping, stable observed_at/write suppression, transition timestamps, default windows, and all frozen reason codes are covered
REGRESSION: complete Agent Observatory module tests, workspace Clippy with warnings denied, formatting, diff check, and dependency-lock audit
REGRESSION result: all 24 Agent Observatory tests passed; Clippy, rustfmt, git diff --check, and unchanged Cargo.toml/Cargo.lock gates passed
FILES: src/agent_observatory.rs, src/agent_observatory/lifecycle.rs, src/agent_observatory/lifecycle_tests.rs
NOTES: The reducer is pure and receives now explicitly. Unavailable process evidence remains stale rather than terminal, and unchanged status/reason pairs preserve observed_at to avoid poll-driven writes.

## AO-021 Implement attribution scoring and primary selection
commit/worktree SHA: 1a97b963 (task started)
RED: attribution sidecar tests imported the candidate, kind, default, validation, trust-rank, selection, and threshold surfaces before implementation
RED result: compile failed with unresolved imports for every required attribution API
GREEN: focused scoring suite after implementing frozen defaults, validation, latest-source refutation, stronger-evidence recovery, deterministic sorting, per-worktree deduplication, and the 0.75 primary threshold
GREEN result: 11 tests passed; exact defaults and trust rank, confidence-first order, trust/time/worktree tie-breaks, below-threshold behavior, timestamp-only exclusion, refutation blocking/recovery, strongest evidence per worktree, 128 shuffled orders, and invalid candidate rejection are covered
REGRESSION: complete Agent Observatory module tests, workspace Clippy with warnings denied, formatting, diff check, and dependency-lock audit
REGRESSION result: all 35 Agent Observatory tests passed; Clippy, rustfmt, git diff --check, and unchanged Cargo.toml/Cargo.lock gates passed
FILES: src/agent_observatory.rs, src/agent_observatory/attribution.rs, src/agent_observatory/attribution_tests.rs
NOTES: Refutation is scoped to worktree plus evidence source. Recovery requires evidence that is both newer and strictly stronger. Timestamp proximity remains stored evidence but is excluded from primary ranking.

## AO-022 Add repository/worktree DB upserts
commit/worktree SHA: cd3c02a7 (task started)
RED: repository reconciliation tests imported repository/worktree upsert inputs, transactional reconcile/read helpers, and removal helpers before implementation
RED result: compile failed with unresolved imports for every AO-022 persistence surface
GREEN: added parameterized transactional repository/worktree upserts, deterministic reads/lists, immutable identity checks, canonical absolute-path validation, active-set removal, and reappearance clearing while preserving IDs and first_seen timestamps
Proof: create/update/remove/reappear, transaction rollback, path rejection, and SQL-metacharacter fixtures all pass; 2 model tests and 56 schema/migration tests pass
Gate: workspace Clippy with -D warnings, rustfmt, diff check, 500-line module limit, no formatted SQL call sites, and unchanged Cargo manifests all pass

## AO-023 Parse worktree porcelain -z
commit/worktree SHA: b73e6663 (task started)
RED: real Git 2.53.0 byte fixtures and malformed-input tests imported the worktree record, unknown-field, typed-error, size-limit, and parser surfaces before implementation
RED result: compile failed with unresolved imports for every AO-023 porcelain parser surface
GREEN: added a NUL-safe byte parser for normal, detached, locked, prunable, and bare records with SHA-1/SHA-256 validation, duplicate/state checks, future-field retention, and bounded errors
Proof: 9 parser tests pass against worktrees.bin sha256 ffac43a52ba54a25d859c852b5b747ee038022b0d0b77adb5b2cc783e75e3e5b and bare.bin sha256 1ef3fc47b4d104cb2c1a60f12c677d017cce2b98efa3a2b4cda2ee288e3fbc33; non-UTF-8 paths/branches/reasons survive unchanged
Gate: workspace Clippy with -D warnings, rustfmt, diff check, fixture checksum verification, module-size limit, and unchanged Cargo manifests all pass

## AO-024 Parse status porcelain v2 -z
commit/worktree SHA: d41b840d (task started)
RED: real Git 2.53.0 clean, dirty, rename, conflict, detached, no-upstream, and diverged byte fixtures imported the status summary, typed error, and parser surfaces before implementation
RED result: compile failed with unresolved StatusSummary, StatusParseError, StatusParseErrorKind, and parse_status_porcelain_v2 imports
GREEN: implemented NUL-safe porcelain-v2 branch/header parsing, ahead/behind counts, tracked/unmerged/rename/untracked/ignored counting, rename source consumption, and non-UTF-8 pathname handling without returning filenames
Proof: 19 combined porcelain tests passed; all seven checked-in status fixtures matched regenerated temporary-repository output byte-for-byte and their SHA-256 checks passed
Gate: workspace Clippy passed with -D warnings; format, diff, 500-line module-size, and Cargo manifest/lock checks passed; exhaustive StatusSummary destructuring proves no pathname field is persisted

## AO-025 Add deterministic temporary Git fixture builder
commit/worktree SHA: 12ad7234 (task started)
RED: deterministic fixture smoke tests imported GitFixture and git_available before implementation
RED result: compile failed because the fixture helper surface was absent
GREEN: real system Git fixture builds root/main/feature/reset/rebase commits, linked and detached worktrees, and a locked linked worktree under isolated HOME/XDG/GIT_CONFIG_GLOBAL/GIT_CONFIG_NOSYSTEM settings
verification: 3 fixture tests passed; repeated builds matched exact SHA vectors a4600ca60e26420e56b54374401fd23ccd4a208d, 3deaf115eb2df48b835b5b706d626640b33230d2, f6a7405024dfb8c42a20bc675fe9093e0bc767fc, 5e5c810eaec0f70f0745db09c1299b2766bb6c81, 96c48c2090c90ff0997e9cecc686a636240df3fb, and ee987310aaf16c3916a9c5d033ecd21dd0d143b5; all 19 porcelain tests passed
quality: workspace Clippy passed with -D warnings; formatting, diff, module-size, and Cargo manifest/lock gates passed
isolation: fixture command environment sets isolated HOME and XDG_CONFIG_HOME, disables global/system Git config, hooks, and signing; global sentinel config remained unchanged

## AO-026 Implement bounded repository discovery
commit/worktree SHA: e68d95bf (task started)
RED: discovery tests imported options, typed warnings, result rows, and the discovery entry point before implementation
RED result: compile failed with unresolved DiscoveryOptions, DiscoveryWarning, DiscoveryWarningKind, and discover_repositories
GREEN: iterative canonical traversal discovers directory and linked-worktree file markers; skips nested symlinks and ignored .git/.cache/cache/node_modules/target trees; enforces inclusive depth and repository caps; reports missing-root, permission, symlink, depth, and cap warnings deterministically
GREEN result: 8 focused discovery tests passed
REGRESSION: 19 porcelain tests and 3 deterministic Git fixture tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-027 Implement one-repository reconciliation
commit/worktree SHA: da6b2c38 (task started)
RED: append-only observation tests imported RepositoryObservationInput, list_repository_observations, and record_repository_observations_if_changed before implementation
RED result: compile failed with all three observation surfaces unresolved
RED: real-Git reconciliation tests imported the command runner, options, report stages/warnings, and both reconcile entry points before implementation
RED result: compile failed with GitCommandResult, GitCommandRunner, ProcessGitRunner, ReconcileOptions, ReconcileStage, ReconcileWarningKind, reconcile_one_repository, and reconcile_one_repository_with_runner unresolved
GREEN: bounded Git commands collect common dir, worktree porcelain, status v2, HEAD, branch, and optional upstream divergence before any database mutation; topology and append-only discovered/status/head observations persist with deterministic transition keys and unchanged-state suppression
GREEN result: 3 real reconciliation tests and 2 observation persistence tests passed, including second-reconcile last_seen updates, dirty/head transitions, transactional observation rollback, and timeout preservation of prior rows
REGRESSION: all 33 Git observer tests, 4 repository/worktree query tests, 2 Agent Observatory model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-028 Detect removed and reappeared worktrees
commit/worktree SHA: 6930a337 (task started)
RED: real linked-worktree lifecycle fixture removed a locked worktree and expected one worktree_removed observation while preserving the row identity and run/evidence references
RED result: reconciliation marked the row removed but inserted zero lifecycle observations
GREEN: compare pre-reconcile rows with the transactional active-set result, emit repeatable worktree_removed/worktree_added transitions, and clear removal on same host/path reappearance while preserving ID, first_seen, and last_seen history
GREEN result: remove, quiet repeat reconcile, re-add at the same path, and second remove all passed; lifecycle order was removed/added/removed with one transition per state change
GATE: agent_runs.primary_worktree_id and agent_run_worktrees evidence remained intact; PRAGMA foreign_key_check returned no violations
REGRESSION: 4 reconciliation tests, 2 observation persistence tests, all 34 Git observer tests, 4 repository/worktree query tests, 2 Agent Observatory model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-029 Parse exact commit metadata
commit/worktree SHA: 6a8724b7 (task started)
RED: real Git and synthetic byte-stream tests imported COMMIT_SHOW_FORMAT, CommitParseOptions, CommitParseErrorKind, commit_show_arguments, and parse_commit_show before implementation
RED result: compile failed with the complete commit parser surface unresolved
GREEN: defined one bounded machine-only git show format using NUL-delimited metadata plus --numstat -z; parser preserves arbitrary path bytes, parses parent lists and RFC3339 times, aggregates text/binary changes, supports rename old/new paths, and applies author/path privacy options
GREEN result: 6 focused tests passed for real merge/binary/rename commits, synthetic non-UTF8 paths, privacy suppression, path truncation, invocation hardening, and bounded malformed errors
GATE: command builder includes --no-walk=unsorted, --diff-merges=first-parent, --find-renames, --numstat, -z, --no-ext-diff, and --no-textconv; no patch/blob option is emitted and plaintext email is never returned
REGRESSION: all 40 Git observer tests, 2 Agent Observatory model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-030 Import fast-forward HEAD transitions
commit/worktree SHA: 6da7c751 (task started)
RED: byte-runner, transactional commit persistence, and real two-commit fast-forward reconcile fixtures imported APIs before implementation
RED result: compile failed with run_command_bytes_capped, GitCommitUpsert/get/list/upsert helpers, expanded reconcile options/report, and bounded commit traversal surfaces absent
GREEN: added capped raw-byte process output while preserving the existing text API; added validated transactional git_commits upserts with stable IDs/first_observed_at and input-order returns; added bounded merge-base/rev-list --reverse enumeration and exact machine-format batch metadata import before topology mutation
GREEN result: raw non-UTF8 stdout was preserved, two exact commits imported in chronological order with parent metadata, repeated reconcile stayed idempotent, and commit rows retained deterministic identities
GATE: max_commits_per_transition uses --max-count=limit+1, returns CommitLimitReached, imports no partial rows, records no HEAD observation, and leaves the prior worktree HEAD unchanged
REGRESSION: 4 process tests, 2 commit DB tests, all 42 Git observer tests, 4 repository/worktree query tests, 2 Agent Observatory model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-031 Handle rewind, reset, rebase, and detached transitions
commit/worktree SHA: 8d9c4bf3 (task started)
RED: transactional reachability and real Git reset/rebase/detached fixtures imported new APIs and expected discarded commits to become unreachable without deleting historical rows
RED result: compile failed with GitCommitReachabilityUpdate and reconcile_git_commits absent; after the DB seam was added the real reset fixture failed because the discarded commit remained reachable
GREEN: added atomic commit upsert plus reachability updates, bounded two-sided old..new and new..old traversal, ancestry classification into fast_forward/rewind/rewrite, exact metadata import for new and displaced commits, repository-wide current-head reachability, and enriched non-fast-forward HEAD observations
GREEN result: hard rewind, divergent rebase, and detached rewind preserved historical commits, updated the current worktree HEAD, toggled reachability correctly, recorded deterministic transition counts/kinds, and remained quiet on unchanged reconcile
GATE: the recording runner observed no reset, rebase, checkout, switch, commit, or merge command issued by Cortex
REGRESSION: 4 process tests, 3 commit DB tests, all 43 Git observer tests, 4 repository/worktree query tests, 2 Agent Observatory model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-032 Build Git watch-set planner
commit/worktree SHA: c178f95f (task started)
RED: pure planner tests imported repository/worktree watch inputs, deterministic targets, bounded error taxonomy, and plan_watch_set before implementation
RED result: compile failed with all watch planner surfaces unresolved
GREEN: added a filesystem-independent BTree-backed planner for canonical project roots, common Git HEAD/index/packed-refs/refs/worktrees controls, and per-worktree control directory HEAD/index paths with repository association and deterministic deduplication
GREEN result: the exact three-worktree fixture produced 13 sorted unique targets; reversing worktree order and duplicating roots produced the identical plan
PROOF: creating 10,000 source files did not change target count or content because production planning performs no filesystem traversal
GATE: unique-path cap failed at observed 13 for limit 12; relative/parent paths, out-of-root worktrees, out-of-common control dirs, blank keys, duplicate keys, and zero caps were rejected with bounded errors
REGRESSION: all 5 watch planner tests and all 48 Git observer tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-033 Implement debounced Git watcher queue
commit/worktree SHA: b1b1b654 (task started)
RED: watcher queue tests imported the event/action/options/error/sender/queue surfaces and git_watch_channel before implementation
RED result: compile failed with every queue type and entry point unresolved
GREEN: added a bounded Tokio mpsc sender, atomic overflow flag, longest-prefix WatchPlan routing, BTree-backed repository/discovery pending maps, injected-Instant debounce polling, and deterministic action ordering
GREEN result: 8 focused queue tests passed, including 100-event coalescing at the last-event deadline, two-repository ordering, project-root discovery, linked-control creation routing, unrelated-path suppression, explicit rescan, channel overflow, and pending-map overflow
GATE: overflow emits exactly one FullReconcile and clears buffered/pending work; queue and pending limits reject zero; all test timing uses injected Instant with no sleeps
REGRESSION: 13 watcher tests, all 56 Git observer tests, and 162 config tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, Cargo.toml/Cargo.lock no-diff gate, and corrected no-sleep static audit passed

## AO-034 Add overflow and periodic repair reconcile
commit/worktree SHA: 54dbe32a (task started)
RED: supervisor tests imported atomic handle, options, scheduled actions, reason/error types, and git_watch_supervisor before implementation
RED result: compile failed with every supervisor type and constructor unresolved
GREEN: added an injected-Instant supervisor with atomic overflow coalescing, one in-flight full reconcile, 60-second overflow and periodic defaults, combined overflow/periodic scans, direct-action passthrough, and failure requeue
GREEN result: 6 focused supervisor tests passed; 100 overflow notifications produced one scan per interval, periodic ticks repaired missed state, skipped intervals coalesced, and same-tick overflow/periodic work produced one combined scan
GATE: failed full reconcile requeued bounded repair while direct repository actions continued; completion and zero/overflowing interval errors are typed; all scheduler tests use injected time with no sleeps
REGRESSION: 19 watcher tests, all 62 Git observer tests, and 162 config tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, Cargo.toml/Cargo.lock no-diff gate, and no-sleep static audit passed

## AO-035 Add run/event/evidence DB write transaction
commit/worktree SHA: 5155601e (task started)
RED: transaction tests imported run/actor/worktree-evidence/event/outbox inputs, result rows, write_agent_projection, and an after-event-insert fault seam before implementation
RED result: compile failed with every projection write type and entry point unresolved
GREEN: added one BEGIN IMMEDIATE transaction that constructs deterministic run/actor/event/evidence/outbox keys, null-safe upserts materialized state, inserts events conflict-safely, updates run event/error/source counters only for a new event, and emits outbox only for a material change
GREEN result: 3 focused tests passed; injected failure after event insert left zero run/actor/evidence/event/outbox rows, retry created exactly one of each, exact replay preserved IDs and emitted no outbox, and a status-only update emitted one new outbox without double-counting the event
GATE: missing worktree resolution rolled back the entire transaction; first/last source log IDs and last_event_id matched the inserted event; deterministic identity helpers produced the expected run/actor/event keys
REGRESSION: all 14 Agent Observatory DB tests, 2 model tests, and 56 database/schema tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, Cargo.toml/Cargo.lock no-diff gate, and fixed-string no-random identity audit passed

## AO-036 Project transcript log rows
commit/worktree SHA: 84b890de (task started)
RED: classifier/projector tests imported transcript caps, classification/diagnostic types, classify_transcript_log, projection outcome, and project_transcript_log before implementation
RED result: whole-lib compile failed with every classifier and projector surface unresolved
GREEN: added a pure canonical-log classifier for Claude/Codex/Gemini plus an atomic transcript projector using existing ai_tool/project/session/transcript_path fields, 64 KiB UTF-8-safe message and metadata caps, normalized project paths, deterministic logs-derived event keys, and no guessed worktree
GREEN result: Claude two-row fixture produced one durable run with two events, Codex and Gemini produced one run/event each, expected run/event keys and timestamps matched, and exact replay emitted no event or outbox
FIX: out-of-order replay exposed run-state rewind; projection run upsert now preserves earliest start, latest activity/status/provider metadata, and only fills historical links without erasing them
GATE: missing session, unsupported provider, and malformed metadata returned typed skip diagnostics with zero writes; classifier/projector contain no scanner or filesystem parser calls
REGRESSION: all 64 Agent Observatory tests, all Agent Observatory DB tests, 56 database/schema tests, AI project normalization tests, and 162 config tests passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, Cargo.toml/Cargo.lock no-diff gate, provider-file static audit, and 64 KiB <= 256 KiB cap audit passed

## AO-037 Project agent commands and shell history
commit/worktree SHA: 4a2887ea (task started)
RED: command classifier/projector tests imported source classifications, typed skip diagnostics, projection outcomes, worktree/run lookups, and agent-command/Atuin projection entry points before implementation
RED result: whole-lib compile failed with the AO-037 classifier, projector, and read-side lookup surfaces unresolved
GREEN: added pure classifiers for canonical agent-command rows plus local and forwarded Atuin shapes, then projected them atomically into command or shell_history events with preserved scrubbed command, severity, exit status, duration, sessions, and cwd evidence
GREEN result: 5 classifier tests and 3 projector tests passed; verified agent-command cwd created or enriched one deterministic run, claimed Atuin cwd attached only to one overlapping run, exact replay emitted no duplicate event/outbox, and absent or ambiguous runs skipped without writes
FIX: Atuin now emits the distinct shell_history event/outbox kind; finished_at ordering compares parsed RFC3339 instants; nested cwd values resolve to the longest active worktree ancestor while sibling path-prefix collisions remain unmatched
GATE: agent-command cwd evidence is verified at 0.98 and may select primary; Atuin cwd-window evidence is claimed at 0.85 and never overrides the verified primary worktree; malformed, inconsistent, unsupported, or unscrubbed rows return typed diagnostics
REGRESSION: all 72 Agent Observatory tests passed, including identity, lifecycle, attribution, transcript projection, command/shell projection, DB projection transactions, migrations, and configuration contracts
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-038 Project MCP, hook, skill, and LLM sources
commit/worktree SHA: 61e9d05e (task started)
RED: source-page and projector tests imported typed MCP, hook, skill, and LLM rows, bounded pages, stable cursors, projection outcomes, skip diagnostics, and project_agent_source before implementation
RED result: whole-lib compile failed with the source paging, unique session lookup, and projector surfaces unresolved; all four durable source fixtures were absent from the run timeline
GREEN: added bounded ascending pages for mcp_events, hook_events, skill_events, and llm_invocations plus atomic typed event, actor, evidence, and outbox projection using existing tool, session, project, and host fields
GREEN result: 2 source-page tests and 2 projector tests passed; all four source kinds emitted exact canonical source IDs and typed event kinds, shared one deterministic run, created source actors, and replayed without duplicate events or outbox rows
FIX: LLM pagination uses a stable started_at plus durable invocation ID cursor and remains correct across VACUUM; MCP tool, hook source, skill plugin, and LLM provider/model remain actor and payload data while the run provider_tool remains the owning AI provider
GATE: pages are bounded to 1 through 500 rows; malformed cursors fail; unknown event labels use typed fallback; payloads are capped to 16 KiB with 4 KiB source-field caps; missing sessions and absent or ambiguous matching runs return typed skip diagnostics
GATE: transcript-derived project paths create verified transcript_project_path evidence at 0.95; source actors preserve MCP server/tool, hook name, skill plugin/name, and LLM provider/model identities
REGRESSION: all 76 Agent Observatory tests passed, including source paging, replay idempotence, VACUUM-stable cursors, provider identity preservation, projection transactions, schema migrations, attribution, lifecycle, command, shell, and transcript paths
GATE: workspace Clippy passed with -D warnings; cargo fmt --check, git diff --check, 500-line module-size gate, Agent Observatory JSON/SQL/TypeScript/placeholder contracts, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-039 Implement transactional source cursors and projector loop
commit/worktree SHA: 0ebb3d2b (task started)
RED: crash/replay, wakeup, bounded-page, retry-health, late LLM completion, and cursor migration tests exercised the projector before source materialization and durable cursor advancement were one atomic operation
RED result: source skips could leave cursors pinned, projector retries did not distinguish retry-safe SQLite lock failures, LLM started-at paging could skip an invocation that completed late, and steady-state cursor reads performed a hidden SQLite write
GREEN: source projection and cursor advancement now commit in the same transaction; committed ingest wakes the projector with poll fallback; retry-safe SQLite busy/locked failures reuse Cortex's bounded 25/100/250 ms backoff while persistent faults fall back to the configured poll interval
GREEN result: terminal LLM rows page by finished_at plus durable invocation ID; migration 48 adds the supporting partial index and preserves legacy source cursor names; legacy running LLM events are consumed without weakening immutable collision detection for final events
FIX: existing projection-cursor reads are read-only after one-time initialization, removing an INSERT OR IGNORE from every projector/status read and eliminating unnecessary global SQLite write-lock pressure
GATE: source pages remain bounded to 1..=500 rows and the runtime enforces the configured byte cap, cancellation leaves durable cursors consistent, non-retryable failures do not advance cursors, and committed log ingestion wakes before the long fallback poll
REGRESSION: definitive Agent Observatory focused binary ran 95 tests with 0 failures; projection DB suite ran 10 tests with 0 failures; Git observer suite ran 62 tests with 0 failures; migration-48 upgrade regression passed
GATE: workspace Clippy passed with -D warnings; cargo fmt --all -- --check, git diff --check, production Rust 500-line module-size gate, Agent Observatory golden contracts, and Cargo.toml/Cargo.lock no-diff gate passed
NOTES: schema head is now 48. The old schema-47 plan references were updated where they described the current/final schema; historical RED/GREEN proof entries remain historical.

## PR-160 final adversarial remediation
RED: beads `syslog-mcp-kq015`, `syslog-mcp-f5p26`, `syslog-mcp-de0oi`, and `syslog-mcp-ce3tq`
RED result: equal-timestamp mutable projections could oscillate, older metadata replay could report a change and emit outbox, hostname provenance was not scrubbed, and an oversized first page row wedged its cursor while reporting healthy.
GREEN: deterministic total-order freshness tie-breaks, monotonic SQL update predicates, all-field provenance scrubbing, and explicit oversized-first-row consumption with durable health detail.
GREEN result: A-B-A convergence, older true no-op, secret-negative provenance, and first-row-over-cap cursor/health tests pass alongside legitimate equal-time command and four-source projector workflows.
REGRESSION: Agent Observatory focused suite, runtime worker suite, private-identifier gates, Clippy with warnings denied, and full nextest.
REGRESSION result: 86 focused tests passed; full nextest ran 2,930 tests with 2 skipped, one slow, and no failures.
FILES: `src/db/agent_observatory_projection_sql.rs`, `src/db/agent_observatory_projection_tests.rs`, `src/agent_observatory/classifier.rs`, `src/agent_observatory/classifier_tests.rs`, `src/runtime/agent_observatory.rs`, `src/runtime/agent_observatory_tests.rs`
NOTES: Equal timestamps are resolved by a stable mutable-state fingerprint, not arrival order. Byte limits allow exactly one oversized first row so cursor progress remains bounded and observable.

## AO-040 Add resumable backfill and exact Git commit attribution
commit/worktree SHA: dcd69957 (task started)
RED: no durable resumable backfill/job progress, no run-to-commit persistence, and no post-reconcile exact commit attribution path; legacy HEAD observations also lacked enough transition detail to repair multi-commit history deterministically
RED result: new backfill and attribution fixtures initially had no engine/DB surfaces; early integration runs exposed live-cursor fixture setup and canonical provider-field mismatches before the intended invariants could be proven
GREEN: added one-snapshot fixed high-water capture, independent per-source maintenance-job cursors, bounded resumable pages that never mutate live projector cursors, exact new/displaced SHA persistence on HEAD observations, legacy commit-graph range reconstruction, and scorer-backed run-to-commit relations with provenance
GREEN result: interrupt/reopen/resume matched uninterrupted materialized state; post-high-water live rows stayed owned by the live projector; two exact Git commits linked at verified 0.98 command-cwd confidence, replay remained idempotent, relation deletion was repaired by backfill, and rewind preserved historical links
FIX: adversarial review added repository-consistency validation for worktree/commit relations, prevented future run/evidence activity from retroactively activating stale historical runs, made high-water capture one SQLite read snapshot, fixed the runtime health/cursor assertion race, and split DB types into ownership sidecars instead of weakening the 500-line production-module gate
REGRESSION: final harness ran 102 Agent Observatory tests, 63 Git observer tests, and 10 projection DB tests with 0 failures; focused attribution ran 4/4, backfill 3/3, runtime health 1/1, and real-Git attribution/backfill repair 1/1
GATE: workspace Clippy passed with -D warnings; cargo fmt --all -- --check, git diff --check, full 500-line Rust production-module gate, Agent Observatory golden contracts, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-041 Extract shared OTLP normalization
commit/worktree SHA: d356739c (task started)
RED: OTLP provider/session/project normalization lived only inside the log adapter, omitted `gen_ai.conversation.id`, had no `gen_ai.agent.name` or service fallback, and exposed no reusable deterministic bounded-attribute representation for traces/metrics
GREEN: extracted `src/otlp/normalization.rs` with the frozen signal-before-resource session/project precedence, explicit/agent/service tool precedence, known Claude/Codex/Gemini aliases, Unicode-safe idempotent `unknown:` normalization, and shared secret-redacting bounded metadata conversion
COMPAT: `/v1/logs` intentionally retains its prior explicit-only `ai_tool` behavior and historical 128-field nested log-attribute view, while the shared signal representation supports the 256-attribute Agent Observatory contract for upcoming spans/metrics
FIX: adversarial review made over-limit attribute selection deterministic with `BTreeMap`, bounded total normalized tool length rather than only source length, preserved unknown attributes while redacting sensitive values, generalized the metadata sanitizer to an explicit field cap, and removed a future-only dead helper instead of suppressing `dead_code`
PROOF: 8 shared-normalization tests, 22 existing/new OTLP log-entry compatibility tests, and 5 metadata-sanitizer tests passed; the complete OTLP library regression ran 50 tests with 0 failures
GATE: workspace Clippy passed with `-D warnings`; `cargo fmt --all -- --check`, `git diff --check`, full 500-line production Rust module-size gate, Agent Observatory golden contracts, and Cargo.toml/Cargo.lock no-diff gate passed

## AO-042 Decode one OTLP trace span
commit/worktree SHA: c5958c93 (task started)
RED: the pinned `opentelemetry-proto` dependency exposed logs only, migration-46 had a row scaffold but no write-input type, and there was no trace-span converter for IDs/times/status/provider/resource/scope normalization
GREEN: enabled the already-pinned 0.32 `trace` message feature, added `OtelSpanInput`, and implemented a pure one-span converter with exact 16-byte trace / 8-byte span IDs, optional parent ID, checked nanosecond-to-SQLite integer conversion, duration, flags, raw enum integers, status, shared provider/session/project normalization, and bounded resource/scope/span metadata
FIX: adversarial review moved core ID/time rejection ahead of JSON work, rejects all-zero IDs and over-limit resource/scope/span attributes with typed errors, preserves unknown future span-kind/status integer values, enforces serialized metadata and API-shaped string bounds, and leaves `content_scrubbed=false` until AO-043 applies the configurable prompt/tool/user/path privacy policy rather than falsely claiming full scrubbing
COMPAT: events/links remain empty arrays by design because AO-043 owns their bounded/privacy-aware serialization; the authenticated `/v1/traces` route remains the existing not-supported response until AO-045, proven by the broad OTLP regression
PROOF: 7 focused trace normalization tests passed, including exact IDs/times/status/resource/scope context, root optionals, malformed/zero IDs, integer/time failures, attribute caps, future enums, metadata limits, and privacy-state truthfulness; the complete trace-enabled OTLP regression ran 57 tests with 0 failures
GATE: locked workspace Clippy passed with `-D warnings`; `cargo fmt --all -- --check`, `git diff --check`, full 500-line production Rust module-size gate, and Agent Observatory golden contracts passed; Cargo.lock changed only because enabling the pinned trace feature activates its required transitive feature dependencies

## AO-043 Preserve span events, links, and OTLP privacy
commit/worktree SHA: 7c5ff1c9 (task started)
RED: AO-042 intentionally discarded event/link arrays, producer dropped counts were not materialized, and arbitrary GenAI prompt/tool/user/path content plus structural span strings could bypass the Agent Observatory privacy policy
GREEN: added ordered bounded event/link serialization with strict linked trace/span IDs, flags, timestamps, tracestate, per-item dropped-attribute counts, producer dropped-count diagnostics, and explicit Cortex byte-cap omission diagnostics; added a shared OTLP privacy layer driven by `AgentObservatoryPrivacyConfig`
PRIVACY: current GenAI prompt/output/system and tool-call argument/result keys default to redacted, user identity defaults redacted while email may retain configured SHA-256 pseudonyms, command/path fields respect their switches, nested arrays/kvlists recurse through the same policy, and generic Cortex secret-pattern/key scrubbing always remains active even when content is explicitly opted in
FIX: adversarial review made nested-array truncation explicit, deterministic attribute retention remains BTree-ordered, validates event/link attribute caps and every linked ID even after byte truncation begins, and secret-scrubs structural strings including span/status/tracestate, event names, link tracestate, service/scope strings, schema URLs, and entity-ref strings before setting `content_scrubbed=true`
PROOF: 6 focused privacy tests and 12 trace tests passed; ordered multi-event/link fixtures preserved exact fields/order, producer and Cortex truncation diagnostics were exact, invalid links/caps rejected safely, path/content opt-ins behaved as configured, and structural-secret negative fixtures proved no planted bearer-token values survived
REGRESSION: complete OTLP library filter ran 68 tests with 0 failures on the definitive locked harness
GATE: locked workspace Clippy passed with `-D warnings`; canonical workspace rustfmt, full 500-line production Rust module-size gate, Agent Observatory golden contracts, `git diff --check`, and no Cargo.toml/Cargo.lock drift from AO-042 all passed

## AO-044 Persist trace spans idempotently
commit/worktree SHA: dce43df4 (checkpoint committed)
RED: normalized spans had no durable write path, so repeat exports could not prove idempotency or distinguish duplicates from malformed records
GREEN: added one-transaction `otel_spans` batch persistence with `ON CONFLICT(trace_id, span_id) DO NOTHING`, shared bounded transient-lock retry, write serialization, and explicit accepted/duplicate/rejected accounting
FIX: direct DB-bypass validation rejects malformed/all-zero IDs, invalid timing, nonexistent run IDs, oversized flattened fields or metadata, and wrong JSON shapes per row without poisoning valid neighbors; the performance cleanup remains intact with no resurrected `OtelSpanRow` scaffold or blanket dead-code allowance
PROOF: focused locked `db::otlp_traces::tests` passed 5/5 covering empty no-op, repeat-export idempotency, same-batch duplicates, malformed-neighbor isolation, and metadata/flattened-field bounds
GATE: pre-commit `diff_check`, `env_guard`, 500-line production module-size, and rustfmt hooks passed

## AO-045 Mount functional /v1/traces
commit/worktree SHA: AO-045 checkpoint (this commit)
RED: authenticated `/v1/traces` still returned the deferred 404 and had no protobuf decode, bounded request handling, persistence, or OTLP partial-success response
GREEN: mounted an authenticated protobuf trace endpoint with an 8 MiB route-specific body cap, blocking decode/persistence offloaded through `spawn_blocking`, a 5,000-span request cap, AO-043 privacy-aware normalization, AO-044 idempotent persistence, and encoded `ExportTraceServiceResponse` output
PARTIAL: malformed individual spans, over-cap spans, configured storage-budget refusal, and direct-storage validation failures are counted as rejected without poisoning valid neighbors; duplicate exports remain successful and do not inflate rejection counts
STRUCTURE: extracted trace HTTP handling to a focused sidecar so `src/otlp.rs` is 293 lines and `src/otlp/trace_http.rs` is 234 lines, leaving runway for metrics without approaching the 500-line production module gate
PROOF: definitive locked handler suite passed 13/13 covering valid 200/protobuf, missing and invalid bearer 401, malformed protobuf 400, unsupported media 415, trace 8 MiB and preserved logs 4 MiB body-limit 413 plus Retry-After, invalid-span partial success, 5,000-span cap, storage-budget partial success, and duplicate idempotency
REGRESSION: definitive locked full OTLP library sweep passed 82/82, including all 5 AO-044 persistence tests plus existing auth/log/normalization/privacy/trace/runtime coverage
GATE: locked production `cargo check` passed without warnings; canonical rustfmt, `git diff --check`, and full 500-line production Rust module-size gate passed; real pre-push `cargo clippy --all-targets --all-features --locked -- -D warnings` passed before the AO-045 push

## AO-046 Normalize gauge and sum points
commit/worktree SHA: AO-046 checkpoint (this commit)
RED: Cortex did not enable the pinned `opentelemetry-proto` metrics message feature and had no generic normalized metric-point input, number-point converter, or deterministic point key implementation
GREEN: enabled only the existing pinned metrics feature with no lockfile churn; added privacy-aware integer/double gauge and sum conversion with exact timestamps, gauge start-time semantics, raw temporality integers, monotonicity, provider/session/project identity, canonical resource/scope metadata including entity refs, bounded/sorted point attributes, exemplars, and JSON-safe non-finite double tokens
IDEMPOTENCY: SHA-256 point keys use fixed-width component framing over canonical resource fingerprint, scope, stream identity, timestamps, sorted attributes, bounded value payload, and sorted exemplar IDs; resource/point/entity-ref input order does not affect the key
FIX: adversarial review found the original Cortex point-key contract omitted OpenTelemetry stream-identifying unit, aggregation temporality, and monotonicity, which could collapse distinct streams; corrected contract section 2.6 and implementation, preserved data-point flags inside `value_json`, and proved description remains non-identifying
STRUCTURE: split canonical exemplar/value/key encoding into `metrics_payload.rs`; production metric modules remain comfortably below the 500-line gate
PROOF: definitive locked focused metric normalization suite passed 8/8, including gauge/sum semantics, deterministic reorder-insensitive keys, stream-identity/flags collision resistance, exemplar validation, non-finite doubles, privacy policy, and fail-closed invalid fields
REGRESSION: definitive locked full OTLP library sweep passed 90/90 with all prior logs/traces/auth/privacy/runtime/database coverage intact
GATE: locked production `cargo check` passed without warnings; canonical rustfmt, `git diff --check`, and the full 500-line production Rust module-size gate passed; exact `cargo clippy --all-targets --all-features --locked -- -D warnings` passed after structurally reducing the point-normalizer argument surface and fixing the test config construction lint
