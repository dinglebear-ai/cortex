> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-07-16 20:29:20 EDT
repo: git@github.com:jmagar/cortex.git
branch: main
head: cf2291e19510a3b0eedb52e2578cf3a44a656052
session id: 019f6874-3e2c-7de3-be83-9775ee4030f3
transcript: /home/jmagar/.codex/sessions/2026/07/15/rollout-2026-07-15T21-04-36-019f6874-3e2c-7de3-be83-9775ee4030f3.jsonl
working directory: /home/jmagar/workspace/cortex
worktree: /home/jmagar/workspace/cortex
pr: "#138 Codex/cli refresh (https://github.com/jmagar/cortex/pull/138)"
beads: syslog-mcp-2p6ea, syslog-mcp-40dyo, syslog-mcp-2p6ea.1, syslog-mcp-2p6ea.2, syslog-mcp-2p6ea.3, syslog-mcp-2p6ea.4, syslog-mcp-2p6ea.5, syslog-mcp-2p6ea.6, syslog-mcp-2p6ea.7, syslog-mcp-2p6ea.8, syslog-mcp-2p6ea.9, syslog-mcp-2p6ea.10, syslog-mcp-2p6ea.11, syslog-mcp-2p6ea.12, syslog-mcp-qql71, syslog-mcp-4qtox, syslog-mcp-g0gwt, syslog-mcp-hu4bu, syslog-mcp-o3yil
---

# Cortex CLI refresh, Lavra review, and NASHOST deployment

## User Request

Run every Cortex CLI command against live services, debug every unexpected failure, refresh the command surface around one-word command leaves and minimal flags, and preserve shared-layer parity across CLI, REST, and MCP. Then run Lavra review, fix every finding, deploy the latest binary to the local PATH and NASHOST, raise the production database limit to 500 GB, and save the complete session.

## Session Overview

The session rebuilt Cortex's CLI around domain-oriented namespaces and one-word leaves, repaired the live failures named by the user, and kept CLI, REST, and MCP as thin adapters over shared service behavior. An 11-agent Lavra review produced 31 raw findings, reconciled into 16 issue groups; all 16 were fixed and closed. PR #138 merged the work to `main` as `cf2291e1`.

The merged Cortex 3.10.0 binary was then installed to `/home/jmagar/.local/bin/cortex` and packaged into a production image loaded directly onto NASHOST. A direct Compose-template copy briefly resolved the wrong env-file path and triggered default storage cleanup; the container was stopped, the canonical `../.env` path restored, and production was verified healthy. The user subsequently raised the live database ceiling to 500 GiB with a 450 GiB recovery target.

## Sequence of Events

1. Inventoried the existing CLI, shared service, REST, MCP, completion, setup, and live-sweep surfaces in a dedicated `codex/cli-refresh` worktree.
2. Reorganized commands under human-oriented namespaces with one-word command leaves, positional defaults, automatic Atuin discovery, generated help/completion updates, and managed-unit migration.
3. Ran the live CLI sweep and debugged file-tail reconciliation, stale heartbeat cache state, long-running integrity checks, passive checkpoint semantics, REST notification testing, empty assess datasets, and expected deferred operations.
4. Ran the full Lavra review workflow with 11 agents, reconciled 31 raw findings into 16 beads, fixed every finding, and reran focused and broad verification.
5. Proved the branch with 112 live CLI cases, 104 MCP smoke cases, REST smoke, Rust tests, clippy, version sync, plugin validation, and exact runtime-image parity; closed and pushed all review beads.
6. Merged the CLI refresh through PR #138, fast-forwarded local `main`, built Cortex 3.10.0, installed it to PATH, restarted the devhost heartbeat agent, and built a container-compatible production image.
7. Loaded the exact image onto NASHOST and recreated Cortex. Detected that a raw template copy used `.env` relative to the remote `compose/` directory, stopped cleanup, corrected it to `../.env`, and verified the production environment inside the container.
8. Raised NASHOST's database settings from 50/45 GiB to 500/450 GiB, recreated Cortex, and verified zero startup deletions, healthy listeners, zero restarts, and no OOM.
9. Performed repository maintenance for this save: fixed one stale OpenWiki command, created a Dependabot follow-up bead, and removed the clean merged worktree plus obsolete local/remote branches.

