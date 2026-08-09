> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-06-12 16:09:09 EST
repo: git@github.com:jmagar/cortex.git
branch: main
head: 8f092ac
plan: docs/superpowers/plans/2026-06-11-file-tail-ingest.md
session id: c82ab23f-fbee-4747-bf81-460a7c156c35
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-cortex/c82ab23f-fbee-4747-bf81-460a7c156c35.jsonl
working directory: /home/jmagar/workspace/cortex
worktree: /home/jmagar/workspace/cortex 8f092ace [main]
pr: "#73 feat: add managed file-tail ingest https://github.com/jmagar/cortex/pull/73"
beads: syslog-mcp-dqlv5, syslog-mcp-6y96m, syslog-mcp-7j9hn
---

# File-tail ingest closeout

## User Request

The session started from Cortex ingest pressure and host-parity debugging: confirm which agents and forwarders were pushing logs, keep full agent parity for syslog and Docker logs, preserve special SWAG/Authelia/AdGuard style log ingestion, and implement a CLI/API/MCP action for explicit file-tail ingestion. The final request was to save the full session to markdown after PR #73 was merged and the branch/worktree cleanup was complete.

## Session Overview

Implemented managed file-tail ingestion for Cortex, reviewed it with PR review agents twice, fixed CodeRabbit and agent findings, verified locally and in Docker, merged PR #73 into `main`, removed the feature worktree and branches, and fast-forwarded the main worktree to `origin/main`.

The final repository state before this note was clean on `main` at merge commit `8f092ac`, with no extra registered worktrees and no local feature branch left.

## Sequence of Events

1. Investigated ingest pressure and log-source ownership, confirming the goal was not selective ingestion but full agent parity plus explicit support for additional app log files.
2. Created a dedicated worktree/branch for managed file-tail ingest and produced the implementation plan in `docs/superpowers/plans/2026-06-11-file-tail-ingest.md`.
3. Implemented file-tail source modeling, registry persistence, supervisor tailing, CLI/API/MCP control surfaces, docs, smoke tests, and version/changelog updates.
4. Opened PR #73 and dispatched PR review toolkit agents, then addressed review findings around admin scope, path safety, query-only mode, Docker smoke coverage, status observability, duplicate adds, rotation handling, and `hostname` attribution.
5. Dispatched review agents again, fixed the remaining findings, and resolved a Docker live-smoke failure by tightening both supervisor add/append handoff and hyphenated FTS5 marker queries.
6. Verified with Rust unit/integration tests, clippy, shell syntax checks, version sync, Docker live smoke, GitHub CI, CodeRabbit, and pre-push hooks.
7. Merged PR #73, deleted the merged feature worktree/local branch/remote branch, and pulled latest `main`.
8. Ran the `vibin:save-to-md` maintenance pass and wrote this session artifact.

## Key Findings

- The old syslog-forwarder-only approach could not preserve the richer SWAG/Authelia/AdGuard/fail2ban style log ingestion cleanly once syslog forwarding was stopped on `nashost` and `backuphost`; Cortex needed first-class file-tail source management.
- File-tail add needed explicit source attribution: `hostname` is required in the model and schema so rows are not silently attributed to the Cortex container host. See `src/file_tail/models.rs:94` and `docs/mcp/SCHEMA.md:153`.
- Mutating `file_tails` operations must be long-running-server-only; stdio/query-only runtimes can read the registry but cannot spawn competing local tailers. This was wired through `src/runtime.rs` and `src/app/services/file_tails.rs`.
- Add-then-append had a race: `file_tails add` could return before the tail task opened the file. The supervisor now initializes the initial checkpoint before `reconcile()` returns in `src/file_tail/supervisor.rs:98` and `src/file_tail/supervisor.rs:106`.
- Docker smoke initially failed because the marker was hyphenated and searched as a raw FTS5 query. Smoke scripts now phrase-quote file-tail markers.
- Writer backpressure is now visible through `blocked_on_writer_since` in `src/file_tail/models.rs:94` and global `status.file_tails.blocked_count` in `src/mcp/tools.rs:415`.

## Technical Decisions

