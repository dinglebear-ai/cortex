---
title: "Development and runtime prerequisites"
created: 2026-04-04
updated: 2026-09-27
---

# Development and runtime prerequisites

The checked-in manifests define the supported toolchain. Use [rust.md](../rust.md) and [CONTRIBUTING.md](../../../CONTRIBUTING.md), not an older template's minimum versions.

## Development

| Tool | Requirement |
| --- | --- |
| Rust / Cargo | Rust 1.97.1, edition 2024; `rust-toolchain.toml` selects rustfmt and Clippy |
| Platform compiler tools | Required to build bundled native dependencies; use the host's configured C toolchain/linker |
| just | Runs the checked-in `Justfile` recipes |
| cargo-nextest | Hermetic Rust tests; doctests run separately |
| Lefthook | Staged-file pre-commit and path-aware pre-push checks |
| Python 3 / Bash | Repository validators and maintenance scripts |
| Node / pnpm | Browser work only; package manager and dependencies are pinned in `web/package.json` |
| Docker / Compose v2 | Container development and isolated live qualification; not needed for every unit test |
| curl / jq / openssl | HTTP diagnostics, readable JSON, and token generation recipes |

`cargo-llvm-cov` supports coverage and `cargo-deny` supports dependency policy checks. Live MCP profiles may also require mcporter and browser tooling; follow [live-qualification.md](../live-qualification.md).

## Runtime

Use the release artifact for the target OS/architecture or the supported container image. A bundled SQLite library does not mean every binary is fully static or that Linux runtime requirements apply to macOS/Windows. Inspect the release workflow and `config/Dockerfile` for the actual platform and runtime packaging. No separate SQLite server is needed.

Container operation requires Docker and Compose plus writable persistent storage and correct bind-mount ownership. Defaults use syslog UDP/TCP 1514 and HTTP 3100; remote exposure requires the configured auth/network policy. Follow [setup.md](../../guides/setup.md).

## Capacity

Size memory, disk, and CPU for ingestion volume, transcript/index history, retention, concurrent queries, and enabled projections. Old toy-server memory estimates are not supported sizing guarantees for the current service. Configure and observe the storage budget, SQLite cache/mmap, query concurrency, and container limits described in [config.md](../../reference/config.md). Test representative workloads before applying production limits.
