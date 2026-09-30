---
title: "Publishing and distribution"
created: 2026-07-30
updated: 2026-09-27
---

# Publishing and distribution

[RELEASING.md](../../../RELEASING.md) and [release.md](../../development/release.md) define the release workflow and gates. `release/components.toml` is the machine-readable version-carrier inventory; `Cargo.toml` is the canonical package version.

## Normal release path

Use Conventional Commits on feature work. Do not bump versions on every feature branch push. Release-please opens/updates the release PR after successful main CI. Its fixup runs `cargo xtask sync-version` and validates `cargo xtask check-release-versions` so all declared carriers and the changelog agree.

Plugin manifests are intentionally unversioned. The `json_no_version` invariant rejects a top-level plugin version. Add any new version-bearing file to `release/components.toml` and the appropriate release-please configuration rather than maintaining a private list.

`just publish [major|minor|patch]` is a manual escape hatch that bumps, commits, tags, and pushes. It requires explicit release intent and clean `main`; it is not the routine contribution workflow.

## Distribution surfaces

| Surface | Source / gate |
| --- | --- |
| Native release archives | `.github/workflows/release.yml`; inspect its build targets for supported platforms |
| npm launcher `@dinglebear/cortex` | `packages/cortex-rmcp/` and the gated npm job in `release.yml` |
| GHCR image | `.github/workflows/docker-publish.yml`; published release or explicit dispatch |
| MCP Registry | `server.json` and `.github/workflows/mcp-registry.yml` |
| MCP bundles | `mcpb/manifest.json` and `scripts/build-mcpb.sh` |
| Onboarding/skills | `plugins/cortex/` and plugin/skill validation |

The root Cargo package sets `publish = false`: **crates.io is not a distribution path**. Do not use `cargo install cortex` as an installation example. Use the repository installer or supported release/npm distribution described in [README.md](../../../README.md). A source install uses an explicit local checkout (`cargo install --path . --locked`).

## Bundle builds

```bash
just build-mcpb
bash scripts/build-mcpb.sh --target windows
```

These create platform-specific bundles for local stdio clients. Follow the script's prerequisite checks; the Windows cross-build requires its Rust target and matching cross-linker. Signing is a separate distribution concern, not implied by successful packaging.

## Verify publication

Check the actual release/tag, workflow results, attached assets and checksums, container identity, npm version, and registry entry for the same release. Do not equate a Git push with a successful release or a deployed fleet upgrade. Remote deployment and service restarts remain separate, explicitly authorized operations.
