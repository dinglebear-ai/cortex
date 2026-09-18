---
name: install-cortex
description: Install or repair Cortex. Use when provisioning its server, configuring a client-only host, choosing syslog/MCP/REST auth or Google OAuth, enabling durable Compose, selecting Codex app-server LLM support, or verifying agent connectivity.
---

# Install Cortex

Configure one server or a client-only connection; let Cortex own setup.

## Rules

- Inspect role, Cortex/data, ports, Docker, disk/retention, sender networks, clients, and public URL.
- Server asks syslog bind/CIDRs, HTTP bind, separate MCP/REST credentials, OAuth, storage, LLM, and persistence. Syslog has no token auth. Client-only hosts start no receiver/database.
- OAuth requires public URL/Google credentials/admin email. Ask explicitly before retaining `CORTEX_TOKEN` with `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false`.
- Existing proxy/Tailscale/Compose edits require current official docs, verified backup/checksum, exact changes, and explicit approval.

## Flow

1. Follow [setup](references/setup.md): acquire/run canonical `install.sh` and let `cortex setup repair` own config.
2. Server: configure tokens/storage; keep HTTP loopback unless needed; restrict syslog with `CORTEX_ALLOWED_SOURCE_CIDRS` and network controls.
3. Start managed Compose and prove `restart: unless-stopped` persistence. Remote HTTPS keeps Cortex auth behind the proxy.
4. Optional `CORTEX_LLM=codex[/MODEL]` uses Codex app-server; verify Codex auth first.
5. Configure selected agent MCP clients, reload them, and prove read-only `status` from a fresh session. Run setup check/doctor/health and report paths/URLs/service state/evidence without secrets.
