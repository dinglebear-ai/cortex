---
title: "Plugin manifests"
created: "2026-07-30"
updated: 2026-09-30
---

<!--
plugin: cortex
surface: plugin-manifests
author: Jacob Magar
license: MIT
description: Current cortex Claude Code plugin manifest reference.
-->

# Plugin manifests

This repo ships separate installer and usage plugin manifests.
`plugins/install-cortex/.claude-plugin/plugin.json` owns guided installation and the MCP client configuration; `plugins/cortex/.claude-plugin/plugin.json` owns investigation skills. Plugin manifests are intentionally unversioned;
release versions live in `Cargo.toml`, `server.json`, and `mcpb/manifest.json`.

## File locations

| File | Platform | Status |
| --- | --- | --- |
| `plugins/install-cortex/.claude-plugin/plugin.json` | Claude Code | Current plugin manifest |
| `plugins/install-cortex/.mcp.json` | Claude Code | MCP server template referenced by the manifest |
| `plugins/install-cortex/skills/` | Claude Code | Guided installer skill |
| `plugins/cortex/.claude-plugin/plugin.json` | Claude Code | Usage plugin manifest |
| `plugins/cortex/skills/` | Claude Code | Investigation skills |
| `plugins/cortex/scripts/` | Claude Code | Manual setup / diagnostic scripts (not hooks) |

This repo does not currently ship tracked Codex or Gemini manifest files. It
does ship `server.json` for MCP Registry metadata. Do not copy older Codex or
Gemini examples from release history into new docs without adding the actual
manifest files.

## Claude Code plugin.json

The installer manifest declares the configuration below. The usage plugin exposes skills without duplicating this MCP connection or installer configuration.

| Field | Purpose |
| --- | --- |
| `mcpServers` | Points Claude Code at `./.mcp.json` |
| `skills` | Exposes repo-local plugin skills |
| `userConfig.server_url` | Base HTTP URL for the running cortex server |
| `userConfig.api_token` | Optional sensitive manifest field for the MCP bearer credential; server auth policy still applies. Despite its name, it is not the separate REST `CORTEX_API_TOKEN`. |
| `userConfig.no_auth` | Explicitly disables static-token enforcement for loopback deployments; non-loopback server deployments also require `CORTEX_TRUSTED_GATEWAY_NO_AUTH=true` |
| `userConfig.is_server` | Whether this machine owns the local Docker Compose deployment |
| `userConfig.cortex_receiver_port` / `cortex_receiver_host_port` / `mcp_port` | Container syslog, published syslog, and shared HTTP port controls |
| `userConfig.data_dir` | Host data directory for the Compose deployment |
| `userConfig.auth_mode` and OAuth fields | Optional OAuth/JWT configuration |
| `userConfig.docker_ingest_enabled` / `fleet_hosts` | Legacy central pull compatibility settings; prefer the host-local agent |

`plugins/install-cortex/.mcp.json` interpolates these values with `${user_config.*}`
placeholders. Keep docs and validation scripts aligned with that syntax.

## Version synchronization

Normal releases use release-please; feature branches do not hand-bump versions.
`release/components.toml` declares all synchronized carriers, and the release PR
fixup uses `cargo xtask sync-version`. `just publish` is an explicit manual
escape hatch. Keep
`plugins/install-cortex/.claude-plugin/plugin.json` and any future Claude/Codex/Gemini plugin
manifests free of a top-level `version` key; CI runs
`cargo xtask check-version-sync` (the manifest's `json_no_version` row) to
enforce that convention.

## See also

- [config.md](config.md) -- plugin settings and userConfig fields
- [hooks.md](hooks.md) -- plugin setup lifecycle (no Claude Code hooks)
- [skills.md](skills.md) -- plugin skills
- [publish.md](../../reference/mcp/publish.md) -- publishing and transport notes