## Key Findings

- File-tail registry mutation and runtime reconciliation were not atomic. `src/filetail/registry.rs:65` now snapshots, mutates, reconciles, and restores on failure; `src/app/services/file_tails.rs:32` routes mutations through that shared transaction.
- Query-only DB opens were incorrectly reconciling server-owned jobs and caches. Authoritative startup reconciliation now lives at `src/db/pool.rs:2491` and `src/db/pool.rs:2533`, preserving query-only clients while deleting orphan heartbeat cache rows.
- Session search needed FTS-first candidates plus a complete and freshly updated rollup. The optimized query begins at `src/db/queries.rs:1145`; live search dropped from exceeding 120 seconds to approximately 5-14 seconds, with a measured 7.56-second run.
- REST notification testing diverged from MCP with a 501 response. `/api/notifications/test` now calls the same shared service path as MCP at `src/api.rs:165` and `src/api.rs:898`.
- Integrity checks cannot truthfully complete inside a 120-second sweep on the roughly 48 GB database. The API now starts and polls background jobs at `src/api.rs:1794`; one full scan completed successfully in 73 minutes.
- The NASHOST deployment's canonical env is `/mnt/cache/appdata/cortex/.env`, while its installed Compose file must refer to `../.env`. Copying the repository template without the deployment transform omitted production limits and caused unintended cleanup.

## Technical Decisions

- Kept transport adapters thin: behavioral defaults, validation, and rollback live in shared models and services; CLI, REST, and MCP only parse or serialize transport-specific shapes.
- Honored the no-hyphen command requirement by migrating installed units and documentation instead of retaining compatibility aliases that would keep the old public grammar alive.
- Made integrity checks asynchronous rather than weakening SQLite verification or extending every live sweep beyond its operational budget.
- Treated checkpoint `busy` or incomplete frame counts as an observable incomplete result rather than an unconditional failure for passive mode.
- Built NASHOST's binary through `config/Dockerfile` instead of copying the host-linked executable into Debian, avoiding host/container libc divergence.
- Loaded `ghcr.io/jmagar/cortex:3.10.0` directly and recreated with `--pull never` so a registry image with the same version tag could not replace the reviewed source build.

## Files Changed

PR #138 contains 153 tracked paths: 9 created and 144 modified. The lists below are exhaustive for the squash commit `cf2291e1`.

### Created in PR #138

| Status | Path | Purpose | Evidence |
| --- | --- | --- | --- |
| created | `scripts/live-cli-sweep-helpers.sh` | Reusable live-sweep classification and bounded-execution helpers | `cf2291e1` |
| created | `scripts/test-live-cli-sweep-helpers.sh` | Focused harness tests | `cf2291e1` |
| created | `src/app/services/correlate_events.rs` | Shared correlation service extraction | `cf2291e1` |
| created | `src/app/services/file_tails_tests.rs` | File-tail service rollback and ownership tests | `cf2291e1` |
| created | `src/app/services/logs_tests.rs` | Shared log-service default tests | `cf2291e1` |
| created | `src/app/services/rag_tests.rs` | RAG and incident-query service tests | `cf2291e1` |
| created | `src/cli/output/sessions/hook_incidents.rs` | Human hook investigation output | `cf2291e1` |
| created | `src/cli/output/sessions/hook_incidents_tests.rs` | Hook output regression tests | `cf2291e1` |
| created | `src/setup/managed_units.rs` | Managed systemd command migration | `cf2291e1` |

### Modified in PR #138

All paths below were modified for the CLI refresh, shared-service fixes, tests, docs, setup migration, or live verification. Evidence for every entry is `git diff-tree --no-commit-id --name-status -r cf2291e1`.

