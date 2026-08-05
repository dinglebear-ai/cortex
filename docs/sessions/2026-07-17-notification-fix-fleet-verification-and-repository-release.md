# Notification Fix, Fleet Verification, and Repository Release

## Session Metadata

- Date: 2026-07-17 17:01 EDT
- Repository: `git@github.com:jmagar/cortex.git`
- Working directory: `/home/jmagar/workspace/cortex`
- Branch at capture: `main`
- HEAD at capture: `3ea94aa72cbb96300ee17cacc2599aa7e1439fbe`
- Worktree: `/home/jmagar/workspace/cortex` (the only registered worktree)
- Active pull request: [#141 — chore(main): release 3.11.1](https://github.com/jmagar/cortex/pull/141)
- Tracking bead: `syslog-mcp-pv4j6`
- Prior full fleet-session artifact: `docs/sessions/2026-07-17-fleet-audit-widget-mcp-apps-and-silence-alerts.md`

## Objective

Resolve the repeated Cortex fleet-silence notifications, distinguish the nashost server from the nashost and Tower heartbeat-agent containers, verify what each configured host is actually delivering, then leave the repository fully merged, synchronized, branch-clean, built, and deployed.

## Summary

The repeated Gotify warnings were not proof that nashost, Tower, and devhost were continuously disconnected. The silence evaluator correctly generated stable outage keys containing the stream's unchanged last-seen timestamp, but the dispatcher only searched a rolling 15-minute firing window. Once an outage remained open longer than that window, the same outage was sent again. Heartbeat- and stream-silence rules now check exact dedup keys across the full firing history; ordinary notification rules retain the rolling window.

The Unraid agent containers were also corrected independently from the full server. The heartbeat-agent image does not serve HTTP on port 3100, so its inherited server-only Docker healthcheck was guaranteed to report unhealthy. That probe was removed only from the agent containers. The full Cortex server on nashost still has its `/health` probe enabled and healthy. Both Unraid agents run as root so their local Docker sockets are accessible.

Live fleet inspection showed that several alerts were stale/repeating rather than fresh transport failures, but it did not support claiming that every known device sends every possible source. nashost and Tower recovered their configured heartbeat, Docker, and TCP paths; devhost, edgehost, WINHOST WSL, laptophost WSL, and agent-os have host-specific gaps or intentionally narrower configurations; BACKUPHOST remains genuinely down because its Unraid array/Docker stack is unavailable.

## Implemented Changes

Commit `5e982465` (`fix(notifications): suppress repeat silence outages`) changed:

- `src/db/notifications.rs`: added an all-history exact-dedup lookup.
- `src/notifications/dispatcher.rs`: heartbeat-silence and stream-silence use lifetime exact-key suppression; other rules keep the configured rolling window.
- `src/notifications/dispatcher_tests.rs`: added regression coverage proving an old firing for the same outage remains suppressed while a changed last-seen timestamp represents a new outage.

The fix was developed test-first. The new regression initially reached the no-Apprise path, demonstrating that the stale firing was no longer found by the rolling-window query. After the dispatcher change, the same candidate was dropped as `dedup_suppressed`.

## Runtime Corrections and Verification

- Recreated nashost's heartbeat-agent container as root and without the server-only HTTP healthcheck.
- Recreated Tower's heartbeat-agent container as root and without the server-only HTTP healthcheck; corrected its persisted environment and token.
- Kept nashost's full Cortex server healthcheck enabled. The server remained healthy.
- Deployed the notification fix to the nashost server and observed repeated evaluation cycles.
- At 19:53, 19:58, 20:03, and 20:08 UTC, unchanged BACKUPHOST heartbeat-silence and devhost stream-silence candidates were dropped as `dedup_suppressed`; their firing counts and timestamps did not advance.

## Fleet Delivery Findings

| Host | Observed delivery | Remaining limitation |
|---|---|---|
| nashost | current heartbeat, Docker stream, TCP syslog, Plex file tail | none in the checked configured paths |
| Tower | current heartbeat; Docker and TCP streams recovered | recent history should continue to be watched after recreation |
| devhost | heartbeat, Docker, TCP, command forwarding | configured shell-history path had no observed rows; command rows can be mislabeled `localhost` |
| edgehost | heartbeat, Docker, TCP | transcript/shell-history evidence conflicted between prior audit and the current database snapshot |
| WINHOST WSL | heartbeat and TCP | no current transcript/shell-history rows observed |
| laptophost WSL | heartbeat and TCP | Docker collection is intentionally disabled |
| agent-os | heartbeat | Windows host is partial because several collectors still assume Linux `/proc` |
| BACKUPHOST | no current heartbeat | Unraid license/array failure prevents Docker and the agent from running |

Android devices and the offline Steam Deck/tablet are not configured as Cortex heartbeat agents, so their absence is not an ingest regression.

## Errors and Corrections During the Work

- Initial wording blurred nashost the machine, nashost the full Cortex server, and the separate nashost heartbeat-agent container. They are now treated as distinct runtime units.
- Tower and nashost were initially discussed too loosely; Tower is a separate Unraid host.
- The first Tower token recreation attempt suffered shell expansion at the wrong layer. The persisted environment was corrected and the container recreated.
- A healthy heartbeat-agent was labeled unhealthy because it inherited a server HTTP probe for a port it never opens. Only that invalid agent probe was disabled.
- Silence alerts used a stable outage key but a time-bounded lookup, producing repeat notifications every time the generic dedup window expired.

## Beads Activity

| Bead | Action | Status |
|---|---|---|
| `syslog-mcp-jk6jv` — Make silence notifications fire once per outage | implemented, verified live, closed | closed |
| `syslog-mcp-pv4j6` — Finalize repository cleanup, release merge, and production deployment | created and claimed for the continuation after this artifact lands | in progress |

Existing open defects remain separately tracked: Windows-native collectors (`syslog-mcp-cj3ug`), misleading container Docker probe state (`syslog-mcp-7v8ck`), oversized TCP line churn (`syslog-mcp-z01or`), `localhost` command hostname attribution (`syslog-mcp-e4l4d`), and transcript-forwarder warning feedback (`syslog-mcp-i6ri8`).

## Repository Maintenance at Capture

- `git status` was clean and `main` exactly matched `origin/main` at `3ea94aa7`.
- One registered worktree existed: `/home/jmagar/workspace/cortex` on `main`.
- One local branch existed: `main`.
- Remote branches were `main`, the intentional long-lived `marketplace-no-mcp`, and the active release-please branch for PR #141.
- PR #141 was mergeable, clean, and all CI/security/review checks had completed successfully.
- `marketplace-no-mcp` has an explicit synchronization workflow in `.github/workflows/sync-marketplace-no-mcp.yml`; it is not stale.
- Three old unchecked plans remain under `docs/plans/`. Their checklists are incomplete and this session did not establish enough evidence to archive them, so they were left untouched.
- The repository instructions and runtime configuration documentation already describe once-per-outage silence behavior and the server/agent distinction; no contradicted documentation required an edit in this pass.

## Decisions

- Preserve the full-history notification firing table and query it by exact dedup key for outage rules instead of inventing another mutable outage-state table.
- Keep the generic rolling dedup window for non-outage rules.
- Keep nashost's server healthcheck enabled; remove the port-3100 probe only from heartbeat-agent containers.
- Do not claim universal fleet completeness when live rows do not prove every configured path.
- Merge the green release-please PR with a merge commit, consistent with prior release PRs, then remove its branch.
- Preserve and synchronize `marketplace-no-mcp`; delete every other stale branch.

## Verification Evidence

| Check | Result |
|---|---|
| Notification regression test before fix | failed through `no_apprise_urls`, reproducing the expired-window bug |
| Notification regression test after fix | same outage dropped as `dedup_suppressed`; changed outage key allowed |
| Live evaluator over four cycles | firing count and timestamps remained stable for unchanged outages |
| nashost server health | healthy with the server HTTP probe still enabled |
| nashost/Tower agent state | recreated as root; server-only health probe absent only on agents |
| Repository worktrees | exactly one, on `main` |
| PR #141 checks | all completed successfully; merge state clean |

## Risks and Rollback

- Lifetime dedup assumes outage keys always include the stalled last-seen timestamp. The evaluator tests cover this contract; changing key construction later must preserve that discriminator.
- BACKUPHOST will remain legitimately silent until its Unraid licensing/array problem is fixed.
- Some fleet paths are not proven end-to-end, particularly shell/transcript forwarding and Windows-native system probes.
- The notification fix can be rolled back by reverting `5e982465`; doing so restores the old repeated-alert behavior.
- Container deployment rollback is the previous versioned image and the prior host binary.

## Immediate Continuation

1. Commit and push this artifact as a path-limited documentation commit.
2. Merge release PR #141 and synchronize `main`.
3. Let or run the canonical `marketplace-no-mcp` synchronization workflow and verify drift checks.
4. Delete the release branch and any other stale local/remote branches, retaining only `main` and `marketplace-no-mcp` remotely and only `main` locally.
5. Run repository quality gates, build the latest release binary, install it on the host PATH, and deploy the same version to the nashost Cortex container.
6. Verify Git synchronization, branch inventory, binary versions, container image/version, and `/health`, then close `syslog-mcp-pv4j6` and push Beads state.
