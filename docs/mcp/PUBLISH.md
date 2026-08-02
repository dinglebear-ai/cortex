---
title: "Publishing Strategy -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Publishing Strategy -- cortex

Versioning and release workflow.

## Versioning

Semantic versioning (MAJOR.MINOR.PATCH). Bump type from commit prefix:

| Prefix | Bump | Example |
| --- | --- | --- |
| `feat!:` / `BREAKING CHANGE` | Major | `0.3.1` -> `1.0.0` |
| `feat:` / `feat(scope):` | Minor | `0.3.1` -> `0.4.0` |
| `fix:`, `docs:`, `chore:`, etc. | Patch | `0.3.1` -> `0.3.2` |

## Version sync

All version-bearing files must match. Update together:

The version-bearing files are declared in `release/components.toml` and bumped
together by `cargo xtask bump-version patch|minor|major`:

| File | Field |
| --- | --- |
| `Cargo.toml` | `version = "X.Y.Z"` in `[package]` (canonical source) |
| `Cargo.lock` | the `cortex` package entry |
| `server.json` | `"version": "X.Y.Z"` plus the `cortex:vX.Y.Z` image tag |
| `mcpb/manifest.json` | `"version": "X.Y.Z"` |
| `docker-compose.prod.yml` | `${CORTEX_VERSION:-X.Y.Z}` default image tag |
| `CHANGELOG.md` | New entry under `## [X.Y.Z]` |

Plugin manifests such as `.claude-plugin/plugin.json` are intentionally
unversioned. `cargo xtask check-version-sync` (via the manifest's
`json_no_version` row) is the guardrail that prevents top-level plugin manifest
`version` keys from coming back.

## Publish workflow

```bash
just publish [major|minor|patch]
```

Steps executed:

1. Verify on `main` branch with clean working tree
2. Pull latest from origin
3. `cargo xtask bump-version <level>` — reads the current version from
   `Cargo.toml`, computes the next, and rewrites every file in
   `release/components.toml` (including `Cargo.lock` and a `CHANGELOG.md` entry)
4. `cargo xtask check-release-versions` — confirm sync + changelog
5. Commit: `release: vX.Y.Z`
6. Tag: `vX.Y.Z`
7. Push to origin with tags (triggers CI/CD publish workflows)

## Package registries

| Registry | Method | Trigger |
| --- | --- | --- |
| crates.io | `cargo publish` via GitHub Actions | `v*` tag push |
| GHCR | Docker image build and push | `v*` tag push |
| MCP Registry | `server.json` under `tv.tootie/cortex` namespace | manual update |
| MCPB (Linux) | `dist/cortex-X.Y.Z-linux.mcpb` | `just build-mcpb` |
| MCPB (Windows) | `dist/cortex-X.Y.Z-windows.mcpb` | `bash scripts/build-mcpb.sh --target windows` |

## server.json

MCP Registry metadata at repo root:

```json
{
  "name": "tv.tootie/cortex",
  "title": "Cortex",
  "description": "Syslog receiver and MCP server for homelab log intelligence.",
  "version": "X.Y.Z",
  "packages": [
    {
      "registryType": "oci",
      "identifier": "ghcr.io/dinglebear-ai/cortex:vX.Y.Z"
    }
  ]
}
```

## MCPB artifact

Run before publishing a release:

```bash
just build-mcpb
bash scripts/build-mcpb.sh --target windows
npx --yes @anthropic-ai/mcpb info dist/cortex-X.Y.Z-linux.mcpb
npx --yes @anthropic-ai/mcpb info dist/cortex-X.Y.Z-windows.mcpb
```

The unsigned MCPBs are target-specific bundles for local stdio clients. Signing is a
separate distribution step once signing keys are available.

## Verification

After publishing, verify:

```bash
# crates.io
cargo install cortex --version X.Y.Z

# Docker
docker pull ghcr.io/dinglebear-ai/cortex:vX.Y.Z

# GitHub Release
gh release view vX.Y.Z
```

## See also

- [CICD.md](CICD.md) -- publish workflows triggered by tags
- [DEPLOY.md](DEPLOY.md) -- installation methods
