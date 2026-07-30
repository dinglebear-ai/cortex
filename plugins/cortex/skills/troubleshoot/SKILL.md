---
name: troubleshoot
description: Troubleshoot cortex connection failures, missing logs, unhealthy containers, restart loops, or vague "logs aren't working" reports.
---

# Cortex Troubleshooting Skill

Diagnose cortex problems systematically. Use the binary's observability counters and existing diagnostic tooling rather than guessing — the codebase exposes most state needed to localize a failure.

## Decision tree — pick the right diagnostic

Match the user's report against one of these branches and follow only that branch. Don't run every check when the failure is narrow.

### Branch A — "MCP can't connect" / "Failed to reconnect" / "401 / 404 from /mcp"

Most common cause: empty / wrong `$CLAUDE_PLUGIN_OPTION_SERVER_URL`, mismatched `$CLAUDE_PLUGIN_OPTION_API_TOKEN`, or service not running.

1. **Is anything listening on the MCP port?**
   `ss -tlnp | grep -E ":$CLAUDE_PLUGIN_OPTION_MCP_PORT"` — if empty, the service is down → branch C
2. **Is the URL Claude Code is using sane?**
   Read `~/.claude/settings.json`, find the `pluginConfigs` key that starts with `cortex@`, and inspect `options.server_url` — empty string is a known footgun (the `.mcp.json` substitution produces a literal `/mcp`). Check non-empty, has scheme, no trailing `/mcp`.
3. **Does observed auth match configured auth?**
   Run `curl -sS -o /dev/null -w '%{http_code}' "$CLAUDE_PLUGIN_OPTION_SERVER_URL/mcp"`.
   - If `$CLAUDE_PLUGIN_OPTION_NO_AUTH` is true or no bearer/OAuth auth is configured, `200` or MCP protocol-level `400/405` can be normal route evidence.
   - If bearer or OAuth auth is enabled, expect `401` for an unauthenticated request.
   - If `404`, the route is wrong or a different server owns that port. If connection refused, branch C.
   - If `200` while auth is intended to be enabled, flag it as an auth configuration mismatch.
4. **Token roundtrip in bearer mode**: `curl -sS -X POST -H "Authorization: Bearer $CLAUDE_PLUGIN_OPTION_API_TOKEN" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}' "$CLAUDE_PLUGIN_OPTION_SERVER_URL/mcp"`. 401 = wrong token. 200 with valid response = server fine, problem is in Claude Code's MCP client config. For OAuth mode, use the OAuth client flow instead of bearer-token curl. Note: verify the MCP protocol version string (`2025-11-25`) matches the current spec if this test fails unexpectedly.

### Branch B — "No logs from <host>" / "host X stopped sending" / "missing entries"

1. **Does the host appear in the hosts list at all?**
   Call MCP tool: `cortex action=hosts`. If host is absent, no logs ever arrived → check forwarding config on `<host>`. If present with old `last_seen`, forwarding stopped → check rsyslog/forwarder on host.
2. **Is the listener actually accepting connections?**
   `ss -tlnp | grep -E ":(${CLAUDE_PLUGIN_OPTION_SYSLOG_HOST_PORT:-$CLAUDE_PLUGIN_OPTION_SYSLOG_PORT})\\b"` should show our process or container port publish. From `<host>`: `nc -zv <our_host> "${CLAUDE_PLUGIN_OPTION_SYSLOG_HOST_PORT:-$CLAUDE_PLUGIN_OPTION_SYSLOG_PORT}"` should connect.
3. **Recent forwarding errors on the host?**
   `ssh <host> "sudo journalctl -t rsyslogd -n 30 --no-pager"` — look for `omfwd` errors (DNS resolution, peer closed, EOF on TCP). Common patterns we've seen: stale forwarder pointing at a dead host, idle TCP timeout flapping, missing rsyslog drop-in.
