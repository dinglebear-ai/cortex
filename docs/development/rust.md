---
title: "Rust build setup"
created: 2026-05-15
updated: 2026-09-27
---

# Rust build setup

The checked-in files are authoritative: [`rust-toolchain.toml`](../../rust-toolchain.toml), [`Cargo.toml`](../../Cargo.toml), [`Cargo.lock`](../../Cargo.lock), and [`.cargo/config.toml`](../../.cargo/config.toml). This repository must not inherit a different SDK or toolchain policy from an old template guide.

## Toolchain and workspace

Rust **1.97.1**, edition **2024**, is pinned by `rust-toolchain.toml` and the workspace MSRV. The toolchain includes rustfmt and Clippy. Use `rustup show active-toolchain` from the checkout to verify selection; do not override it with an unpinned `stable` command.

The workspace has two members: the root `cortex` package (library plus explicitly declared binary) and `xtask/`. Cargo is supplied with Rust. A platform C toolchain is needed for bundled native dependencies; on macOS use the installed command-line developer tools, while Linux linker/cache choices depend on the host environment. `clang`, `mold`, and cache wrappers are not universal runtime requirements.

## Repo-specific overrides

`.cargo/config.toml` provides:

```toml
[alias]
xtask = "run --quiet --package xtask --"

[build]
target-dir = ".cache/cargo"
```

Global Cargo settings may add a linker, wrapper, jobs, or caches. Inspect them when diagnosing a host-only failure; do not overwrite them as part of repository setup. `.cache/cargo` is generated output, not source.

## Dependency policy

The workspace pins `rmcp = "=3.1.0"` with default features disabled. Root runtime and dev dependencies enable their required feature sets separately. `lab-auth` is pinned to a Git revision in `Cargo.toml`. `Cargo.lock` is tracked; use `--locked` for reproducible checks and make dependency updates explicit.

## Developer checks

`just` is the recipe runner. Install/configure cargo-nextest for `just test`, cargo-llvm-cov for coverage, and Lefthook for Git hooks. Python 3 and Bash run repository validators. Exact recipes and CI invocations live in `Justfile`, `lefthook.yml`, and `.github/workflows/ci.yml`.

```bash
cargo build --locked
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
env -u CORTEX_API_TOKEN -u NO_AUTH cargo nextest run --locked
cargo test --doc --locked
cargo xtask check-version-sync
```

Use [CONTRIBUTING.md](../../CONTRIBUTING.md) for the full validation and publication workflow. Browser tooling is independently pinned in `web/package.json`; it is not another Cargo workspace member.
