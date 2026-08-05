---
date: 2026-05-09 01:46:00 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 4a9228d
agent: Codex
session id: unavailable
transcript: unavailable; no recent ~/.codex project jsonl transcript found
working directory: /home/jmagar/workspace/syslog-mcp
worktree: /home/jmagar/workspace/syslog-mcp 4a9228d [main]
---

# Syslog MCP OAuth and Plugin Deploy Fix

## User Request

Fix `codex mcp login syslog` failing with `Error: No authorization support detected`, make the initial plugin setup work without extra env files or manual overrides, restore OAuth config, deploy the live Docker service, and push a new image.

## Session Overview

The OAuth discovery issue was fixed and deployed. The live `syslog-mcp` container now runs `0.17.5` from `ghcr.io/jmagar/syslog-mcp:latest`, OAuth metadata is public under the `/mcp/.well-known/*` path, and `codex mcp login syslog` now reaches the browser authorization URL instead of failing discovery.

## Sequence of Events

1. Investigated the live service and found it was still running stale plugin behavior even after the repo/image work had been pushed.
2. Hot-patched the installed plugin cache so setup would preserve OAuth settings and stop rebuilding from installed source-cache contents by default.
3. Recreated the live Docker container from the published GHCR image and verified OAuth discovery publicly.
4. Committed and pushed the durable plugin deploy fix as `bae3abd fix: pull published image by default`, version `0.17.4`.
5. Found the GitHub MCP integration failure was a separate `syslog status` scope-map regression, fixed it, and pushed `4a9228d fix: allow status under read scope`, version `0.17.5`.
6. Rebuilt and redeployed the live service from the `0.17.5` published image.

## Key Findings

- The installed plugin cache can contain Rust source files, so source-file detection alone made plugin setup build locally from stale cache contents instead of pulling the published image.
- The durable fix is in `scripts/plugin-setup.sh:443`: local Docker builds now require `CLAUDE_PLUGIN_OPTION_BUILD_LOCAL=true`; normal plugin deploys pull `ghcr.io/jmagar/syslog-mcp:latest`.
- `syslog status` was implemented as a tool action but was missing from the mounted-auth read-scope map, causing `requires scope: syslog:__deny__` in CI.
- `src/mcp/rmcp_server.rs:347` now maps `status` to `syslog:read`.
- `src/mcp/rmcp_server_tests.rs:580` and `src/mcp/rmcp_server_tests.rs:661` now cover `status` in both read-scope allow and empty-scope deny cases.

## Technical Decisions

- Kept one canonical runtime `.env` for plugin Docker deployment instead of preserving separate `syslog-mcp.env`-style override files.
- Kept OAuth active on `syslog.example.internal` because the reverse proxy was restored and Codex needs server-side OAuth discovery at the MCP path.
- Made local Docker builds opt-in with `CLAUDE_PLUGIN_OPTION_BUILD_LOCAL=true` so plugin setup is stable for users and still supports source development.
- Mapped `status` to `syslog:read` instead of making it unauthenticated, because it exposes runtime status and should behave like the other read-only MCP actions.

## Files Modified

- `scripts/plugin-setup.sh`: plugin Docker deploy now pulls the published GHCR image by default and only builds locally when explicitly requested.
- `src/mcp/rmcp_server.rs`: `status` action added to read-scope mapping.
- `src/mcp/rmcp_server_tests.rs`: scope tests updated to include `status`.
- `Cargo.toml`, `Cargo.lock`, `.claude-plugin/plugin.json`: version bumped through `0.17.5`.
- `CHANGELOG.md`: documented `0.17.4` plugin deploy behavior and `0.17.5` status scope fix.
- Installed plugin cache hot-patched at `/home/jmagar/.claude/plugins/cache/jmagar-lab/syslog/7e4cde457197/scripts/plugin-setup.sh` so the currently installed plugin stopped reverting the live service before the next install/update cycle.

## Commands Executed

- `git add . && git commit -m "fix: pull published image by default"`: committed the durable published-image deploy behavior.
- `git push origin main`: pushed `bae3abd`; pre-push ran the full local test suite successfully.
- `docker compose pull syslog-mcp && docker compose up -d --force-recreate --no-build syslog-mcp`: recreated the live service from the published image.
- `gh run watch 25589420870 --repo jmagar/syslog-mcp --exit-status`: confirmed the `0.17.4` Docker image build succeeded.
- `git add . && git commit -m "fix: allow status under read scope"`: committed the `status` scope-map fix.
- `git push origin main`: pushed `4a9228d`; pre-push ran the full local test suite successfully.
- `gh run watch 25589643314 --repo jmagar/syslog-mcp --exit-status`: confirmed the `0.17.5` Docker image build succeeded.