- Use a JSON registry under the Cortex data directory for managed sources, because file-tail configuration needs to be mutated by CLI/API/MCP and survive process restarts without editing Compose or config files.
- Keep path safety conservative: default production root is `/file-tail-root`; additional roots require `CORTEX_FILE_TAIL_ALLOWED_ROOTS`; symlinks and sensitive Cortex mounts are denied.
- Require `hostname` on add instead of inferring it, because these file-tail rows represent remote/service log files mounted into Cortex, not logs generated by the Cortex container itself.
- Wait for durable ingest acknowledgement before advancing line checkpoints, so backpressure does not lose unread file positions.
- Avoid retrying admin `POST /api/file-tails` on HTTP 503, because a replay can duplicate a mutating add/remove request.
- Preserve full agent parity separately from file-tail sources: syslog and Docker logs still belong to the deployed agents; file-tail ingest is for explicit extra log files such as SWAG, Authelia, AdGuard, and fail2ban.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `.env.example` | - | Document file-tail env and admin-token settings | `git diff --name-status 5632f78..8f092ac` |
| modified | `CHANGELOG.md` | - | Record `1.20.1` review-hardening release notes | `git diff --name-status 5632f78..8f092ac` |
| modified | `CLAUDE.md` | - | Update action counts/admin scope and file-tail reference | `git diff --name-status 5632f78..8f092ac` |
| modified | `Cargo.lock` | - | Sync package version metadata | `git diff --name-status 5632f78..8f092ac` |
| modified | `Cargo.toml` | - | Bump package version | `git diff --name-status 5632f78..8f092ac` |
| modified | `README.md` | - | Document managed file-tail ingest and examples | `git diff --name-status 5632f78..8f092ac` |
| modified | `config.toml` | - | Add file-tail config defaults/examples | `git diff --name-status 5632f78..8f092ac` |
| modified | `docker-compose.prod.yml` | - | Add read-only `/file-tail-root` mount wiring | `docker-compose.prod.yml:53` |
| modified | `docker-compose.yml` | - | Add local file-tail mount wiring | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/CLI.md` | - | Document `cortex file-tail` commands | `docs/CLI.md:38` |
| modified | `docs/CONFIG.md` | - | Document file-tail roots and examples | `docs/CONFIG.md:133` |
| modified | `docs/INVENTORY.md` | - | Refresh inventory-related docs touched by merge | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/api.md` | - | Document `/api/file-tails` | `docs/api.md:66` |
| modified | `docs/contracts/log-filter-surface.md` | - | Update source/filter contract | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/contracts/mcp-actions-current.md` | - | Add file-tail action to MCP action docs | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/contracts/source-kinds.md` | - | Add `file-tail` source-kind contract | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/mcp/SCHEMA.md` | - | Document schema constraints and required hostname | `docs/mcp/SCHEMA.md:153` |
| modified | `docs/mcp/TESTS.md` | - | Update MCP smoke/test docs | `git diff --name-status 5632f78..8f092ac` |
| modified | `docs/mcp/TOOLS.md` | - | Update MCP tool inventory | `git diff --name-status 5632f78..8f092ac` |
| created | `docs/sessions/2026-06-11-file-tail-ingest.md` | - | Save implementation/review session notes | `git diff --name-status 5632f78..8f092ac` |
| created | `docs/superpowers/plans/2026-06-11-file-tail-ingest.md` | - | Implementation plan artifact | `git diff --name-status 5632f78..8f092ac` |
| modified | `mcpb/manifest.json` | - | Sync bundle version | `git diff --name-status 5632f78..8f092ac` |
| modified | `plugins/cortex/skills/cortex/SKILL.md` | - | Update skill docs for file-tail action | `git diff --name-status 5632f78..8f092ac` |
| modified | `scripts/check-rust-module-size.sh` | - | Update module-size guard tooling | `git diff --name-status 5632f78..8f092ac` |
| created | `scripts/rust-module-size.allow` | - | Add module-size allowlist | `git diff --name-status 5632f78..8f092ac` |
| modified | `scripts/smoke-test.sh` | - | Add/fix file-tail smoke probes and admin-scope gates | `git diff --name-status 5632f78..8f092ac` |
| modified | `server.json` | - | Sync registry package version/image tag | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/agent_deploy.rs` | - | Preserve agent parity for syslog/Docker deployment settings | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/agent_deploy_tests.rs` | - | Test agent deployment parity behavior | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/api.rs` | - | Add admin file-tail route and caller audit logging | `src/api.rs` |
| modified | `src/api_tests.rs` | - | Cover file-tail route auth/admin behavior | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/app.rs` | - | Re-export file-tail app models | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/app/models/ops.rs` | - | Add file-tail operation models to app layer | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/app/service_tests.rs` | - | Add service-layer file-tail tests | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/app/services.rs` | - | Mount file-tail registry/control callbacks | `git diff --name-status 5632f78..8f092ac` |
| created | `src/app/services/file_tails.rs` | - | Implement file-tail service operations | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/app/services/filters.rs` | - | Support file-tail source-kind filtering | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli.rs` | - | Add file-tail CLI command routing | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/args.rs` | - | Add file-tail CLI arg structs | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/commands.rs` | - | Register file-tail command parser | `git diff --name-status 5632f78..8f092ac` |
| created | `src/cli/commands/file_tails.rs` | - | Parse file-tail subcommands | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/dispatch.rs` | - | Dispatch CLI file-tail requests | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/dispatch_tests.rs` | - | Test CLI dispatch output/status fields | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/help.rs` | - | Document file-tail help | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/help_tests.rs` | - | Cover file-tail help catalog | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/http_client.rs` | - | Add file-tail API client and non-retry admin POST | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/http_client_tests.rs` | - | Test 503 non-retry and redaction behavior | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/parse.rs` | - | Route file-tail parse branch | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/parse_tests.rs` | - | Test required `--host` and command parsing | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/cli/run.rs` | - | Run file-tail command arm | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/config.rs` | - | Add file-tail and agent parity config fields | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/config_tests.rs` | - | Cover config additions | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/enrich/dispatch.rs` | - | Dispatch file-tail source-kind enrichment | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/enrich/parser.rs` | - | Add file-tail source kind | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/enrich/parser_tests.rs` | - | Test file-tail source-kind string | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail.rs` | - | File-tail module root and test hooks | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/models.rs` | - | Source/request/status models | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/models_tests.rs` | - | Model validation tests | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/path_policy.rs` | - | Path allowlist and safe-open policy | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/path_policy_tests.rs` | - | Path policy tests | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/registry.rs` | - | Persistent source registry | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/registry_tests.rs` | - | Registry persistence tests | `git diff --name-status 5632f78..8f092ac` |
| created | `src/file_tail/supervisor.rs` | - | Tail supervisor, checkpointing, rotation handling | `src/file_tail/supervisor.rs:98` |
| created | `src/file_tail/supervisor_tests.rs` | - | Supervisor ingestion, ack, rotation, race tests | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/ingest.rs` | - | Durable ingest acknowledgement plumbing | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/lib.rs` | - | Export file-tail module | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/main.rs` | - | Runtime/CLI integration for file-tail command | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/mcp/actions.rs` | - | Register `file_tails` action | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/mcp/schemas.rs` | - | Add action-specific JSON Schema constraints | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/mcp/schemas_tests.rs` | - | Test schema conditions and required hostname | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/mcp/tools.rs` | - | Dispatch file-tail action and expose blocked status | `src/mcp/tools.rs:415` |
| modified | `src/mcp/tools_tests.rs` | - | Test MCP action/admin/status behavior | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/receiver/writer.rs` | - | Support durable writer acknowledgement semantics | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/receiver/writer_tests.rs` | - | Cover writer acknowledgement behavior | `git diff --name-status 5632f78..8f092ac` |
| modified | `src/runtime.rs` | - | Wire supervisor into long-running runtime only | `git diff --name-status 5632f78..8f092ac` |
| modified | `tests/mcporter/test-tools.sh` | - | Add/fix mcporter file-tail smoke probe | `git diff --name-status 5632f78..8f092ac` |
| modified | `tests/test_live.sh` | - | Add/fix Docker/live file-tail smoke probe | `git diff --name-status 5632f78..8f092ac` |

