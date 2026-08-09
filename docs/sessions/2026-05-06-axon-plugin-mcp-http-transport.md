> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-05-06 22:59:31 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 0dcfee2
plan: none
agent: Claude (claude-sonnet-4-6)
session id: d8f1293d-fbb4-4073-8912-e174fa706dff
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/d8f1293d-fbb4-4073-8912-e174fa706dff.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
---

## User Request

Diagnose why the syslog plugin MCP server showed "SDK auth failed / not authenticated" in the plugin panel, then investigate why the axon plugin's MCP server was not appearing in `claude mcp list` at all, and fix the axon plugin to use the same HTTP transport pattern as syslog.

## Session Overview

Two distinct problems were diagnosed and resolved: (1) syslog's "SDK auth failed" was identified as a cosmetic UI issue caused by the Claude Code SDK attempting OAuth/OIDC discovery — the server itself is healthy. (2) The axon plugin's MCP server was crashing immediately on startup due to missing `TEI_URL`, because the stdio subprocess spawning model combined with no default for a required env var caused instant failure. The axon plugin was redesigned to match syslog's HTTP transport + systemd service pattern, removing docker mode as a dead-end approach.

## Sequence of Events

1. User showed syslog plugin panel with "SDK auth failed / not authenticated"
2. Ran `/syslog:doctor` skill — executed full health check across MCP, HTTP, systemd, syslog port
3. Identified root cause: Claude Code SDK probes `GET /mcp/.well-known/openid-configuration` (OAuth discovery), syslog-mcp doesn't implement OIDC, SDK reports auth failure even though Bearer token auth works correctly
4. User noted axon plugin was not appearing in `claude mcp list` despite showing as ✔ enabled in favorites
5. Diagnosed axon: `axon mcp` (stdio subprocess) exited immediately with `TEI_URL environment variable is required`; plugin data dir was empty — no userConfig values had ever been saved
6. Read syslog plugin files (`plugins/.mcp.json`, `.claude-plugin/plugin.json`, `scripts/plugin-setup.sh`) to establish the reference pattern
7. Discovered `axon serve mcp --transport http` is supported; default port 8001 (`AXON_MCP_HTTP_PORT`), default host `127.0.0.1` (`AXON_MCP_HTTP_HOST`)
8. Found security constraint: non-loopback binding refuses to start without `AXON_MCP_HTTP_TOKEN`
9. Rewrote axon plugin: `.mcp.json` → HTTP transport; `plugin.json` → added `api_token`, `server_url`, `mcp_host`, `mcp_port`, default for `tei_url`; `hooks.json` → calls `plugin-setup.sh`; new `scripts/plugin-setup.sh` mirrors syslog's pattern
10. Added `docker-compose.yml` for docker mode; user questioned host networking approach
11. Corrected to proper port mapping + auth token; user then asked about URL normalization for docker vs systemd
12. Read existing `docker-compose.yaml` and `.env` — discovered axon already handles URL rewriting in `environment` block (`axon-qdrant:6333`, `axon-tei:80`, `axon-chrome:6000`) and uses `axon` network (not `jakenet`)
13. Found `AXON_MCP_API_KEY` in `.env` vs `AXON_MCP_HTTP_TOKEN` in source — fixed locally (`.env` is gitignored)
14. User correctly pointed out plugin compose's `context: .` would fail when copied to data dir (no source available there)
15. Dropped docker mode entirely — plugin ships pre-built binary, systemd is the right path; repo's own `docker-compose.yaml` handles the full stack
16. Committed and pushed three commits to `axon_rust` main

## Key Findings

