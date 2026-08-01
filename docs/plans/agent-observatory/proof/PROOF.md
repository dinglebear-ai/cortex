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