## Beads Activity

| bead | title | actions observed | final status | why it mattered |
|---|---|---|---|---|
| `syslog-mcp-dqlv5` | Implement Unraid heartbeat-agent parity | Closed on 2026-06-11 with reason stating Unraid syslog file tailing, Docker unix socket streaming, deploy syslog target wiring, and live `nashost`/`backuphost` verification were implemented | closed | This was the agent-parity work immediately preceding managed file-tail ingest. |
| `syslog-mcp-6y96m` | Managed file-tail ingest | Claimed/implemented and closed; reopened/closed again after review follow-ups per session note and interactions | closed | Main feature bead for PR #73. |
| `syslog-mcp-7j9hn` | PR #73 review follow-up | Opened and closed by the API/CLI/docs review-remediation agent; interaction notes API/CLI/MCP/docs/smoke findings and verification | closed | Tracked first review pass remediation. |

No new bead was created by this `save-to-md` pass. `bd list --all --sort updated --reverse --limit 100 --json` and `.beads/interactions.jsonl` were read before writing this file.

## Repository Maintenance

### Plans

- Checked `docs/plans` and `docs/superpowers/plans`.
- No plan files under `docs/plans/` were moved. The files `docs/plans/2026-03-29-unifi-cef-hostname-fix.md`, `docs/plans/2026-05-04-rmcp-stdio-support-follow-up.md`, and `docs/plans/2026-05-11-mnemo-feature-port.md` were not proven complete during this pass.
- The session plan `docs/superpowers/plans/2026-06-11-file-tail-ingest.md` is under `docs/superpowers/plans`, not `docs/plans`; it was left in place.