```text
CLAUDE.md
README.md
docs/CLI.md
docs/CONFIG.md
docs/INVENTORY.md
docs/api.md
docs/contracts/cli-surface.md
docs/mcp/SCHEMA.md
docs/plugin/CLAUDE.md
docs/plugin/CONFIG.md
docs/plugin/HOOKS.md
docs/repo/SCRIPTS.md
openwiki/ai-incidents.md
openwiki/exposure-surfaces.md
openwiki/log-intelligence.md
openwiki/operations.md
openwiki/quickstart.md
packages/cortex-rmcp/README.md
plugins/cortex/scripts/plugin-setup.sh
plugins/cortex/skills/hook-friction-assessment/SKILL.md
plugins/cortex/skills/incidents/SKILL.md
plugins/cortex/skills/mcp-friction-assessment/SKILL.md
plugins/cortex/skills/skill-improvement-assessment/SKILL.md
scripts/check-runtime-current.sh
scripts/live-cli-sweep.sh
scripts/plugin-setup.sh
scripts/smoke-test-http.sh
scripts/smoke-test.sh
scripts/test-check-runtime-current.sh
src/agent/shell_history.rs
src/agent_deploy.rs
src/agent_deploy_tests.rs
src/api.rs
src/api_tests.rs
src/app/models/context.rs
src/app/models/core.rs
src/app/models/hook_events.rs
src/app/models/mcp_events.rs
src/app/models/rag.rs
src/app/models/skill_events.rs
src/app/service_tests.rs
src/app/services.rs
src/app/services/ai.rs
src/app/services/analytics.rs
src/app/services/file_tails.rs
src/app/services/hook_incidents.rs
src/app/services/incidents.rs
src/app/services/logs.rs
src/app/services/mcp_incidents.rs
src/app/services/rag.rs
src/app/services/skill_incidents.rs
src/app/services/surface_tests.rs
src/app/services/topic_correlate.rs
src/app/services/topic_correlate_tests.rs
src/cli.rs
src/cli/argdefaults.rs
src/cli/argdefaults_tests.rs
src/cli/args.rs
src/cli/args/sessions.rs
src/cli/commands/clock_skew.rs
src/cli/commands/correlate_state.rs
src/cli/commands/file_tails.rs
src/cli/commands/host_state.rs
src/cli/commands/ingest.rs
src/cli/commands/ingest_tests.rs
src/cli/commands/state.rs
src/cli/commands/state_tests.rs
src/cli/complete.rs
src/cli/complete_tests.rs
src/cli/completions/_cortex.zsh
src/cli/dispatch.rs
src/cli/dispatch/surface/analytics.rs
src/cli/dispatch/surface/analytics_tests.rs
src/cli/dispatch/surface/gap.rs
src/cli/dispatch/surface/gap_tests.rs
src/cli/dispatch_command_log.rs
src/cli/dispatch_command_log_tests.rs
src/cli/dispatch_sessions.rs
src/cli/dispatch_tests.rs
src/cli/help.rs
src/cli/help_tests.rs
src/cli/http_client.rs
src/cli/http_client_tests.rs
src/cli/output/sessions.rs
src/cli/parse.rs
src/cli/parse/assess.rs
src/cli/parse/sessions.rs
src/cli/parse/sessions/hooks.rs
src/cli/parse/sessions/mcp_events.rs
src/cli/parse/sessions/mcp_incidents.rs
src/cli/parse/sessions/more.rs
src/cli/parse/sessions/more_tests.rs
src/cli/parse/sessions/ops.rs
src/cli/parse/sessions/skill_incidents.rs
src/cli/parse/sessions_tests.rs
src/cli/parse_admin.rs
src/cli/parse_admin_tests.rs
src/cli/parse_command_log.rs
src/cli/parse_command_log_tests.rs
src/cli/parse_logs.rs
src/cli/parse_logs_tests.rs
src/cli/parse_tests.rs
src/cli/run.rs
src/cli/setup.rs
src/cli/setup/plugin_options.rs
src/cli_tests.rs
src/config.rs
src/config_tests.rs
src/db.rs
src/db/models.rs
src/db/pool.rs
src/db/pool_tests.rs
src/db/queries.rs
src/db/queries_tests.rs
src/filetail/models.rs
src/filetail/models_tests.rs
src/filetail/registry.rs
src/filetail/registry_tests.rs
src/filetail/supervisor.rs
src/filetail/supervisor_tests.rs
src/main.rs
src/main_tests.rs
src/mcp/action_flags.rs
src/mcp/actions.rs
src/mcp/schemas.rs
src/mcp/schemas_tests.rs
src/mcp/tools_tests.rs
src/runtime.rs
src/setup.rs
src/setup/debug_wrapper.rs
src/setup/doctor.rs
src/setup/firstrun.rs
src/setup/heartbeat_agent.rs
src/setup/resolve.rs
src/setup/sessions_index.rs
src/setup/sessions_watch.rs
src/setup/sessions_watch_health.rs
src/setup/sessions_watch_health_tests.rs
src/setup_tests.rs
src/shell_history_ingest.rs
src/surfaces.rs
src/surfaces/api.rs
src/surfaces_tests.rs
tests/test_live.sh
```

