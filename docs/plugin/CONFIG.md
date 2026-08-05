---
title: "Plugin Settings -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Plugin configuration and user-facing settings for Claude Code plugin deployment.
-->

# Plugin Settings -- cortex

Plugin configuration and user-facing settings for Claude Code plugin deployment.

## How it works

cortex ships one `cortex` binary with two MCP modes:

- `cortex serve mcp` -- long-lived daemon with syslog listener + MCP HTTP server.
- `cortex mcp` -- local query-only stdio MCP server.

The binary also includes direct local CLI commands such as `cortex search`,
`cortex tail`, and `cortex stats`. These are useful for host-local scripts and
manual diagnostics, but they are not plugin connection modes.

The published Claude Code plugin remains HTTP-first because plugin installs commonly target a running Docker Compose or reverse-proxy deployment.

Connection credentials flow through two files:

1. **`plugin.json`** -- declares `userConfig` fields that Claude Code prompts for at install time
2. **`.mcp.json`** -- references those fields as `${user_config.<key>}` in the URL and headers

```text
plugin.json userConfig (user enters values)
  --> .mcp.json (${user_config.*} interpolated by Claude Code)
    --> HTTP connection to running cortex server
```

Server-mode plugin installs also run the same setup path as the one-line
installer:

```text
plugin userConfig
  --> bin/cortex setup pluginhook exports CORTEX_* overrides
    --> cortex setup repair (same engine as cortex setup deploy local)
      --> ~/.cortex/.env + ~/.cortex/compose/docker-compose.yml
        --> Docker Compose cortex container
```

Client-mode installs only connect to an existing server and skip local setup.

## userConfig fields

| Field | Type | Sensitive | Description |
| --- | --- | --- | --- |
| `is_server` | boolean | no | Whether this machine should run the local ingest/MCP server |
| `server_url` | string | no | Base server URL (the plugin appends `/mcp`) |
| `api_token` | string | yes | Bearer token for MCP authentication |
| `no_auth` | boolean | no | Disable service-local auth; non-loopback server binds require `CORTEX_TRUSTED_GATEWAY_NO_AUTH=true` |
| `auth_mode` | string | no | `bearer` or `oauth` |
| `data_dir` | directory | no | Optional database directory override; empty uses `~/.cortex/data` |
| `fleet_hosts` | string | no | Fleet hosts for Docker ingest and rsyslog drop-in deployment |

Sensitive fields are stored encrypted by Claude Code and masked in the UI.
See `.claude-plugin/plugin.json` for the full field list and descriptions.

## Why the plugin defaults to HTTP

Syslog ingestion is daemon-oriented: something must listen on UDP/TCP and keep
writing SQLite. Direct stdio is useful only when the MCP host can read the
database path locally. For remote/Docker/plugin deployments, HTTP keeps the
ingestion and query surfaces attached to the same running service.

The plugin does not maintain a separate deployment model. Server mode delegates
to `cortex setup repair` (the same local reconcile path exposed as
`cortex setup deploy local`), and the generated Compose assets live under
`~/.cortex/compose`. Stale user-level `cortex.service` units/drop-ins
from older releases are disabled and removed during repair.

## See also

- [PLUGINS.md](PLUGINS.md) -- plugin manifest reference
- [../CLI.md](../CLI.md) -- direct local CLI command reference
- [../CONFIG.md](../CONFIG.md) -- full configuration reference
