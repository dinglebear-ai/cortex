---
title: "Marketplace Publishing -- cortex"
created: "2026-07-30"
updated: 2026-09-27
---

<!--
plugin: cortex
surface: marketplace-publishing
author: Jacob Magar
license: MIT
description: Marketplace publishing and registry package reference for cortex.
-->

# Marketplace Publishing -- cortex

Repository-owned packaging surfaces and registry metadata. Public directory acceptance and external marketplace availability are not established by the presence of a manifest.

## Marketplace locations

| Marketplace | Manifest | Registry entry |
| --- | --- | --- |
| Claude Code | `plugins/install-cortex/.claude-plugin/plugin.json` and `plugins/cortex/.claude-plugin/plugin.json` | Client/onboarding package in this repository |
| Codex | `.codex-plugin/plugin.json` | Not currently shipped |
| Gemini | `gemini-extension.json` | Not currently shipped |
| MCP Registry | `server.json` | Tracked MCP Registry metadata |

## Installation

### Claude Code

Use the current instructions in [plugins/cortex/README.md](../../plugins/cortex/README.md) and the `install-cortex` skill. Do not assume an older third-party marketplace location is still authoritative.

### Codex CLI

No Codex plugin manifest is currently shipped from this repo.

### Gemini CLI

No Gemini extension manifest is currently shipped from this repo.

## MCP Registry

The repo ships `server.json` for MCP Registry metadata under the
`ai.dinglebear/cortex` namespace, with DNS verification via the `dinglebear.ai`
domain.

Example registry entry:

```json
{
  "name": "ai.dinglebear/cortex",
  "packages": [
    {
      "registryType": "oci",
      "identifier": "ghcr.io/dinglebear-ai/cortex:vX.Y.Z"
    }
  ]
}
```

## OCI publishing

Cortex publishes a container image alongside native release archives and the
`@dinglebear/cortex` npm launcher. The current distribution workflows, rather
than this example table, define actual publication triggers and platforms:

| Registry | Image |
| --- | --- |
| GHCR | `ghcr.io/dinglebear-ai/cortex:latest` |
| GHCR (versioned) | `ghcr.io/dinglebear-ai/cortex:vX.Y.Z` |

The root Cargo package sets `publish = false`; crates.io is not a distribution
path. See [PUBLISH.md](../mcp/PUBLISH.md) for release-please and packaging gates.

## See also

- [PLUGINS.md](PLUGINS.md) -- manifest file details
- [../mcp/PUBLISH.md](../mcp/PUBLISH.md) -- versioning and release workflow
