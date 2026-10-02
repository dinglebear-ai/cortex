# Cortex setup

Load when installing or repairing a Cortex server, client, or host agent.

## Install and preview

```sh
curl -fsSL https://raw.githubusercontent.com/dinglebear-ai/cortex/main/install.sh | sh
```

The bootstrap verifies the native release checksum, installs atomically, and calls
`cortex setup start`. It supports Linux x64/ARM64 and macOS ARM64; Windows x64 uses
`install.ps1`. Source builds remain available with `CORTEX_INSTALL_METHOD=build`.
Set `CORTEX_INSTALL_SKIP_SETUP=1` for executable acquisition only.

Choose the role interactively or explicitly with `--role server|client|agent`.
First-time noninteractive setup must specify the role. Preview the concrete plan
before changing existing exposure or proxy boundaries:

```sh
cortex setup start --role server --clients codex,claude --dry-run --json
cortex setup effective --json
```

Server setup owns managed Compose startup; do not run a redundant `compose up`.
Client and agent roles do not create a server database or receiver. Docker-backed
agents use `~/.cortex/heartbeat-agent-compose`, with configuration and removal
scoped to the agent service; do not use server Compose commands to manage them. Agent
capabilities are explicit: `docker`, `transcripts`, `journald`, `shell_history`,
`agent_commands`, `file_tails`, and `syslog_file`. Report unavailable or unconfigured selections;
do not silently omit them. Transcript, shell-history, command, and file capture
need the user's selection even when local paths are detected.

## Server and authentication

Defaults: syslog `0.0.0.0:1514` UDP/TCP, HTTP `127.0.0.1:3100`, MCP `/mcp`, REST
`/api/*`. Syslog requires CIDR/network controls. Supply all advanced settings with
`--set KEY=VALUE`; provide secrets with `--secret-file KEY=PATH`.

Keep `CORTEX_TOKEN` separate from `CORTEX_API_TOKEN` and optional
`CORTEX_API_ADMIN_TOKEN`. Non-loopback unauthenticated HTTP fails closed unless a
trusted-gateway boundary is explicit. Do not change unrelated proxies or network
policies without authorization.

OAuth callback: `https://YOUR_PUBLIC_URL/auth/google/callback`. Required:
`CORTEX_AUTH_MODE=oauth`, public URL, Google client ID/secret, and admin email.
OAuth normally disables static MCP bearer. Retaining it requires the explicit
choice `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false`; ingest credentials
remain a separate policy.

Compose uses `restart: unless-stopped`; verify Docker starts at boot and that
health survives restart when restart validation is authorized.

## Client, agent, LLM, and proof

```sh
cortex setup start --role client --server https://cortex.example.com \
  --token-file /private/mcp-token --api-token-file /private/rest-token \
  --clients codex,claude
cortex setup start --role agent --server https://cortex.example.com \
  --token-file /private/ingest-token --capabilities docker,transcripts
cortex setup verify --json
```

Client-only machines use server URL/auth and start no database or receiver.
`CORTEX_LLM=codex` selects Codex app-server; `gemini[/MODEL]` is the alternative.

Verification exercises authenticated MCP initialization and read-only `status`,
plus REST when configured. Reopen selected clients and prove `status` from a fresh
session. Health, service registration, and capability selection are not evidence
of telemetry delivery; prove each selected source independently. Create test
syslog records only with approval.

Interactive first-server setup asks for authentication, exposure, OAuth inputs,
and optional six-hour backups. Automation can save the schedule policy with
`--backup-schedule` or `--no-backup-schedule`; scheduling needs an installed
executable and working user cron.

Saved deployment settings support `cortex update` while preserving version pins.
Use the [deployment runbook](../../../../../docs/runbooks/deploy.md) for managed
backup scheduling and schema-compatible recovery. Report verification evidence
and any recovery or platform limitations without exposing secrets.