### Beads

- Read recent bead state and interactions.
- No bead mutations were needed during this save pass because the relevant beads were already closed and PR #73 was merged.
- `bd dolt push` had been run before the PR branch push during review remediation.

### Worktrees and branches

- Before save, `git worktree list --porcelain` showed only `/home/jmagar/workspace/cortex` on `main` at `8f092ac`.
- Local branch list showed only `main` tracking `origin/main`.
- Remote branch list showed `origin/main`; `git ls-remote --heads origin codex/file-tail-ingest` returned no output.
- Earlier in the session, the stale worktree `/home/jmagar/workspace/cortex/.worktrees/file-tail-ingest`, local branch `codex/file-tail-ingest`, and remote branch `origin/codex/file-tail-ingest` were removed after PR #73 merged.

### Stale docs

- File-tail docs had already been updated in PR #73: `README.md`, `docs/CONFIG.md`, `docs/CLI.md`, `docs/api.md`, and `docs/mcp/SCHEMA.md`.
- No additional stale-doc edit was made during this save pass. The stale-doc check was bounded to docs touched by the PR and current worktree evidence.

### Current repo state

- `git status --short --branch` before writing this artifact showed `## main...origin/main`.
- Main was fast-forwarded from `5632f78` to `8f092ac` after PR merge.

## Tools and Skills Used

- **Skills.** `superpowers:systematic-debugging`, `superpowers:writing-plans`, `vibin:work-it`, `superpowers:dispatching-parallel-agents`, `superpowers:receiving-code-review`, and `vibin:save-to-md`.
- **Shell commands.** Used `git`, `gh`, `cargo`, `bash`, `docker`, `bd`, `rg`, `find`, `sed`, `tail`, and `wc` for repo state, implementation, tests, PR state, cleanup, and documentation evidence.
- **File editing tools.** Used patch-based edits for Rust, shell, docs, and this session artifact.
- **GitHub CLI.** Created/checked/merged PR #73, inspected checks, and deleted the feature branch.
- **Subagents/agents.** Dispatched PR review toolkit agents named Reviewer, Hunter, and Analyzer twice; used review findings as the acceptance bar.
- **MCP/Cortex context.** Earlier investigation used Cortex/session search context to identify the special log sources that had previously been carried by syslog forwarders.
- **Issues encountered.** Docker smoke initially failed; pre-push branch deletion ran the full test hook and emitted a non-fatal `getcwd() failed` line after the worktree was removed.

## Commands Executed

