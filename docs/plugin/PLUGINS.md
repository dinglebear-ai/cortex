---
title: "Plugin Manifest Reference -- cortex"
created: "2026-07-30"
updated: 2026-09-27
---

<!--
plugin: cortex
surface: plugin-manifests
author: Jacob Magar
license: MIT
description: Current cortex Claude Code plugin manifest reference.
-->

# Plugin Manifest Reference -- cortex

This repo currently ships the Claude Code plugin manifest at
`.claude-plugin/plugin.json`. Plugin manifests are intentionally unversioned;
release versions live in `Cargo.toml`, `server.json`, and `mcpb/manifest.json`.

## File locations

| File | Platform | Status |
| --- | --- | --- |
| `.claude-plugin/plugin.json` | Claude Code | Current plugin manifest |
| `plugins/cortex/mcp.json` | Claude Code | MCP server template referenced by the manifest |
| `plugins/cortex/skills/` | Claude Code | Plugin skill surfaces |
| `plugins/cortex/scripts/` | Claude Code | Manual setup / diagnostic scripts (not hooks) |

This repo does not currently ship tracked Codex or Gemini manifest files. It
does ship `server.json` for MCP Registry metadata. Do not copy older Codex or
Gemini examples from release history into new docs without adding the actual
manifest files.

## Claude Code plugin.json

The current manifest declares:

| Field | Purpose |
| --- | --- |
| `mcpServers` | Points Claude Code at `./plugins/cortex/mcp.json` |
| `skills` | Exposes repo-local plugin skills |
| `userConfig.server_url` | Base HTTP URL for the running cortex server |
| `userConfig.api_token` | Required Bearer token used by the plugin MCP client; enforced by the server unless `no_auth=true` |
| `userConfig.no_auth` | Explicitly disables static-token enforcement for loopback deployments; non-loopback server deployments also require `CORTEX_TRUSTED_GATEWAY_NO_AUTH=true` |
| `userConfig.is_server` | Whether this machine owns the local Docker Compose deployment |
| `userConfig.syslog_port` / `syslog_host_port` / `mcp_port` | Container port mapping controls |
| `userConfig.data_dir` | Host data directory for the Compose deployment |
| `userConfig.auth_mode` and OAuth fields | Optional OAuth/JWT configuration |
| `userConfig.docker_ingest_*` | Legacy central pull compatibility settings; prefer the host-local agent |

`plugins/cortex/mcp.json` interpolates these values with `${user_config.*}`
placeholders. Keep docs and validation scripts aligned with that syntax.

## Version synchronization

Normal releases use release-please; feature branches do not hand-bump versions.
`release/components.toml` declares all synchronized carriers, and the release PR
fixup uses `cargo xtask sync-version`. `just publish` is an explicit manual
escape hatch. Keep
`.claude-plugin/plugin.json` and any future Claude/Codex/Gemini plugin
manifests free of a top-level `version` key; CI runs
`cargo xtask check-version-sync` (the manifest's `json_no_version` row) to
enforce that convention.

## See also

- [CONFIG.md](CONFIG.md) -- plugin settings and userConfig fields
- [HOOKS.md](HOOKS.md) -- plugin setup lifecycle (no Claude Code hooks)
- [SKILLS.md](SKILLS.md) -- plugin skills
- [../mcp/PUBLISH.md](../mcp/PUBLISH.md) -- publishing and transport notes
