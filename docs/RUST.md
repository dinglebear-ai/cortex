---
title: "Rust Build Setup"
created: 2026-05-15
updated: 2026-07-30
---

---
title: "Rust Build Setup"
doc_type: "guide"
status: "active"
owner: "cortex"
audience:
  - "contributors"
  - "agents"
scope: "service"
source_of_truth: false
upstream_refs:
  - "https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md"
last_reviewed: "2026-05-15"
---

# Rust Build Setup

This repo follows the build conventions of the rmcp server family.
The canonical reference is [rmcp-template/docs/RUST.md](https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md).

## System prerequisites

- Rust stable ≥ 1.86 (`rustup update stable`)
- `clang` and `mold` for fast Linux builds: `apt install clang mold`
- `just` command runner (optional): `cargo install just`

## Global Cargo config

Build performance depends on `~/.cargo/config.toml` on the developer's machine.
See [rmcp-template/docs/RUST.md](https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md)
for the expected config (mold linker, profile settings, Cranelift backend).

## Local `.cargo/config.toml`

This repo's `.cargo/config.toml` has one intentional override:

```toml
[build]
target-dir = ".cache/cargo"
```

**Why:** The non-standard target directory keeps Cargo artifacts out of the
repo root so Docker `COPY` instructions and bind mounts don't require an
explicit `./target` exclusion. All other settings (mold linker, profile
tuning, Cranelift) are inherited from the global config.

This repo has no xtask crate, so no `[alias]` section is needed.

## rmcp version intent

`Cargo.toml` declares `rmcp = "1.6.0"` as the supported lower bound for the
HTTP and stdio MCP API surface cortex uses. Cargo may resolve a newer compatible
`1.x` release in `Cargo.lock` (currently the lockfile resolves `rmcp 1.7.0`).
That is intentional semver behavior, not a mismatch. Do not pin the manifest to
the lockfile version unless cortex starts depending on an API that requires that
newer release.
