---
title: "Coding and Git rules"
created: 2026-07-30
updated: 2026-09-27
---

# Coding and Git rules

[AGENTS.md](../../../AGENTS.md) is canonical. This page summarizes the contributor-facing rules; it does not override scoped instructions or explicit task authorization.

## Git workflow

Use feature branches and pull requests by default. Treat `main` as production-ready; direct integration requires explicit authorization. Inspect and preserve pre-existing changes. Do not silently clean, reset, force-push, or include unrelated work. Even when all dirty changes are authorized for inclusion, inspect them for secrets and accidental runtime artifacts.

Use Conventional Commits, for example `fix(db): handle checkpoint timeout` or `docs(repo): refresh contributor instructions`. `feat` denotes a feature and a breaking-change marker identifies an incompatible change; release-please computes the release from the configured commit history.

## Version policy

Normal feature branch pushes do **not** bump versions. Release-please manages release PRs after green main CI. `Cargo.toml` is the canonical version and `release/components.toml` lists every synchronized carrier. Run `cargo xtask check-version-sync`; release changes additionally require `cargo xtask check-release-versions`. Plugin manifests are intentionally unversioned.

`cargo xtask bump-version` and `just publish` are explicit manual escape hatches, not routine contribution steps. The root package has `publish = false`; no crates.io publishing workflow exists. See [RELEASING.md](../../../RELEASING.md) and [release.md](../release.md).

## Source style and checks

Use sibling Rust modules and sidecar unit tests. Keep shared behavior in `CortexService`, parameterize SQL, run blocking DB operations through the existing helpers, use structured `tracing`, and preserve authorization, bounds, redaction, retry/cancellation, and provenance. `cargo fmt --all -- --check` and `cargo clippy --all-targets --locked -- -D warnings` must pass for applicable Rust changes.

Bash scripts use `set -euo pipefail` and quote path/variable expansions. Do not bypass Lefthook: pre-commit is staged-file scoped and pre-push delegates to `cargo xtask pre-push`. See [CONTRIBUTING.md](../../../CONTRIBUTING.md) for the test matrix.

## Never commit

Credentials, private keys, `.env` secrets, production databases, raw transcripts, local logs, and generated build output must remain outside source control. `Cargo.lock` is tracked for reproducibility. Dependency and lockfile updates must be intentional; do not opportunistically update pinned SDKs during unrelated work.

## Documentation

Edit `AGENTS.md`, not independent Claude/Gemini copies. Update current docs with behavior changes; retain historical plans as history. Run the symlink validator and regression suite. See [documentation.md](documentation.md) for ownership and validation.