### Maintenance and runtime files

| Status | Path | Purpose | Evidence |
| --- | --- | --- | --- |
| modified | `openwiki/exposure-surfaces.md` | Replaced stale `cortex search-sessions` with `cortex sessions search` | `d7283f00` |
| created | `docs/sessions/2026-07-16-cli-refresh-lavra-review-and-nashost-deployment.md` | Complete session artifact | save-to-md workflow |
| modified | `/home/jmagar/.local/bin/cortex` | Installed merged Cortex 3.10.0 release binary | SHA-256 `bb3d01d8ad93bf4faff4e902971f97606619b0bdafc04462c1b87eb55d2a0fac` |
| modified | `/mnt/cache/appdata/cortex/compose/docker-compose.yml` on NASHOST | Installed 3.10.0 image reference and canonical `../.env` path | live Compose inspection |
| modified | `/mnt/cache/appdata/cortex/.env` on NASHOST | Raised max/recovery database sizes to 512000/460800 MiB | live container environment |
| created | `ghcr.io/jmagar/cortex:3.10.0` on local Docker and NASHOST | Exact production image built from merged source | running image ID `sha256:83d6fe5e61476e33740037a6e1f21e5ac5a681a5cf765bb8c00912cbd812d0bb` on NASHOST |

## Beads Activity

| ID | Title | Actions | Final status | Why it mattered |
| --- | --- | --- | --- | --- |
| `syslog-mcp-2p6ea` | Refresh Cortex CLI command surface | Worked and closed | closed | Parent epic for the one-word, minimal-flag CLI refresh |
| `syslog-mcp-40dyo` | Address Lavra review findings for CLI refresh | Created, claimed, inventoried, documented, closed | closed | Parent review task; records 11 agents, 31 raw findings, and 16 resolved groups |
| `syslog-mcp-2p6ea.1` | Make session search rollup complete and fresh | Created and closed | closed | Prevented incomplete or stale session results |
| `syslog-mcp-2p6ea.2` | Preserve historical matches in similar incident search | Created and closed | closed | Applied FTS before candidate caps |
| `syslog-mcp-2p6ea.3` | Honor incident context query filters | Created and closed | closed | Restored query semantics |
| `syslog-mcp-2p6ea.4` | Make completion resolve canonical leaf commands and flags | Created and closed | closed | Kept completion aligned with the new grammar |
| `syslog-mcp-2p6ea.5` | Render hook investigation findings in human output | Created and closed | closed | Fixed missing operator-visible evidence |
| `syslog-mcp-2p6ea.6` | Align MCP event time filters with shared request models | Created and closed | closed | Removed transport divergence |
| `syslog-mcp-2p6ea.7` | Select default hosts from authoritative heartbeats | Created and closed | closed | Avoided stale cache host selection |
| `syslog-mcp-2p6ea.8` | Reject duplicate checkpoint mode arguments | Created and closed | closed | Removed ambiguous CLI parsing |
| `syslog-mcp-2p6ea.9` | Derive safe file tail ownership without extra flags | Created and closed | closed | Made the common add path flag-light and usable |
| `syslog-mcp-2p6ea.10` | Migrate setup units and docs to one-word commands | Created and closed | closed | Prevented installed services from retaining obsolete grammar |
| `syslog-mcp-2p6ea.11` | Wait for background integrity completion in live sweep | Created and closed | closed | Reconciled review feedback with asynchronous production behavior |
| `syslog-mcp-2p6ea.12` | Verify host binary and container parity before live sweep | Created and closed | closed | Ensured live results came from the reviewed image |
| `syslog-mcp-qql71` | Make file tail add duplicate detection atomic | Created and closed | closed | Closed a race in registry mutation |
| `syslog-mcp-4qtox` | Roll back file tail registry mutations when reconcile fails | Created and closed | closed | Fixed committed-but-unapplied state |
| `syslog-mcp-g0gwt` | Enforce topic correlation default window in shared service | Created and closed | closed | Kept every transport on one default |
| `syslog-mcp-hu4bu` | Honor MCP time filters for skill and hook events | Created and closed | closed | Fixed event-filter parity |
| `syslog-mcp-o3yil` | Review Dependabot alert 4 for serde_with panic | Created during maintenance | open | Tracks the medium alert discovered during push rather than burying it in this note |