## Errors Encountered

- `codex mcp login syslog` originally failed with `No authorization support detected`; root cause was OAuth discovery not being available at the path Codex probes for the configured MCP endpoint.
- The live service initially kept reverting because plugin setup treated installed cache source files as a source checkout and built locally instead of pulling the new GHCR image.
- CI MCP integration failed on `syslog status` with `forbidden: requires scope: syslog:__deny__`; root cause was a missing entry in `required_scope_for()`.
- CI `Security Audit` still fails on RustSec advisory `RUSTSEC-2023-0071` for `rsa`, reached through `lab-auth` / `jsonwebtoken`; this was observed as pre-existing and separate from the OAuth and deploy fixes.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| Codex login | `codex mcp login syslog` failed discovery | Command prints an OAuth authorization URL |
| Plugin Docker setup | Installed cache could trigger a local build | Published GHCR image is pulled unless local build is explicitly requested |
| OAuth metadata | Codex path discovery failed | `/mcp/.well-known/oauth-authorization-server` and protected-resource metadata respond publicly |
| `syslog status` under mounted auth | Denied by `syslog:__deny__` | Allowed with `syslog:read` |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `docker exec syslog-mcp syslog --version` | running deployed version | `syslog-mcp 0.17.5` | PASS |
| `docker inspect syslog-mcp --format ...` | healthy live container | `Health=healthy`, `RestartCount=0`, started `2026-05-09T02:55:50Z` | PASS |
| `curl https://syslog.example.internal/mcp/.well-known/oauth-authorization-server` | OAuth issuer and endpoints | issuer `https://syslog.example.internal`, authorize/token/register endpoints present | PASS |
| unauthenticated `POST https://syslog.example.internal/mcp` | `401` with protected-resource metadata | `401`, `WWW-Authenticate: Bearer resource_metadata="https://syslog.example.internal/mcp/.well-known/oauth-protected-resource"` | PASS |
| `timeout 25s codex mcp login syslog` | no discovery error | printed browser authorization URL; timed out only because browser completion was not performed | PASS |
| `cargo test` | all tests pass locally | 285 lib tests, 9 main tests, and integration tests passed | PASS |
| `cargo clippy -- -D warnings` | no warnings | completed successfully | PASS |
| `gh run watch 25589643314 --repo jmagar/syslog-mcp --exit-status` | Docker image build succeeds | build-and-push succeeded | PASS |
| `gh run watch 25589643306 --repo jmagar/syslog-mcp --exit-status` | CI succeeds except known audit issue | tests, formatting, clippy, secret scan, MCP integration passed; Security Audit failed on RustSec `rsa` advisory | PARTIAL |

## Risks and Rollback

- The live service is now tied to the published `latest` image path; rollback is to set `SYSLOG_MCP_VERSION` in `/home/jmagar/.claude/plugins/data/syslog-jmagar-lab/.env` or compose env and recreate with a known-good tag.
- OAuth config remains in the live plugin data `.env`; if gateway/reverse-proxy routing changes again, re-check both authorization-server and protected-resource metadata under `/mcp/.well-known/*`.
- The RustSec `rsa` advisory remains unresolved because it comes through upstream `lab-auth` / `jsonwebtoken`; fixing it likely requires dependency changes outside this narrow OAuth deploy fix.

## Decisions Not Taken

- Did not remove OAuth or switch back to `NO_AUTH=true`; the user restored the proxy and requested OAuth config back.
- Did not keep a separate `syslog-mcp.env`; plugin deployment now uses the single canonical `.env`.
- Did not make `syslog status` unauthenticated; it was treated as a read-only MCP action requiring `syslog:read`.

## References

- GitHub Actions build `25589643314`: Docker image build for `4a9228d`, completed successfully.
- GitHub Actions CI `25589643306`: tests, formatting, clippy, secret scan, and MCP integration passed; Security Audit failed on RustSec `rsa`.
- Commits: `27dd47c`, `bae3abd`, `4a9228d`.

## Open Questions

- Whether to suppress, document, or eliminate the `rsa` RustSec advisory from the `lab-auth` / `jsonwebtoken` dependency path.
- Whether the eventual gateway at `mcp.example.internal/syslog` should be modeled as a generic upstream MCP registration flow instead of service-specific OAuth wiring.

## Next Steps

Started but not completed:

- None for the OAuth discovery/deploy issue; live service is fixed and redeployed.

Follow-on tasks:

- Decide how to handle the RustSec `rsa` audit failure.
- If moving to the new gateway, add a general upstream MCP server model that lets users choose between the unified Lab endpoint and a per-server gateway path.
