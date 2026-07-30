---
title: "MCP Elicitation -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# MCP Elicitation -- cortex

## Overview

Elicitation is an MCP protocol capability that allows servers to request information from users interactively. cortex does not use elicitation.

## Why no elicitation

cortex is a self-contained syslog receiver with no interactive first-run prompts. There are no upstream credentials to collect via MCP elicitation. All configuration is handled via environment variables, `config.toml`, and plugin `userConfig`.

Most MCP actions are read-only and require `cortex:read` when auth is mounted. A small set of state-changing/admin actions exists:

- `ack_error`
- `unack_error`
- `notifications_test`

Those actions require `cortex:admin`; they do not use elicitation confirmation gates. The action registry and scope mapping live in `src/mcp/actions.rs::ACTION_SPECS`.

## Configuration entry points

Instead of elicitation, cortex uses:

| Method | Purpose |
| --- | --- |
| Environment variables | All runtime configuration |
| `config.toml` | Local development overrides |
| Plugin `userConfig` | MCP URL and API token when installed via Claude Code plugin |

## See also

- [ENV.md](ENV.md) -- environment variable reference
- [AUTH.md](AUTH.md) -- bearer token configuration
