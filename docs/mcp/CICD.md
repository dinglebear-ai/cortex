---
title: "CI/CD workflows"
created: 2026-07-30
updated: 2026-09-27
---

# CI/CD workflows

The executable source is [`.github/workflows/`](../../.github/workflows). Do not copy illustrative YAML into this page and present it as the running configuration.

| Workflow | Current responsibility |
| --- | --- |
| `ci.yml` | Main pushes, PRs targeting main, and a weekly scheduled check; path classification selects Rust, docs, browser, security, plugin, and related gates |
| `repository-contract.yml` | Repository/fleet contract validation |
| `live-qualification.yml` | Explicit live qualification profiles and their evidence |
| `release-please.yml` | Release PR creation/fixup after successful main CI, plus manual dispatch |
| `release.yml` | Tag-triggered native release builds, live/package gates, GitHub release assets, and npm launcher publication; manual dispatch is gated |
| `docker-publish.yml` | Published-release/manual container build, verification, and publication; inspect the workflow for its actual platform support |
| `mcp-registry.yml` | Published-release/manual MCP registry publication through the pinned shared workflow |

There is no current `publish-crates.yml` or `codex-plugin-scanner.yml`. The root package has `publish = false`. Plugin/skill validation is part of current repository checks, not an imaginary standalone legacy workflow.

## Validation

The main CI workflow classifies changed paths and uses the checked-in Rust setup action. Applicable gates include formatting, Clippy, nextest, doctests, doc-contract tests, version synchronization, module size, public identity/security, and browser/package checks. Agent instruction authority and its regression fixtures run in a lightweight independent job so documentation-only changes cannot bypass them.

Use [CONTRIBUTING.md](../../CONTRIBUTING.md) for local commands. Lefthook's pre-commit gate is staged-file scoped; its pre-push router is path-aware. A local gate pass and completed remote CI are separate evidence.

## Releases and permissions

Normal feature commits do not hand-bump versions. Release-please and `release/components.toml` maintain release carriers. Tag/release events publish only through the configured gates. `just publish` is an explicitly authorized manual escape hatch. See [PUBLISH.md](PUBLISH.md), [RELEASING.md](../../RELEASING.md), and [RELEASE.md](../RELEASE.md).

Token names, permissions, pinned action revisions, runner images, cache setup, and publication architecture are defined in the workflows. Never add a credential value to a guide or assume a default workflow token can trigger all downstream publication events.