## Repository Maintenance

### Plans

- Inspected all files under `docs/plans/` and `docs/plans/complete/`.
- Did not move `2026-03-29-unifi-cef-hostname-fix.md`, `2026-05-04-rmcp-stdio-support-follow-up.md`, or `2026-05-11-mnemo-feature-port.md`. Their goals appear substantially implemented, but their unchecked historical task lists and divergent old file/command names make completion status ambiguous without a dedicated plan audit.

### Beads

- Read the CLI refresh epic, the Lavra parent, all 16 finding beads, and current tracker state.
- All CLI refresh and Lavra beads were already closed with live verification evidence.
- Created `syslog-mcp-o3yil` after `git push` surfaced open Dependabot alert #4 for `serde_with`; no other new work was hidden in prose.

### Worktrees and branches

- `git worktree list --porcelain` showed a clean `cli-refresh` worktree and `main`.
- GitHub reported PR #138 merged at `cf2291e1`; the final branch-only analytics files were byte-identical to `main`.
- Removed `/home/jmagar/workspace/cortex/.worktrees/cli-refresh`, deleted local `codex/cli-refresh`, deleted `origin/codex/cli-refresh`, and ran `git worktree prune`.
- Left `origin/marketplace-no-mcp` and `origin/release-please--branches--main--components--cortex` untouched because they are maintained integration/release branches, not stale session branches.

### Stale docs

- Searched current docs for removed hyphenated command forms.
- Historical plans and session logs were left unchanged as historical evidence.
- Fixed the current OpenWiki exposure guide's stale `cortex search-sessions` reference and pushed the path-limited maintenance commit `d7283f00` before creating this artifact.

## Tools and Skills Used

- **Lavra review skill and subagents.** Ran the exact Lavra review workflow with 11 review agents. The final goal-verifier agent was canceled after running too long, so completion was based on reconciled findings plus independent live and test evidence, not a claimed verifier pass.
- **Shell and file tools.** Used `rg`, `sed`, `awk`, `jq`, `find`, `git`, `gh`, `bd`, `sqlite3`, `sha256sum`, and focused file reads. Manual repository edits used `apply_patch`.
- **Rust toolchain.** Used Cargo release builds, focused tests, full `--all-features` tests, clippy with warnings denied, formatting/version checks, and the repo's release wrapper.
- **Live runtime tools.** Used Docker, Docker Compose, SSH to NASHOST, user systemd, `curl`, container logs/inspection, and ZFS snapshot inspection. Missing `XDG_RUNTIME_DIR`/DBus variables were supplied explicitly for noninteractive user-systemd calls.
- **MCP and REST harnesses.** Used `scripts/smoke-test.sh`, `scripts/smoke-test-http.sh`, mcporter-backed MCP calls, and the 112-case live CLI sweep. Labby's local setup probe reported `localhost:8765` unreachable, but the review did not depend on that gateway path.
- **Save skill.** Used `vibin:save-to-md` with the full Codex JSONL transcript. No browser automation or web search was used.

## Commands Executed

