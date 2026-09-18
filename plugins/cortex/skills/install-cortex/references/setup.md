# Cortex setup

## Install

```sh
curl -fsSLo /tmp/cortex-install.sh https://raw.githubusercontent.com/dinglebear-ai/cortex/main/install.sh
sh /tmp/cortex-install.sh
```

The canonical installer checksum-verifies native releases (or supports `CORTEX_INSTALL_METHOD=build`) and calls `cortex setup repair` without replacing existing tokens.

## Server + auth

Defaults: syslog `0.0.0.0:1514` UDP/TCP, HTTP `127.0.0.1:3100`, MCP `/mcp`, REST `/api/*`. Syslog requires CIDR/network controls.

Keep `CORTEX_TOKEN` separate from `CORTEX_API_TOKEN` and optional `CORTEX_API_ADMIN_TOKEN`. Non-loopback unauthenticated HTTP fails closed unless a trusted-gateway boundary is explicit.

OAuth callback: `https://YOUR_PUBLIC_URL/auth/google/callback`. Required: `CORTEX_AUTH_MODE=oauth`, public URL, Google client ID/secret, admin email. OAuth normally disables static MCP bearer; set `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false` only for intentional dual mode. OTLP may still need `CORTEX_TOKEN`.

```sh
cortex setup check
cortex setup repair
cortex compose up
cortex compose status
```

Compose uses `restart: unless-stopped`; verify Docker starts at boot and health survives restart.

## Client, LLM, proof

Client-only machines use the server `/mcp` URL/auth and run no database/receiver. `CORTEX_LLM=codex` is Codex app-server; `gemini[/MODEL]` is the alternative.

```sh
cortex status
cortex doctor
curl -fsS http://127.0.0.1:3100/health
```

Create a test syslog record only with approval. Debug setup, listener/network, auth, storage, Compose, proxy, client, and LLM layers independently.
