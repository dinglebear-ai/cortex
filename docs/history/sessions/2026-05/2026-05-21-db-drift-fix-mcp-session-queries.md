---
date: 2026-05-21 16:08:08 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 661a077
session id: 08b26792-987b-4f23-9f46-f1b869e5b830
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/08b26792-987b-4f23-9f46-f1b869e5b830.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
---

## User Request

Investigate whether AI sessions are being ingested currently, then trace why an assistant in the axon_rust session concluded they weren't working.

## Session Overview

Confirmed AI session ingestion was healthy, traced a false "not working" conclusion in a prior axon_rust session to the recurring Docker named-volume DB drift bug (container reading a different SQLite file than the CLI/ai-watch), fixed it by recreating the container with `syslog compose up`, and patched the last remaining gap in the dev `docker-compose.yml` that would have reproduced the drift on any dev build.

## Sequence of Events

1. Ran `syslog ai doctor` and `systemctl --user status syslog-ai-watch.service` — confirmed ingestion healthy: 12,004 records, 86 checkpoints, service active 13h, newest indexed entry was the current session
2. Located the axon_rust session that claimed AI sessions weren't working (`d313d646-0282-4bcd-babf-1e26d8f5c03c.jsonl`) — user message was "can you use the syslog mcp to find the 4 most recent sessions for this project"
3. Confirmed the MCP `list_ai_projects` and `sessions` actions exist in the dispatch table (`src/mcp/tools.rs:45-54`) and are wired to real implementations
4. Queried the MCP HTTP endpoint — `list_ai_projects` returned `{"projects":[],"total_projects":0}` despite 12,111 rows with `ai_project IS NOT NULL` in the local DB
5. Ran `syslog compose doctor` — identified DB drift: container mounted Docker named volume `syslog-mcp-data` via a volume (not a bind mount), while CLI/ai-watch used `/home/jmagar/.syslog-mcp/data/syslog.db`
6. Searched `docs/sessions/` — found this exact problem was hit and fixed twice before (2026-05-16, 2026-05-21 02:40 AM); session note from the morning explicitly flagged that `syslog setup repair` doesn't propagate the `name:` fix to installed compose files
7. Verified the installed binary and `~/.syslog-mcp/compose/docker-compose.yml` both already had the `name:` fix; the issue was the running container predated it
8. Ran `syslog compose up` — container recreated, `compose doctor` clean, `list_ai_projects` now returns 18 projects with full data
9. Identified `docker-compose.yml` (dev) was missing `name: ${SYSLOG_MCP_VOLUME_NAME:-syslog-mcp-data}` on its volume declaration — merging with `docker-compose.prod.yml` would strip the name and reproduce drift on any dev build
10. Added `name:` line to `docker-compose.yml`, committed, pushed

## Key Findings

- `src/mcp/tools.rs:45-46,54`: `sessions`, `search_sessions`, `list_ai_projects` actions all exist and work — the prior assistant was wrong to conclude they didn't
- The axon_rust assistant hit empty MCP results and made a faulty "not set up" inference rather than investigating the infrastructure
- `docker-compose.yml:15-17`: top-level `volumes:` block is only exercised when `SYSLOG_MCP_DATA_VOLUME` is unset or a bare name; with an absolute path it's a bind mount and the block is ignored
- `src/setup/firstrun.rs:338-353`: `write_compose_assets` only writes `docker-compose.yml` (generated from embedded `COMPOSE_ASSET` = `docker-compose.prod.yml`); no separate prod file is ever written to `~/.syslog-mcp/compose/`
- This is the third time this drift has been hit: 2026-05-16 (original discovery, v0.25.3 fix), 2026-05-21 02:40 (volume name prefix issue, v0.27.2 fix), 2026-05-21 15:33 (running container predated fixes)

## Technical Decisions

- **`syslog compose up` over manual docker recreate**: uses the same code path as setup repair, ensures env-file substitution is applied correctly
- **`name:` fix in dev compose**: dev compose `extends` prod compose but declares its own top-level `volumes:` block; Compose merges at the field level so the dev declaration overwrites the prod `name:` unless explicitly repeated
- **No `.env` / `.env.example` changes**: `SYSLOG_MCP_DATA_VOLUME=/home/jmagar/.syslog-mcp/data` already set (bind mount active); `.env.example` already documented both vars

## Files Modified

| File | Change |
|------|--------|
| `docker-compose.yml` | Added `name: ${SYSLOG_MCP_VOLUME_NAME:-syslog-mcp-data}` to top-level volume declaration |

## Commands Executed

```bash
# Confirmed ingestion healthy
syslog ai doctor
systemctl --user status syslog-ai-watch.service

# Confirmed MCP has session actions but returns empty (wrong DB)
curl -s -X POST http://localhost:3100/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"syslog","arguments":{"action":"list_ai_projects"}}}'
# → {"projects":[],"total_projects":0}

# Confirmed data exists in local DB
sqlite3 /home/jmagar/.syslog-mcp/data/syslog.db \
  "SELECT COUNT(*) FROM logs WHERE ai_project IS NOT NULL AND ai_project != '';"
# → 12111

# Confirmed drift
syslog compose doctor
# → Error data-mount — container /data is a volume (expected bind to /home/jmagar/.syslog-mcp/data)

# Fixed
syslog compose up
syslog compose doctor  # → Ok data-mount, Ok ai-watch-coord

# Verified MCP now returns data
# → {"projects":[...],"total_projects":18}
```

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `syslog action:list_ai_projects` via MCP | Returns `{"projects":[],"total_projects":0}` | Returns 18 projects with full session/event counts |
| `syslog action:sessions` via MCP | Returns `{"count":0,"sessions":[]}` | Returns live session data |
| `syslog compose doctor` | Error: `data-mount — container /data is a volume` | Ok: both coordination checks pass |
| dev `docker-compose.yml` on `compose up` | Would create project-prefixed named volume, reproduce drift | Uses `name:` pin, stable volume name regardless of `COMPOSE_PROJECT_NAME` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `syslog compose doctor` | Both coordination checks Ok | Ok data-mount, Ok ai-watch-coord | ✅ |
| `list_ai_projects` via MCP | Non-empty projects list | 18 projects returned | ✅ |
| `strings $(which syslog) \| grep name:` | `name: ${SYSLOG_MCP_VOLUME_NAME:-syslog-mcp-data}` | Present | ✅ |

## Risks and Rollback

- Low risk. `syslog compose up` only recreated the container; the DB bind mount means no data was created in or lost from a named volume.
- Rollback: revert `docker-compose.yml` commit `661a077` if the `name:` line causes issues (unlikely — it matches `docker-compose.prod.yml` exactly).

## Decisions Not Taken

- **Update `.env.example` with `SYSLOG_MCP_VOLUME_NAME`**: already documented there; no change needed
- **Add `SYSLOG_MCP_VOLUME_NAME` to `~/.syslog-mcp/.env`**: not needed — user is on a bind mount via `SYSLOG_MCP_DATA_VOLUME`, so the named volume block is never exercised

## Open Questions

- Why did the container start with a named volume after the 02:40 AM session explicitly fixed this and verified `syslog doctor` clean? Likely the container was not recreated after the compose fix was applied — only the file changed, not the running container.

## Next Steps

- **Unstarted**: consider adding a `syslog compose up` call at the end of `syslog setup repair` so the running container is always consistent with the installed compose file after a repair