| Command | Result |
| --- | --- |
| `CORTEX_ENV_FILE=... bash scripts/live-cli-sweep.sh` | 112 commands; 0 unexpected failures; 7 classified deferred, empty-data, or long-running cases |
| `bash scripts/smoke-test.sh` | 104/104 MCP cases passed; 4 environment-dependent skips |
| `bash scripts/smoke-test-http.sh` | All REST assertions passed |
| `cargo test --all-features` | Library, binary, and integration suites passed; library had 2046 tests with 1 ignored, binary had 540 passed with 1 ignored after the final Atuin changes |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo xtask check-version-sync` | All 14 version carriers passed |
| `cargo build --release --locked` | Built Cortex 3.10.0 and atomically refreshed `~/.local/bin/cortex` |
| `docker build -f config/Dockerfile -t ghcr.io/jmagar/cortex:3.10.0 .` | Built the production-compatible image from merged `main` |
| `docker save ... | ssh nashost docker load` | Loaded the exact image on NASHOST |
| `docker compose ... up -d --force-recreate --pull never` | Recreated production without registry substitution |
| `docker exec cortex ... CORTEX_MAX_DB_SIZE_MB ...` | Verified `512000` max and `460800` recovery values inside the live container |
| `curl -fsS http://127.0.0.1:3100/health` on NASHOST | Returned `{"status":"ok"}` |
| `bd dolt push` | Pushed completed review tracker state |
| `git push origin --delete codex/cli-refresh` | Removed the merged remote topic branch |

## Errors Encountered

- `ingest filetail add` could persist a mutation and then fail reconciliation with `Permission denied`. Root cause was non-atomic registry/reconcile behavior plus ownership assumptions; fixed with derived ownership, insert-if-absent, and rollback.
- `state host` exposed orphan `host_heartbeats_latest` rows while authoritative heartbeat history was empty. Fixed by startup reconciliation and authoritative default-host selection.
- `db integrity --quick` exceeded 120 seconds on production. It was converted to a background start/status contract; a real full job later completed in 73 minutes.
- Passive checkpoints returned busy or incomplete frame counts. The shared result now reports completeness without treating expected passive contention as an unconditional command failure.
- REST notification testing returned 501 and redirected users to MCP. REST now calls the same shared notification service.
- Session search and several analytics queries exceeded the sweep budget or selected stale results. FTS-first filtering, complete rollups, match-recency ordering, and bounded default windows resolved the live failures.
- Noninteractive `systemctl --user` initially failed because DBus environment variables were absent. Supplying `/run/user/1000` and the user bus fixed the operational call.
- The first NASHOST Compose update copied `docker-compose.prod.yml` directly, leaving `env_file: .env` relative to `/mnt/cache/appdata/cortex/compose`. The live container therefore missed `/mnt/cache/appdata/cortex/.env` and ran two default-budget cleanup passes before being stopped. The installed Compose path was corrected to `../.env`, the container was force-recreated, and its production limits were verified from inside the process environment.
- Observed cleanup counters prove at least 380,000 oldest telemetry rows were deleted across the two mistaken passes; the exact upper count is unknown because the replaced containers' final log counters were removed during recreation. Recent error-floor protections remained active.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| CLI grammar | Hyphenated and scattered commands, repeated flags | Domain namespaces with one-word leaves, positionals, and useful defaults |
| Transport behavior | Several REST/MCP/CLI defaults diverged | Shared request models and services own defaults and validation |
| File-tail mutation | Commit could survive failed reconcile | Atomic duplicate detection and rollback |
| Heartbeat state | Stale latest-cache rows could drive results | Orphans reconciled; authoritative heartbeats drive defaults |
| Integrity | Foreground quick check exceeded sweep timeout | Background job start/status returns immediately |
| Notifications | REST returned 501 | REST and MCP use the same notification service |
| Session search | Could exceed 120 seconds and order by session activity | Bounded FTS-first search ordered by match recency |
| Atuin import | Required an explicit path | Resolves `ATUIN_DB_PATH`, XDG data home, or the standard history path |
| Production version | NASHOST ran Cortex 3.9.1 | NASHOST runs the reviewed Cortex 3.10.0 image |
| Database ceiling | 50 GiB max, 45 GiB recovery | 500 GiB max, 450 GiB recovery |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| Live CLI sweep | No unexpected command failures | 112 cases, 0 unexpected failures, 7 expected/deferred | pass |
| MCP smoke | Shared action surface works | 104/104 passed | pass |
| REST smoke | REST parity assertions pass | All assertions passed | pass |
| Full Rust tests | No regression | All library, binary, and integration tests passed | pass |
| Clippy | No warnings | Passed with `-D warnings` | pass |
| Plugin validation | All plugin checks pass | 48/48 passed | pass |
| Runtime parity | Running image equals built image and version | Exact reviewed image and Cortex 3.10.0 | pass |
| NASHOST health | Healthy, no restart/OOM | `healthy`, restarts `0`, OOM `false` | pass |
| Corrected storage startup | No cleanup under production limits | `deleted_rows=0` | pass |
| 500 GiB configuration | 512000/460800 MiB in process env | Verified inside running container | pass |
| Git closeout | Main clean and synchronized | PR #138 merged; maintenance commit pushed | pass |