4. **Drop-in present and correct?**
   `ssh <host> "cat /etc/rsyslog.d/99-cortex.conf 2>/dev/null"` should contain `*.* @@<our_host>:<externally reachable syslog port>` (TCP), usually `${CLAUDE_PLUGIN_OPTION_SYSLOG_HOST_PORT:-$CLAUDE_PLUGIN_OPTION_SYSLOG_PORT}` — if missing or wrong, push the drop-in config to the host over SSH.
5. **For Docker container logs**: if user expected logs from a container in `$CLAUDE_PLUGIN_OPTION_FLEET_HOSTS` but doesn't see them, check `$CLAUDE_PLUGIN_OPTION_DOCKER_INGEST_ENABLED`. If false, ingest is off entirely. If true, verify the docker-socket-proxy on that host is reachable: `curl -sS http://<host>:2375/_ping` should return `OK`.

### Branch C — Service down / crashing / unhealthy

1. **Get current state**:
   `docker ps --filter name=cortex --format '{{.Status}}'`
2. **If recently restarted / crashing — get the actual error**: use `logs` for the last 100 lines, or run `docker compose logs` manually. Look for: panic messages, port-bind errors (`address already in use`), DB lock errors, OOM kills.
3. **Common service-failure causes (ranked by frequency in this plugin's history)**:
   1. Port `$CLAUDE_PLUGIN_OPTION_SYSLOG_PORT` or `$CLAUDE_PLUGIN_OPTION_MCP_PORT` held by another process. First identify the owner with `ss -tulpn`/`lsof`/`fuser`; only kill or restart anything after the user approves the specific process and impact.
   2. Database lock (another `cortex` stdio process holds it). `pgrep -af "cortex"` to list candidates; only kill stragglers after approval.
   3. Docker image missing/stale: `docker compose pull` to refresh.
4. **If healthcheck failing but `/health` works manually**: Container is unhealthy because the healthcheck command inside the image is wrong/can't run. Compare image version to what you expect — `docker inspect cortex | jq '.[0].Config.Image'`.

### Branch D — "Something's off" / vague / user doesn't know

Run a comprehensive preflight and health check: env/config, storage, ports, service status, HTTP `/health`, MCP actions, listener reachability, Docker ingest, and fleet rsyslog forwarding. A PASS / WARN / FAIL result narrows the problem to a specific check. Then re-enter this skill on the failing check's category.

## Use observability counters

The binary exposes runtime counters via `cortex action=stats` and `/health`. Useful signals:

- `total_logs` not increasing → ingest pipeline is broken, not just MCP
- `write_blocked: true` → storage budget tripped, oldest logs being purged but can't keep up; check `$CLAUDE_PLUGIN_OPTION_MAX_DB_SIZE_MB` vs disk free
- `phantom_fts_rows` growing → retention purges aren't merging FTS5 cleanly; usually self-recovers
- `last_ingest_at` minutes-stale → forwarders aren't reaching us
- Newer counters in `RuntimeObservability` (since v0.13.0): UDP/TCP packets, ingest queue depth, writer flush failures — pull these via the `/health` endpoint or stats action and use to localize "ingest path" vs "writer path" failures

## Don't over-fix

- For a single-host symptom, don't restart the whole stack — just fix that host's forwarder.
- For an MCP-only failure with healthy ingest, don't touch the listener config.
- If the immediate problem is a missing config, prefer `redeploy` over manual Docker commands.

## When to escalate to the user

- After a confident diagnosis, propose the fix and ask before applying it for anything destructive: changing settings.json, killing processes, deleting files, switching deploy modes.
- If checks return inconsistent state (e.g. listener says ours, but the binary says it isn't writing), surface the inconsistency rather than guessing.
- If the failure looks like an upstream bug (panic, deadlock, repeated crash on the same input), gather the journalctl/docker-logs output and stop — don't try multiple fix attempts on suspected source-code bugs.
