---
title: "MCP development workflow"
created: 2026-07-30
updated: 2026-09-27
---

# MCP development workflow

Read the root [AGENTS.md](../../AGENTS.md), [CONTRIBUTING.md](../../CONTRIBUTING.md), and the scoped [MCP instructions](AGENTS.md). They define checkout safety, the pinned toolchain, tests, and publication policy.

## Local iteration

Build with `cargo build --locked`. Before `just dev` or `cargo run -- serve mcp`, configure a writable database path and the required REST token as described in [SETUP.md](../SETUP.md). Merely copying `.env.example` does not prove the process has loaded valid credentials. Keep a development server separate from the installed deployment.

Use `just --list` for recipes. `just test` runs cargo-nextest; `just test-doc` runs doctests separately. `just validate-plugin` validates packaging and skills. Live profile requirements and permitted targets are described in [LIVE_QUALIFICATION.md](../LIVE_QUALIFICATION.md).

## Change an MCP action

Start in `src/mcp/actions.rs::ACTION_SPECS`: action identity, scope, cost, input metadata, flags, and handler dispatch belong to that registry and its supporting modules. Inspect a comparable current handler rather than copying an old handwritten switch. Shared behavior belongs in `CortexService` under `src/app/`; persistence belongs under `src/db/`.

Preserve the public `cortex` tool name and existing wire contracts. Add the handler and relevant CLI/REST projections through their registries, with sidecar tests for authorization, invalid input, bounds, redaction, and failure behavior. Deterministic MCP investigation must not silently invoke a CLI-only LLM assessment.

Update the corresponding [TOOLS.md](TOOLS.md), [SCHEMA.md](SCHEMA.md), [TESTS.md](TESTS.md), [INVENTORY.md](../INVENTORY.md), and affected skill references. Keep registry coverage tests green. Do not reintroduce separate action-name/scope lists or hand-edited help inventories. Plugin manifests remain unversioned.

## Source and package locations

Syslog parsing lives in `src/receiver/`, not `src/syslog/`. The source-build Dockerfile is `config/Dockerfile`. Skills live in `plugins/cortex/skills/`; primary guidance is `using-cortex`. The current tracked plugin manifest is `.claude-plugin/plugin.json`; no Codex/Gemini manifest or Claude lifecycle-hook directory is shipped. See [repository structure](../repo/REPO.md).

## Diagnostics

Use scoped `RUST_LOG` filters and bounded requests to inspect the behavior under test. Do not print credentials or raw private evidence. Use [CONNECT.md](CONNECT.md), [MCPORTER.md](MCPORTER.md), and the canonical live harness for authenticated MCP calls; a unauthenticated example is not a substitute for the configured policy.

Run formatting, Clippy, the applicable tests, and documentation/packaging checks before publishing. A local unit test, a remote CI pass, and a live deployment verification are distinct results.
