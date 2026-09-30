```yaml
date: 2026-07-02 07:05:51 EST
repo: git@github.com:jmagar/cortex.git
branch: claude/infallible-galileo-d5d1d1
head: e7d3572
working directory: /home/jmagar/workspace/cortex/.claude/worktrees/infallible-galileo-d5d1d1
worktree: /home/jmagar/workspace/cortex/.claude/worktrees/infallible-galileo-d5d1d1
pr: #111 fix: harden fake-gemini test double against stdin broken-pipe race (https://github.com/jmagar/cortex/pull/111) — MERGED
beads: syslog-mcp-sxx56 (created, claimed, closed)
```

## User Request

Investigate and fix a flaky unit test — `src/app/service_tests.rs::ai_assess_writes_llm_invocation_audit_row_via_runner` — which intermittently fails under a fully-loaded parallel `cargo test --lib` run with `failed to write Gemini stdin: Broken pipe (os error 32)`, but passes reliably in isolation. The user provided detailed investigation starting points (inspect the fake-Gemini subprocess/test double, check for a startup/stdin race under scheduler pressure, consider whether the write error should be tolerated when the child already exited successfully). Once the fix was verified, the user asked to merge it into `main`.

## Session Overview

Traced the flake to a genuine race between the test's fake-`gemini` script (which never read stdin before exiting) and `run_gemini_assessment`'s concurrent stdin-writer task. Fixed by draining stdin in the test double before it responds, verified the fix reproduces-then-resolves the exact reported error under real system load, opened PR #111, watched all CI checks pass, squash-merged into `main` (tag `v3.2.3`), and closed out the tracking bead.

## Sequence of Events

1. Read the flaky test (`src/app/service_tests.rs:1614`) and the fake-`gemini` script it writes to disk, confirming it never reads stdin before printing output and exiting 0.
2. Read `src/assessment.rs::run_gemini_assessment` to understand the stdin-write/stdout-read/exit-status flow and confirmed stdin-write errors are surfaced as fatal (`assessment.rs:242`) even after a successful exit status, unlike the deliberate broken-pipe-priority test (`assessment_tests.rs:248`) which relies on exit-status errors taking priority over stdin errors.
3. Searched the codebase for all fake-`gemini` test doubles (`src/assessment_tests.rs` at two other locations) and confirmed only the flaky test's script has the pattern (exit 0 without ever reading stdin) that races against the concurrent write.
4. Edited the fake script to `cat >/dev/null` before responding, forcing it to block until the parent's write+shutdown completes, eliminating the race deterministically without touching production code.
5. Ran the target test in isolation; reproduced the exact original failure on the **unpatched** code (`git stash`) under the session's then-current system load, then confirmed the fix passes 6/6 consecutive runs.
6. Ran the full `assessment::tests::` suite (12 tests, including the intentionally-racy `gemini_assessment_reports_child_stderr_before_stdin_pipe_error`) to confirm the fix doesn't mask the exit-status-priority behavior that test depends on.
7. Ran `cargo fmt --check` on the changed file — clean.
8. On "merge it into main": created and claimed bead `syslog-mcp-sxx56`, bumped version 3.2.2 → 3.2.3 via `cargo xtask bump-version patch`, filled in the CHANGELOG entry, ran `cargo xtask check-release-versions` and `cargo clippy --all-targets -- -D warnings` (both clean).
9. Committed the fix + version bump, pushed (pre-commit/pre-push hooks — rustfmt, version-sync, clippy, module-size — all passed), opened PR #111.
10. Watched all PR checks via `gh pr checks 111 --watch` until green (Clippy, Coverage, Tests, MCP Integration Tests, Dependency Check, Formatting, Version Sync, Secret Scan, build-and-push, Pre-publish CI gate, CodeRabbit, GitGuardian).
11. Squash-merged PR #111 (`gh pr merge 111 --squash`), deleted the remote branch via the GitHub API (local `--delete-branch` cleanup failed only because `main` is checked out in a sibling worktree — the merge itself had already succeeded), fetched `origin` to confirm `main` now includes the fix at `1ac3b73` tagged `v3.2.3`.
12. Closed bead `syslog-mcp-sxx56`.

## Key Findings

