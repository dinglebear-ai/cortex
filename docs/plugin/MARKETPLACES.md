---
title: "Marketplace Publishing -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

<!--
plugin: cortex
surface: marketplace-publishing
author: Jacob Magar
license: MIT
description: Marketplace publishing and registry package reference for cortex.
-->

# Marketplace Publishing -- cortex

Registration and publishing patterns for Claude, Codex, and Gemini marketplaces.

## Marketplace locations

| Marketplace | Manifest | Registry entry |
| --- | --- | --- |
| Claude Code | `.claude-plugin/plugin.json` | `claude-homelab` marketplace |
| Codex | `.codex-plugin/plugin.json` | Not currently shipped |
| Gemini | `gemini-extension.json` | Not currently shipped |
| MCP Registry | `server.json` | Tracked MCP Registry metadata |

## Installation

### Claude Code

```bash
/plugin marketplace add jmagar/claude-homelab
/plugin install cortex @jmagar-claude-homelab
```

### Codex CLI

No Codex plugin manifest is currently shipped from this repo.

### Gemini CLI

No Gemini extension manifest is currently shipped from this repo.

## MCP Registry

The repo ships `server.json` for MCP Registry metadata under the
`tv.tootie/cortex` namespace, with DNS verification via the `tootie.tv`
domain.

Example registry entry:

```json
{
  "name": "tv.tootie/cortex",
  "packages": [
    {
      "registryType": "oci",
      "identifier": "ghcr.io/dinglebear-ai/cortex:vX.Y.Z"
    }
  ]
}
```

## OCI publishing

cortex uses OCI (Docker) images as the primary distribution package, not PyPI or npm:

| Registry | Image |
| --- | --- |
| GHCR | `ghcr.io/dinglebear-ai/cortex:latest` |
| GHCR (versioned) | `ghcr.io/dinglebear-ai/cortex:vX.Y.Z` |

Additionally published to crates.io for `cargo install` usage.

## See also

- [PLUGINS.md](PLUGINS.md) -- manifest file details
- [../mcp/PUBLISH.md](../mcp/PUBLISH.md) -- versioning and release workflow