- `src/mcp/routes.rs` (axon): non-loopback bind without `AXON_MCP_HTTP_TOKEN` → hard error at startup
- `crates/core/config/types/config.rs:454-455` (axon): `mcp_http_port` default is `8001`, env `AXON_MCP_HTTP_PORT`
- `crates/core/config/parse/build_config.rs:481` (axon): `mcp_http_host` default is `127.0.0.1`, env `AXON_MCP_HTTP_HOST`
- `docker-compose.yaml` (axon): already overrides `QDRANT_URL`/`TEI_URL`/`AXON_CHROME_REMOTE_URL` to container DNS in `environment` block — no separate docker URL fields needed
- `.env` (axon): `AXON_MCP_API_KEY` was silently ignored; correct var is `AXON_MCP_HTTP_TOKEN`
- Syslog "SDK auth failed" is cosmetic: server logs show repeated `Unauthorized MCP request rejected method=GET path=/mcp/.well-known/openid-configuration` — the SDK probes OIDC before connecting, syslog doesn't implement it

## Technical Decisions

- **HTTP transport over stdio**: stdio subprocess model is fragile — one missing env var crashes it before Claude can connect. HTTP transport connects to a pre-running persistent service that survives config gaps.
- **systemd only, no docker mode**: plugin ships a pre-built binary; `docker compose up` from the plugin data dir has no build context. Docker is for the full stack via the repo's `docker-compose.yaml`, not for the plugin's MCP surface.
- **Always set `AXON_MCP_HTTP_TOKEN`**: even when binding `127.0.0.1` (which doesn't require it), setting the token consistently means no code path changes when the user later reconfigures the host.
- **`127.0.0.1` as default MCP host**: loopback binding satisfies axon's startup security policy without requiring auth; plugin connects locally anyway.
- **`axon` network (not `jakenet`)**: matched to the existing `docker-compose.yaml` which uses a `bridge` network named `axon`.

## Files Modified

All changes are in `~/workspace/axon_rust` (not this repo):

| File | Status | Purpose |
|------|--------|---------|
| `plugins/.mcp.json` | modified | Switched from stdio subprocess to HTTP transport pointing to `${user_config.server_url}/mcp` with Bearer auth header |
| `.claude-plugin/plugin.json` | modified | Changed `"mcp"` key to `"mcpServers"`, added `api_token`/`server_url`/`mcp_host`/`mcp_port` userConfig, gave `tei_url` a default |
| `plugins/hooks/hooks.json` | modified | Replaced inline symlink command with call to `scripts/plugin-setup.sh` |
| `scripts/plugin-setup.sh` | created | Full SessionStart hook: writes `axon.env`, installs/updates `axon-mcp.service`, starts/restarts on change |
| `docker-compose.yml` | created then deleted | Docker mode was added then removed as unworkable |
| `.env` (local only) | modified | Renamed `AXON_MCP_API_KEY` → `AXON_MCP_HTTP_TOKEN`; not committable (gitignored) |

## Commands Executed

```bash
# Port/host discovery
grep -n "mcp_http_port\|mcp_port" crates/core/config/parse/build_config.rs
# → mcp_http_port: env_port("AXON_MCP_HTTP_PORT", 8001)

# Startup behavior
axon mcp 2>&1
# → error: TEI_URL environment variable is required

axon serve mcp --help
# → --transport: stdio, http, or both; --tei-url overrides TEI_URL

# Syslog health checks
curl -sf http://localhost:3100/health        # → {"status":"ok"}
systemctl --user is-active syslog-mcp       # → active
nc -z -w2 localhost 1514 && echo PASS       # → PASS
curl -s -X POST http://localhost:3100/mcp \
  -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer Buzzaroo" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"syslog","arguments":{"action":"stats"}}}'
# → 402 logs, 4 hosts, write_blocked=false, 0.47MB
```

## Errors Encountered

- **`axon mcp` crash**: `TEI_URL environment variable is required` — root cause: stdio subprocess with `required: true` field and no default; plugin data dir was empty so `${user_config.tei_url}` resolved to empty string. Fixed by switching to HTTP transport and adding `"default": "http://localhost:52000"`.
- **MCP curl `Not Acceptable`**: Initial MCP health check calls omitted `Accept: application/json, text/event-stream` header. Fixed by adding the header.
- **Docker compose `context: .` failure**: Plugin setup copies compose file to data dir; build context becomes an empty data dir with no `Dockerfile`. Fixed by dropping docker mode entirely.
- **`AXON_MCP_API_KEY` ignored**: Wrong env var name in `.env`; axon reads `AXON_MCP_HTTP_TOKEN`. Fixed locally.

## Behavior Changes (Before/After)

| | Before | After |
|---|---|---|
| Axon plugin in `claude mcp list` | Absent — stdio subprocess crashed on start | Will appear as Connected once plugin is updated and SessionStart fires |
| Axon MCP transport | stdio subprocess (`axon mcp`) | HTTP to pre-running `axon-mcp.service` (`axon serve mcp`) |
| Axon service lifecycle | No management — binary symlink only | systemd user service with env file, auto-restart, daemon-reload on update |
| `AXON_MCP_HTTP_TOKEN` in production `.env` | Set as `AXON_MCP_API_KEY` (wrong name, silently ignored) | Set as `AXON_MCP_HTTP_TOKEN` (correct, token now enforced) |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `curl -sf http://localhost:3100/health` | `{"status":"ok"}` | `{"status":"ok"}` | ✓ PASS |
| `systemctl --user is-active syslog-mcp` | `active` | `active` | ✓ PASS |
| `nc -z -w2 localhost 1514` | exit 0 | exit 0 | ✓ PASS |
| MCP `action=stats` | isError=false | 402 logs, 4 hosts | ✓ PASS |
| MCP `action=hosts` | isError=false | devhost, WINHOST, laptophost, edgehost | ✓ PASS |
| MCP `action=tail n=1` | isError=false | timestamp 2026-05-07T01:41:19 | ✓ PASS |

## Risks and Rollback

- **axon plugin**: If `plugin-setup.sh` fails on SessionStart, axon MCP won't be available. Rollback: `systemctl --user stop axon-mcp && systemctl --user disable axon-mcp`. The old cached plugin version (50b1a240) is still in the plugin cache and can be re-pinned.
- **`AXON_MCP_HTTP_TOKEN` rename**: If anything else was reading `AXON_MCP_API_KEY`, it now silently breaks. Check other tools/scripts that read axon's `.env`.

## Decisions Not Taken

- **OAuth/OIDC endpoints on syslog-mcp**: Would fix the "SDK auth failed" cosmetic issue in the plugin panel. Rejected — server is fully functional; the UI warning is misleading but harmless.
- **Docker mode for axon plugin**: Requires pre-built image or build context in the data dir — neither is viable. The repo's `docker-compose.yaml` already handles the full stack.
- **`network_mode: host` in docker compose**: Would have avoided the non-loopback auth requirement, but breaks jakenet connectivity and is the wrong pattern for container networking. Rejected in favor of proper port mapping + token auth.
- **Separate `docker_qdrant_url`/`docker_tei_url` userConfig fields**: Not needed — the compose `environment` block already overrides URLs to container DNS names, same as the existing `docker-compose.yaml`.

## Open Questions

- **Axon plugin activation**: The plugin update + SessionStart has not been run yet. The `api_token` value needs to be set in the plugin UI before `plugin-setup.sh` will succeed (`API token is required`).
- **`AXON_MCP_HTTP_TOKEN` in existing containers**: The running `axon` container (full stack via `docker-compose.yaml`) needs to pick up the corrected env var. Restart or re-`docker compose up` required.
- **Syslog "SDK auth failed" UX**: Three fix options identified (remove token, implement OIDC, accept cosmetic warning) — no decision made.

## Next Steps

**Unfinished (started but not completed):**
- Run the axon plugin update (`/plugin` → axon → Update) and verify `axon-mcp.service` installs and starts cleanly
- Set `api_token` in axon plugin userConfig UI

**Follow-on tasks:**
- Consider adding a `/axon:doctor` skill mirroring `/syslog:doctor` to check Qdrant, TEI, Chrome reachability from the axon plugin
- Restart the full axon docker stack to pick up `AXON_MCP_HTTP_TOKEN` rename in `.env`
