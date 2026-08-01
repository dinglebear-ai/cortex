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