## Risks and Rollback

- The mistaken env-file path caused irreversible deletion of at least 380,000 oldest telemetry rows from the live database. A ZFS snapshot exists at `cache/appdata@autosnap_2026-07-16_04:00:12_hourly`, but a whole-database rollback would discard newer data and was not attempted.
- Roll back the server binary by restoring the prior image tag in `/mnt/cache/appdata/cortex/compose/docker-compose.yml` and recreating with `--pull never` after ensuring that image exists locally.
- Roll back the storage policy by restoring `CORTEX_MAX_DB_SIZE_MB=51200` and `CORTEX_RECOVERY_DB_SIZE_MB=46080`, then recreating Cortex. Doing so can resume configured deletion if logical size exceeds the lower ceiling.
- Source rollback is a normal revert of squash merge `cf2291e1`; avoid reverting the independent OpenWiki correction unless the old command is restored.

## Decisions Not Taken

- Did not retain old hyphenated command aliases because the user explicitly required a one-word command surface; installed units and docs were migrated instead.
- Did not make integrity checks superficial to fit 120 seconds; retained SQLite verification and changed only the execution contract.
- Did not pull `ghcr.io/jmagar/cortex:3.10.0` during deployment because a registry artifact with the same version could differ from the reviewed source tree.
- Did not restore the 04:00 ZFS database snapshot after cleanup because it would replace hours of newer ingestion and required a separate recovery plan.
- Did not move the three remaining unchecked plan files to `docs/plans/complete/` without a dedicated audit.

## References

- [PR #138: Codex/cli refresh](https://github.com/jmagar/cortex/pull/138)
- [Dependabot alert #4](https://github.com/jmagar/cortex/security/dependabot/4)
- `docs/CLI.md`
- `docs/mcp/DEPLOY.md`
- `openwiki/quickstart.md`
- `/home/jmagar/.codex/plugins/cache/dendrite-no-mcp/lavra/0.7.7/skills/lavra-review/SKILL.md`
- `/home/jmagar/.codex/plugins/cache/dendrite-no-mcp/vibin/local/skills/save-to-md/SKILL.md`

## Open Questions

- The exact final telemetry-row deletion count from the two misconfigured startup passes cannot be recovered from the removed container logs; observed counters establish a lower bound of 380,000 rows.
- The three unchecked files remaining under `docs/plans/` appear historically implemented but need a focused plan-to-current-code audit before they can be moved safely.
- Dependabot alert #4 remains open pending reachability analysis and dependency remediation under `syslog-mcp-o3yil`.

## Next Steps

- **Unfinished session work:** none for the CLI refresh, Lavra findings, binary deployment, or 500 GiB production configuration.
- **Tracked follow-up:** work `syslog-mcp-o3yil`, update or dismiss the affected `serde_with` dependency with evidence, and run the full Rust quality gates.
- **Operational follow-up:** monitor NASHOST database growth against the new 500 GiB ceiling and verify ZFS replication/snapshot coverage for `/mnt/cache/appdata/cortex/data`.
- **Documentation follow-up:** audit the three remaining unchecked plan files against current code before moving them to `docs/plans/complete/`.