| command | result |
|---|---|
| `git status --short --branch` | Main clean and synced after cleanup |
| `git worktree list --porcelain` | Only root main worktree remained |
| `git branch -vv` / `git branch -r -vv` | Only local `main`; remote `origin/main` |
| `gh pr view 73 --repo jmagar/cortex --json ...` | PR #73 state `MERGED`, merge commit `8f092ac`, all required checks successful |
| `git ls-remote --heads origin codex/file-tail-ingest` | No output; remote feature branch deleted |
| `git diff --name-status 5632f78..8f092ac` | Enumerated the PR #73 file set |
| `bash -n scripts/smoke-test.sh tests/test_live.sh tests/mcporter/test-tools.sh` | Passed during review remediation |
| `cargo test file_tail --lib --quiet` | Passed: 45 tests |
| `cargo test file_tails --lib --quiet` | Passed: 13 tests |
| `cargo test http_client::tests::file_tails_post_does_not_retry_503 --quiet` | Passed |
| `cargo clippy --all-targets -- -D warnings` | Passed |
| `cargo test --locked` | Passed |
| `CORTEX_TOKEN=codex-live-smoke-token bash tests/test_live.sh --mode docker --token codex-live-smoke-token` | Passed: 122 passed, 0 failed, 4 skipped |
| `git push origin codex/file-tail-ingest` | Pushed review-fix commit `37b412e`; pre-push tests passed |
| `gh pr merge 73 --repo jmagar/cortex --merge --delete-branch=false` | Merged PR #73 |
| `git worktree remove /home/jmagar/workspace/cortex/.worktrees/file-tail-ingest` | Removed merged feature worktree |
| `git branch -d codex/file-tail-ingest` | Deleted local merged branch |
| `git push origin --delete codex/file-tail-ingest` | Deleted remote branch; pre-push tests passed |
| `git pull --ff-only origin main` | Fast-forwarded main to `8f092ac` |

## Errors Encountered

- **Docker live smoke failed: file-tail marker not queryable.** First failure exposed an add/append timing hole: `file_tails add` could return before the task opened the file. Fixed by initializing the initial checkpoint synchronously during supervisor reconcile. The second failure exposed an FTS5 syntax issue: hyphenated markers must be phrase-quoted. Fixed all three smoke scripts.
- **Test expectation failed after checkpoint initialization.** `supervisor_waits_for_durable_ack_before_checkpointing` expected no checkpoint before ack; after the stronger add contract, the initial offset is valid. Updated the test to require that the offset does not advance beyond 0 until durable ack.
- **Pre-push hook was slow and noisy on branch deletion.** Deleting `origin/codex/file-tail-ingest` ran the full test hook and produced one non-fatal `sh: 0: getcwd() failed` line because the feature worktree had just been removed. The hook still passed and the remote branch deletion completed.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Extra app logs | Special app logs depended on ad hoc syslog forwarder behavior | Operators can add explicit file-tail sources through CLI, REST API, or MCP |
| Source attribution | File-tail source host could be implicit or wrong | `hostname` is required for `file_tails op=add` |
| Query-only mode | Local/stdio surfaces could risk competing tailers | Query-only runtimes can read registry but reject mutations |
| Checkpoint safety | A line could be read and checkpointed before durable DB acknowledgement | Checkpoint advances only after durable ingest ack |
| Add-then-append | Append immediately after add could be skipped by startup timing | Reconcile establishes initial checkpoint before returning |
| Rotation/truncation | Some copytruncate/regrow and rename-create cases were weaker | Prefix fingerprinting and EOF grace improve detection |
| Backpressure visibility | File-tail writer blocking was not surfaced in general status | `blocked_on_writer_since` and `status.file_tails.blocked_count` are visible |
| Smoke coverage | File-tail smoke did not prove Docker readability or hyphenated search correctly | Docker/live/mcporter smokes chmod/check mount readability and phrase-quote markers |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `bash -n scripts/smoke-test.sh tests/test_live.sh tests/mcporter/test-tools.sh` | Shell scripts parse | Passed | pass |
| `cargo test file_tail --lib --quiet` | File-tail unit tests pass | 45 passed | pass |
| `cargo test file_tails --lib --quiet` | Service/API file-tail filtered tests pass | 13 passed | pass |
| `cargo test http_client::tests::file_tails_post_does_not_retry_503 --quiet` | Non-retry regression passes | Passed | pass |
| `cargo clippy --all-targets -- -D warnings` | No clippy warnings | Passed | pass |
| `bash scripts/check-version-sync.sh` | Version-bearing files agree | `1.20.1` synced | pass |
| `cargo test --locked` | Full locked suite passes | Passed, including integration/doc tests | pass |
| `CORTEX_TOKEN=codex-live-smoke-token bash tests/test_live.sh --mode docker --token codex-live-smoke-token` | Docker smoke passes | 122 passed, 0 failed, 4 skipped | pass |
| GitHub PR checks | Required checks green | Formatting, Clippy, Tests, cargo-deny, secret scan, MCP integration, Docker build/push, CodeRabbit success | pass |
| `git status --short --branch` after cleanup | Clean `main` | `## main...origin/main` | pass |

