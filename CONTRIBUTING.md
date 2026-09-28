# Contributing to Cortex

Start with [AGENTS.md](AGENTS.md) for repository-wide instructions and [docs/README.md](docs/README.md) for operator and architecture references. `CLAUDE.md` and `GEMINI.md` resolve to that same instruction file.

## Prepare a checkout

Confirm the remote, branch, worktrees, and existing changes before editing. Use a feature branch or an isolated worktree for normal development; do not discard someone else's changes. The checked-in Rust toolchain is authoritative.

```bash
rustup show active-toolchain
cargo fetch --locked
just --list
lefthook install
```

The Cargo workspace contains the `cortex` library/binary and `xtask`; build output is `.cache/cargo`. See [Rust setup](docs/RUST.md) for toolchain, linker, and test-runner requirements. Do not copy production credentials or databases into test fixtures.

## Validate a change

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
env -u CORTEX_API_TOKEN -u NO_AUTH cargo nextest run --locked
cargo test --doc --locked
cargo xtask check-version-sync
just validate-plugin
bash scripts/check-agent-memory-symlinks.sh
bash scripts/test-agent-memory-symlinks.sh
python3 scripts/test-repository-contract.py
git diff --check
```

For a focused iteration, run the relevant sidecar tests first; a focused pass does not replace the applicable CI gates. The pre-push router in `xtask/src/pre_push.rs` is path-aware. Do not suppress a failing gate; record a genuine environment blocker separately from a code failure.

Browser changes use the package manager pinned in `web/package.json`:

```bash
pnpm --dir web install --frozen-lockfile
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test
pnpm --dir web build
```

`pnpm --dir web test:e2e` uses `web/playwright.config.ts`; review its service requirements before running it. `web/out/` is the static export served by `src/web_app.rs`; do not hand-edit generated assets.

## Change the right layer

Implement shared behavior in `src/app/` and storage behavior in `src/db/`. MCP action metadata comes from `src/mcp/actions.rs`; public surface discovery comes from `src/surfaces/`. Add sidecar tests for validation, authorization, bounds, privacy, failures, and replay where relevant. For a new ingest format, start with [Adding sources](docs/ADDING_SOURCES.md), not a second scanner or transport-specific provider table.

Update the matching current docs in the same change. Preserve dated design/history documents as history rather than silently rewriting their original claims. [Documentation maintenance](docs/repo/DOCUMENTATION.md) maps behavior to its authoritative files.

## Live work is separate

Hermetic tests do not validate an installed fleet. The isolated live profiles and explicit fleet grants are described in [LIVE_QUALIFICATION.md](docs/LIVE_QUALIFICATION.md). Do not run production reset, inventory mutation, deployment, or release commands merely because a smoke-test recipe exists.

## Publish the work

Use Conventional Commits and a PR by default. Direct integration to `main` requires explicit authorization. Normal feature branches do not bump versions: release-please and `release/components.toml` own that workflow. See [RELEASING.md](RELEASING.md).

Before pushing, inspect the staged diff for secrets and accidental artifacts, run the required gates, and preserve pre-existing changes according to the user's instructions. After pushing, verify the remote SHA and `git status --short --branch`; report CI separately from local checks.