- `src/app/service_tests.rs:1666` (pre-fix): the fake-`gemini` script was `#!/usr/bin/env bash\necho '...'\necho '...'\n` — it never reads stdin.
- `src/assessment.rs:150-153`: `run_gemini_assessment` spawns a concurrent tokio task (`stdin_task`) that writes the prompt to the child's stdin and shuts it down, running in parallel with reading the child's stdout stream.
- `src/assessment.rs:242`: `stdin_result?` is checked *after* `status.success()` — so a stdin write failure is fatal specifically when the child otherwise exited 0, which is exactly the flaky test's scenario (the other fake-gemini test at `assessment_tests.rs:248` exits non-zero, so the exit-status error masks the stdin error there by design).
- Under scheduler pressure, the child (a two-`echo` bash script) can run to completion and close its stdin read end before the writer task is scheduled, producing `Broken pipe (os error 32)` even though stdout already streamed valid, complete output.
- Reproduced the failure directly: running the single test in isolation on the **unpatched** code failed with the exact reported error under the session's ambient system load, proving this is a real scheduling race and not exclusively a full-suite-only artifact.

## Technical Decisions

- **Fixed the test double, not production code.** Considered (per the user's option 4) making `run_gemini_assessment` tolerate a `BrokenPipe` stdin-write error when the child already exited successfully with valid output, but rejected it: real `gemini` CLI usage presumably reads its full prompt before producing output, so masking stdin-write failures in production would risk hiding genuine problems in real invocations. The test double not reading stdin is what created an artificial race that a real Gemini CLI wouldn't exhibit.
- **`cat >/dev/null` over a delay/retry.** Draining stdin to EOF is a deterministic synchronization point (blocks until the parent's `shutdown()` sends EOF) rather than a timing-based band-aid (e.g., `sleep`), so it removes the race entirely rather than narrowing its window.
- **Did not touch the other two fake-gemini scripts** (`assessment_tests.rs`'s `sleep 5` timeout test and `exit 42` broken-pipe-priority test) — both intentionally avoid reading stdin for their own test purposes, and applying the same fix there would break the second test's assertion that exit-status errors take priority over stdin errors.
- **Landed the session log via a fresh branch off `origin/main`, not the merged topic branch.** The `save-to-md` workflow's default rule (commit on the current topic branch and let it ride the existing PR) assumed an open PR; PR #111 was already merged and its remote branch deleted before this doc was written, so recommitting to the dead topic branch would have stranded the log on an orphaned, unmerged ref instead of landing it on `main`.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `src/app/service_tests.rs` | — | Harden fake-gemini test double: drain stdin before responding | PR #111 diff; `git show --stat 1ac3b73` |
| modified | `CHANGELOG.md` | — | Add `[3.2.3]` entry describing the fix | PR #111 diff |
| modified | `Cargo.toml` | — | Version bump 3.2.2 → 3.2.3 | `cargo xtask bump-version patch` output |
| modified | `Cargo.lock` | — | `cortex` package entry version bump | `cargo xtask bump-version patch` |
| modified | `docker-compose.prod.yml` | — | `${CORTEX_VERSION:-3.2.3}` default tag | `cargo xtask bump-version patch` |
| modified | `mcpb/manifest.json` | — | MCP Bundle version bump | `cargo xtask bump-version patch` |
| modified | `server.json` | — | MCP Registry version + image tag bump | `cargo xtask bump-version patch` |
| created | `docs/sessions/2026-07-02-fix-flaky-llm-invocation-test.md` | — | This session log | this commit |

## Beads Activity

| ID | Title | Actions | Final Status | Why it mattered |
|---|---|---|---|---|
| `syslog-mcp-sxx56` | Fix flaky `ai_assess_writes_llm_invocation_audit_row_via_runner` test | Created, claimed, closed | Closed | Tracks the flaky-test root cause and fix per repo's mandatory beads workflow; referenced in the merge commit ("Closes syslog-mcp-sxx56") |

## Repository Maintenance

- **Plans**: Reviewed `docs/plans/` (three open: `2026-03-29-unifi-cef-hostname-fix.md`, `2026-05-04-rmcp-stdio-support-follow-up.md`, `2026-05-11-mnemo-feature-port.md`). None relate to this session's work or show evidence of completion within this session; left in place.
- **Beads**: `syslog-mcp-sxx56` created, claimed, and closed for this session's work (see above). No other in-flight beads were touched.
- **Worktrees and branches**: Ran `git worktree list --porcelain` and `git branch -vv` (via injected context). Observed:
  - This session's branch `claude/infallible-galileo-d5d1d1` is fully merged into `main` (PR #111) and its remote ref is already deleted; the local branch/worktree is left in place since it's the currently active worktree for this session.
  - Sibling worktrees `happy-dirac-58d616` (branch `claude/happy-dirac-58d616`, open PR context per branch name), `happy-kepler-2d8fa5` (branch `feature/pr2-skill-event-extraction`, in-progress GH #94 PR2), `jolly-jemison-0735af`, and `serene-taussig-76f587` belong to other active or recent sessions with unclear/independent ownership — not touched, per the "unclear ownership" exclusion.
  - The `main` worktree itself (`/home/jmagar/workspace/cortex`) is one commit behind `origin/main` (pre-dates this session's merge) — not this skill's responsibility to fast-forward; noted for the user.
- **Stale docs**: No documentation was found that references this test, the fake-gemini test-double pattern, or contradicts the fix. No doc updates required.
- **Transparency**: All maintenance actions above are based on direct command output (`bd show`, `git worktree list`, `git branch -vv`, `ls docs/plans/`) gathered during this pass; no destructive actions were taken.

## Tools and Skills Used

- **Shell commands (Bash)**: `grep`/`sed` for code reading, `cargo test`/`cargo fmt`/`cargo clippy`/`cargo xtask` for verification and release mechanics, `git` for branch/commit/push/stash operations, `gh` for PR create/checks/merge, `bd` for bead tracking. No failures beyond one expected/handled case (see Errors Encountered).
- **File tools (Read/Edit/Write)**: Used to inspect and edit `src/app/service_tests.rs`, `src/assessment.rs`, `src/assessment_tests.rs`, `CHANGELOG.md`, and to write this session log.
- **Background task execution (Bash `run_in_background` + `TaskOutput`)**: Used for long-running `cargo test`/`cargo clippy` compiles and `gh pr checks --watch`, to avoid blocking on multi-minute CI/compile cycles.
- **MCP servers/skills**: None invoked for the fix-and-merge work. The `save-to-md` skill (this invocation) is producing the current document.
- **Subagents/agents**: None used — all investigation and edits were done directly in the main session.
- **Browser tools**: Not used.

## Commands Executed

| Command | Result |
|---|---|
| `grep -n "ai_assess_writes_llm_invocation_audit_row_via_runner" -A 60 src/app/service_tests.rs` | Located the test and its fake-gemini script construction |
| `grep -n "failed to write Gemini stdin\|fn run_gemini_assessment\|stdin" src/assessment.rs` | Found the stdin-write/status-check ordering in `run_gemini_assessment` |
| `grep -rn "fake-gemini\|CORTEX_HEADLESS_GEMINI_CMD" --include="*.rs" src/` | Confirmed only one fake-gemini script needed the fix |
| `cargo test --lib app::services::tests::ai_assess_writes_llm_invocation_audit_row_via_runner -- --exact` (post-fix) | `ok`, 6/6 runs across repeated invocations |
| `git stash && cargo test --lib app::services::tests::ai_assess_writes_llm_invocation_audit_row_via_runner -- --exact; git stash pop` | Reproduced the exact reported `Broken pipe (os error 32)` failure on unpatched code |
| `cargo test --lib assessment::tests::` | `ok`, 12/12 passed, including the intentionally-racy exit-status-priority test |
| `cargo fmt -- --check` | Clean |
| `cargo xtask bump-version patch` | `Bumped cortex 3.2.2 → 3.2.3` |
| `cargo xtask check-release-versions` | `OK: 8 version-bearing file(s) in sync at 3.2.3.` |
| `cargo clippy --all-targets -- -D warnings` | Clean |
| `bd create --title="Fix flaky ai_assess_writes_llm_invocation_audit_row_via_runner test" ...` | Created `syslog-mcp-sxx56` |
| `git commit -m "fix: harden fake-gemini test double against stdin broken-pipe race" ...` | Committed `e7d3572`; pre-commit hooks (rustfmt, version-sync, etc.) passed |
| `git push -u origin claude/infallible-galileo-d5d1d1` | Pushed; pre-push hooks (version-sync, module-size, clippy) passed |
| `gh pr create --title "fix: harden fake-gemini test double against stdin broken-pipe race" ...` | Opened PR #111 |
| `gh pr checks 111 --watch` | All checks passed (Clippy, Coverage, Tests, MCP Integration Tests, Dependency Check, Formatting, Version Sync, Secret Scan, build-and-push, Pre-publish CI gate) |
| `gh pr merge 111 --squash --delete-branch` | Merge succeeded; `--delete-branch` step errored (see Errors Encountered) |
| `gh pr merge 111 --squash` (retry) | `Pull request jmagar/cortex#111 was already merged` — confirmed merge landed |
| `gh api repos/jmagar/cortex/git/refs/heads/claude/infallible-galileo-d5d1d1 -X DELETE` | Deleted the stale remote branch ref |
| `git fetch origin` | Confirmed `origin/main` at `1ac3b73`, new tag `v3.2.3` |
| `bd close syslog-mcp-sxx56` | Closed the tracking bead |

## Errors Encountered

- `gh pr merge 111 --squash --delete-branch` exited 1 with `failed to run git: fatal: 'main' is already used by worktree at '/home/jmagar/workspace/cortex'`. Root cause: `gh`'s post-merge local cleanup step tries to switch the local repo to the base branch (`main`) to delete the now-merged local branch, but `main` is checked out in a sibling worktree, which git refuses to double-checkout. The merge itself (server-side) had already succeeded before this local cleanup step ran. Resolved by verifying merge state (`gh pr view 111 --json state,mergedAt` → `MERGED`) and deleting the remote branch ref directly via the GitHub API instead of relying on `gh`'s local branch cleanup.

## Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| `ai_assess_writes_llm_invocation_audit_row_via_runner` test reliability | Intermittently failed under load with `Broken pipe (os error 32)`; also reproducible in isolation under sufficient ambient system load | Passes reliably (6/6 verified runs) regardless of system load; the fake-gemini test double now blocks on stdin EOF before responding |
| `cortex` package version | 3.2.2 | 3.2.3 (tagged `v3.2.3`, released via merged PR #111) |
| Production `run_gemini_assessment` behavior | Unchanged | Unchanged — this was a test-only fix |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo test --lib app::services::tests::ai_assess_writes_llm_invocation_audit_row_via_runner -- --exact` (unpatched, via `git stash`) | Fails intermittently per bug report | Failed with `failed to write Gemini stdin: Broken pipe (os error 32)` | pass (confirms root cause) |
| `cargo test --lib app::services::tests::ai_assess_writes_llm_invocation_audit_row_via_runner -- --exact` (patched, x6) | Passes reliably | `ok` on all 6 runs | pass |
| `cargo test --lib assessment::tests::` | All 12 tests pass, including the intentionally-racy exit-status-priority test | `12 passed; 0 failed` | pass |
| `cargo fmt -- --check` | No formatting diffs | Clean | pass |
| `cargo clippy --all-targets -- -D warnings` | No warnings | Clean | pass |
| `cargo xtask check-release-versions` | All version-bearing files in sync | `OK: 8 version-bearing file(s) in sync at 3.2.3.` | pass |
| `gh pr checks 111 --watch` | All CI checks green | Clippy, Coverage, Tests, MCP Integration Tests, Dependency Check, Formatting, Version Sync, Secret Scan, build-and-push, Pre-publish CI gate, CodeRabbit, GitGuardian all `pass` | pass |
| `gh pr view 111 --json state,mergedAt` | `MERGED` | `{"state":"MERGED","mergedAt":"2026-07-02T07:22:23Z"}` | pass |

## Risks and Rollback

Low risk: the change is confined to a test double (`src/app/service_tests.rs`) and does not alter any production code path. If a regression were suspected, revert commit `1ac3b73` (the squash-merge of PR #111) on `main` and re-open the investigation — this would restore the pre-existing flaky-test behavior, not introduce a new one.

## Decisions Not Taken

- **Tolerating `BrokenPipe` in `run_gemini_assessment` when the child already exited successfully** (explicitly suggested as option 4 in the user's investigation prompt): rejected because it would change production error-handling semantics based on a test-only artifact, and could mask genuine partial-write failures against a real `gemini` CLI that does expect to consume the full prompt.
- **Adding a delay/sleep to the test double**: rejected in favor of `cat >/dev/null`, which is a deterministic synchronization point rather than a timing-dependent mitigation that would only narrow, not eliminate, the race window.

## References

- PR: https://github.com/jmagar/cortex/pull/111
- Merge commit: `1ac3b73cecb6c848facebd7c1d6640e7b12cfdd3`
- Release tag: `v3.2.3`
- Bead: `syslog-mcp-sxx56`

## Next Steps

- No unfinished work from this session — the fix is merged, tagged, and the tracking bead is closed.
- Out-of-scope observations for future sessions (not actioned here): the `main` worktree at `/home/jmagar/workspace/cortex` is one commit behind `origin/main` and could be fast-forwarded by its owner; sibling worktrees `jolly-jemison-0735af` and `serene-taussig-76f587` both sit at the pre-merge `origin/main` tip with no apparent divergent work and may be candidates for pruning by whoever owns them.
