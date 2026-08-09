---
title: "Connect to MCP -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Connect to MCP -- cortex

How to connect to the cortex server from every supported client.

## Via plugin (simplest)

```bash
# Claude Code
/plugin marketplace add jmagar/claude-homelab
/plugin install cortex @jmagar-claude-homelab
```

The plugin manifest handles transport and tool registration. Configure the MCP URL and optional API token when prompted.

cortex uses RMCP Streamable HTTP in stateless JSON-response mode for daemon deployments. Local stdio clients can launch `cortex mcp` when they can read the SQLite database directly.

## Claude Code CLI

```bash
claude mcp add --transport http cortex http://localhost:3100/mcp
```

With bearer auth:

```bash
claude mcp add --transport http \
  --header "Authorization: Bearer $CORTEX_TOKEN" \
  cortex http://localhost:3100/mcp
```

### Scopes

| Flag | Scope | Config file |
| --- | --- | --- |
| `--scope project` | Current project only | `.claude/settings.local.json` |
| `--scope user` | All projects (local) | `~/.claude/settings.json` |
| (none) | Defaults to project | `.claude/settings.local.json` |

## Codex CLI

`.codex/mcp.json` (project) or `~/.codex/mcp.json` (global):

```json
{
  "mcpServers": {
    "cortex": {
      "type": "http",
      "url": "http://localhost:3100/mcp",
      "headers": {
        "Authorization": "Bearer your-token-here"
      }
    }
  }
}
```

## Gemini CLI

`gemini-extension.json` (project root) or `~/.gemini/gemini-extension.json` (global):

```json
{
  "mcpServers": {
    "cortex": {
      "type": "http",
      "url": "http://localhost:3100/mcp",
      "headers": {
        "Authorization": "Bearer your-token-here"
      }
    }
  }
}
```

## Direct stdio clients

Use `cortex mcp` for local query-only access. It exposes the same query-oriented
`cortex` tool actions as HTTP, but it does not receive syslog, start `/mcp`,
run cleanup jobs, or require `CORTEX_TOKEN`.

```json
{
  "mcpServers": {
    "cortex": {
      "command": "/path/to/cortex",
      "args": ["mcp"],
      "env": {
        "CORTEX_DB_PATH": "/data/cortex.db",
        "RUST_LOG": "warn"
      }
    }
  }
}
```

The daemon must still be running somewhere to ingest logs into that database.

## MCPB bundle

Build a Linux MCP Bundle from the existing stdio server:

```bash
just build-mcpb
```

The generated `dist/cortex-<version>-linux.mcpb` bundles the release
`cortex` binary and launches it as:

```bash
server/cortex mcp
```

The bundle is query-only. It reads `cortex.db` from the configured data
directory and does not start the syslog listener, HTTP MCP server, REST API,
Docker Compose, or deployment flows.

## stdio bridge to HTTP

Use an HTTP bridge when the DB path is not local to the MCP client, or when the server is remote/Docker-only:

```json
{
  "mcpServers": {
    "cortex": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://localhost:3100/mcp", "--transport", "http-only"]
    }
  }
}
```

## Manual configuration reference

All clients use the same `mcpServers` JSON structure. The only difference is the file path.

### Config file locations

| Client | Scope | File |
| --- | --- | --- |
| Claude Code | Project | `.claude/settings.local.json` |
| Claude Code | User | `~/.claude/settings.json` |
| Codex CLI | Project | `.codex/mcp.json` |
| Codex CLI | User | `~/.codex/mcp.json` |
| Gemini CLI | Project | `gemini-extension.json` |
| Gemini CLI | Global | `~/.gemini/gemini-extension.json` |

## Via SWAG reverse proxy

When cortex is behind SWAG, the MCP endpoint becomes:

```
https://cortex.example.invalid/mcp
```

Configure clients to use this URL instead of `localhost:3100`.

## Verifying connection

```bash
# Health check (unauthenticated)
curl -s http://localhost:3100/health
# Expected: {"status":"ok"}

# List available tools
curl -s -X POST http://localhost:3100/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

# Test a tool call
curl -s -X POST http://localhost:3100/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cortex","arguments":{"action":"stats"}}}'
```

If connection fails, check:

1. Server is running (`just up` or `just dev`)
2. Port 3100 is not blocked by firewall
3. Bearer token matches between client config and server `.env`
4. Docker port mapping is correct: `docker port cortex`

## See also

- [AUTH.md](AUTH.md) -- bearer token setup
- [ENV.md](ENV.md) -- environment variables
- [TRANSPORT.md](TRANSPORT.md) -- transport details