## Risks and Rollback

- **Risk: host path exposure.** File-tail ingest reads mounted host files. Mitigation: read-only mount, default `/file-tail-root`, explicit allowlist, symlink denial, sensitive mount denial, admin-only mutation.
- **Risk: operational misconfiguration.** Operators must mount the correct host log root and pass source hostnames accurately. Mitigation: docs and required `hostname`.
- **Risk: high log volume.** Tail sources can add ingestion pressure. Mitigation: durable ack/backpressure visibility and existing storage guardrails.
- **Rollback:** revert merge commit `8f092ace3537129175072da367e275258802ff22` or revert the PR #73 commits, then redeploy the previous image/config. To disable the new behavior without code rollback, do not mount `CORTEX_FILE_TAIL_LOG_VOLUME` and remove/disable file-tail sources via the admin API/CLI.

## Decisions Not Taken

- **Do not revive broad syslog forwarders for special app logs.** That would reintroduce duplicate or ambiguous ingestion after the user confirmed syslog forwarding was already stopped on `nashost` and `backuphost`.
- **Do not selectively filter file-tail content inside Cortex.** The user explicitly wanted full parity and broad observability rather than selective ingestion.
- **Do not mount sensitive Cortex internals by default.** The default root remains `/file-tail-root`, not `/data` or arbitrary host paths.
- **Do not delete ambiguous historical plans.** Existing `docs/plans` files were left because this maintenance pass did not prove them complete.

## References

- PR #73: https://github.com/jmagar/cortex/pull/73
- Merge commit: `8f092ace3537129175072da367e275258802ff22`
- Implementation plan: `docs/superpowers/plans/2026-06-11-file-tail-ingest.md`
- Session implementation note: `docs/sessions/2026-06-11-file-tail-ingest.md`
- Key docs touched by PR #73: `README.md`, `docs/CONFIG.md`, `docs/CLI.md`, `docs/api.md`, `docs/mcp/SCHEMA.md`

## Open Questions

- `docs/plans/2026-03-29-unifi-cef-hostname-fix.md`, `docs/plans/2026-05-04-rmcp-stdio-support-follow-up.md`, and `docs/plans/2026-05-11-mnemo-feature-port.md` may be old enough to archive, but this pass did not verify their checklists.
- The Claude transcript file discovered by the skill points mostly at an earlier June 10 session; this note uses current Codex thread context plus live repo/PR evidence for PR #73 facts.

## Next Steps

- Deploy the new `main` image/config if production has not already picked up PR #73.
- Configure `CORTEX_FILE_TAIL_LOG_VOLUME` per host for the app log root that should appear as `/file-tail-root` inside Cortex.
- Add file-tail sources for SWAG, Authelia, AdGuard, fail2ban, or other explicit log files using `cortex file-tail add --id ... --path /file-tail-root/... --tag ... --host ...`.
- Use `cortex file-tail status --json` and `cortex action=status`/MCP `status` to watch tail health and blocked writer state.
- Review the three ambiguous historical `docs/plans/` files in a separate cleanup pass before moving them to `docs/plans/complete/`.
