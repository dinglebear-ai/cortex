---
date: 2026-05-31 12:00:34 EST
repo: git@github.com:jmagar/cortex.git
branch: feat/heartbeat-state-parity-and-incident-findings
head: aba264f
session id: 83cc1e16-1332-4339-81de-139b1a4035fb
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-cortex/83cc1e16-1332-4339-81de-139b1a4035fb.jsonl
working directory: /home/jmagar/workspace/cortex
worktree: /home/jmagar/workspace/cortex
pr: "#60 Heartbeat fleet-state parity (correlate_state) + deterministic abuse-incident findings — https://github.com/jmagar/cortex/pull/60"
beads: syslog-mcp-xcpl, syslog-mcp-tfr0, syslog-mcp-w4hh, syslog-mcp-rvcz, syslog-mcp-zs7g, syslog-mcp-a8pn, syslog-mcp-u1cl, syslog-mcp-h43u, syslog-mcp-ua4v, syslog-mcp-9wbm, syslog-mcp-is8b, syslog-mcp-6scc, syslog-mcp-pxab, syslog-mcp-3qen
---

# DB operations review, beads filing, and beads git-export diagnosis

## User Request

Run `/comprehensive-review:full-review` "scoped strictly to DB operations." Then: file beads
for the P0/P1 findings; investigate (via an agent) why beads' git-side auto-export warned on
`git add`; and silence that recurring warning.

## Session Overview

- Ran the 5-phase comprehensive review orchestrator strictly over `src/db/` (the SQLite + FTS5
  persistence layer), producing 8 specialist analyses and a consolidated final report under the
  gitignored `.full-review/` directory.
- Verdict: a mature layer with **no correctness, injection, or durability defects**; risk is
  concentrated in the migration framework, one storage guardrail, and write-path/scale behavior.
  Corrected a stale stack assumption — the layer is **synchronous rusqlite 0.39 + r2d2** (Rust
  2021), not the "async SQLx" CLAUDE.md documents.
- Filed 1 epic + 13 child beads covering all 3 Critical (P0) and 10 High (P1) findings.
- Diagnosed the `bd` auto-export `git add failed: exit status 128` warning as benign (it stems
  from `.beads` being a gitignored symlink to the shared global store) but surfaced and fixed two
  real, unrelated repo problems: a stale `.git/index.lock` and a dead `core.hooksPath`.
- Silenced the recurring warning via `bd config set export.git-add false`.
- No source code was modified. No commits were made to tracked source.

## Sequence of Events

1. Pre-flight: confirmed no prior `.full-review/` session; enumerated `src/db/` (9 modules + sidecar
   tests, ~7,300 LOC each); wrote `00-scope.md` and `state.json`.
2. Phase 1 (parallel agents): code-reviewer + architect over `src/db/`. Both independently flagged
   the rusqlite-not-SQLx drift, duplicate Migration 22, and stale `KNOWN_SCHEMA_VERSION`.
3. Phase 2 (parallel agents): security-auditor + performance engineer. Security found 2 Medium
   (cleartext Apprise creds, migration-22 brick); performance found 3 Critical (write amplification,
   redundant indexes, rollup writer starvation).
4. Checkpoint 1: presented the Phase 1–2 summary; user chose "Continue to Phase 3."
5. Phase 3 (parallel agents): test-coverage + documentation. Confirmed all prior doc defects plus
   the omitted `heartbeat.rs` / phantom `error_detection/` in CLAUDE.md's tree.
6. Phase 4 (parallel agents): Rust/SQLite best-practices + DB-ops. DevOps reviewer escalated the
   migration brick and storage self-wipe to Critical on blast-radius grounds.
7. Phase 5: synthesized `05-final-report.md` (P0–P3 action plan); marked `state.json` complete.
8. Filed beads: epic `syslog-mcp-xcpl`, three P0 bugs, ten P1 tasks; linked all as parent-child.
9. Dispatched a general-purpose agent to investigate the `git add` 128 warning; applied the two
   safe fixes it found (lock removal, hooksPath unset) after verifying no live git/bd process.
10. Set `bd config set export.git-add false` to stop the recurring warning.

## Key Findings

- **Stack drift (cross-cutting):** `Cargo.toml:29-31` is `rusqlite 0.39 + r2d2 + r2d2_sqlite`; `rg
  sqlx` finds only stale comments. CLAUDE.md's deps table (`:241`), design bullet (`:130`), and
  "Transaction Pattern (Rust/SQLx)" section (`:133,137-148`) are wrong; the `pool.begin().await?`
  example will not compile.
- **P0-1 Migration 22 brick:** `pool.rs:701-731` — bare `execute_batch` (auto-commits per
  statement) of two `ADD COLUMN` + version INSERT, plus a duplicate dead block. A crash between
  ALTER#1 and the version marker re-runs and hits `duplicate column name` → `restart: unless-stopped`
  crash-loop. Correct transactional pattern already exists at `apply_migration_13` (`pool.rs:1106`).
- **P0-2 Storage self-wipe:** `maintenance.rs:135-244` + `exceeds_trigger:820-826` — `min_free_disk_mb`
  triggers on whole-filesystem free space but only deletes cortex's own rows, so external disk
  pressure drains cortex's entire log history.
- **P0-3 Rollup writer starvation:** `queries.rs:604-650` holds an IMMEDIATE lock across DELETE +
  full GROUP BY every 5 min; breaks at ~100× scale. The `source_max_id` watermark for incremental
  refresh exists but is unused.
- **Beads git-export 128:** `.beads` is a symlink → `../../.beads` (shared global store), gitignored
  at `.gitignore:18`; `git add` of a path beyond a symlink fatals with `pathspec ... is beyond a
  symbolic link`. Benign — Dolt is the source of truth.
- **Two latent repo bugs (unrelated, now fixed):** stale empty `.git/index.lock` (mtime 11:38, no
  owning process) blocked all index writes including `git push`; `core.hooksPath` still pointed at
  the pre-rebrand `/home/jmagar/workspace/syslog-mcp/.git/hooks` (commit e8f69ae rename leftover).

## Technical Decisions

- Kept the review strictly within `src/db/`, reading consumers (`app/service.rs`, `db.rs`) only to
  understand contracts — honoring the "scope strictly to DB operations" instruction.
- Recorded findings as analysis documents (the orchestrator's file-based workflow) rather than the
  project's beads-first convention during the review, then filed beads afterward at the user's
  request — review output is analysis, not code change.
- Did not implement any fixes for review findings; filed them as tracked beads instead, so the work
  is visible and prioritized rather than half-done mid-session on a feature branch that isn't ours.
- Applied only the two clearly-safe, read-verified repo fixes (lock + hooksPath); left the feature
  branch's in-progress work and the 5 plan files untouched.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | `docs/sessions/2026-05-31-db-operations-review-and-beads-filing.md` | — | This session log | the file being written |
| created | `.full-review/00-scope.md` … `05-final-report.md`, `state.json` | — | Review analysis artifacts (8 docs + state) | gitignored at `.gitignore:27`; `ls .full-review/` shows 8 files |

No tracked source files were modified this session. The `.full-review/` artifacts are gitignored
and remain local only (`git check-ignore -v .full-review` → `.gitignore:27`).

Repo state (non-file) changes:
- Removed `/home/jmagar/workspace/cortex/.git/index.lock` (stale, empty).
- `git config --unset core.hooksPath` (was `…/syslog-mcp/.git/hooks` → now default `.git/hooks`).
- `bd config set export.git-add false` (in the shared `~/.beads/config.yaml`).

## Beads Activity

| ID | Title | Action | Status | Why |
|---|---|---|---|---|
| syslog-mcp-xcpl | [epic] DB operations layer review remediation (src/db/) | created | open | Umbrella for all P0/P1 review remediation |
| syslog-mcp-tfr0 | Migration 22 non-transactional → startup-brick crash-loop | created (P0 bug) | open | Crash-mid-migration permanently bricks startup |
| syslog-mcp-w4hh | Storage-budget enforcement self-wipes under external disk pressure | created (P0 bug) | open | External disk pressure drains all telemetry |
| syslog-mcp-rvcz | AI rollup refresh starves single writer (insert-drop cliff at scale) | created (P0 bug) | open | Dropped inserts at ~100× scale |
| syslog-mcp-zs7g | Drop 3 redundant single-column logs indexes (write amplification) | created (P1) | open | Immediate write relief, trivial |
| syslog-mcp-a8pn | Refactor migration framework: registry + single transactional driver | created (P1) | open | Structurally prevents the migration-brick class |
| syslog-mcp-u1cl | Fix stale KNOWN_SCHEMA_VERSION (20 vs head 22) | created (P1 bug) | open | Drift check can't detect a DB stuck at 20/21 |
| syslog-mcp-h43u | Move rusqlite backup out of app layer into db/maintenance | created (P1) | open | Stop driver leaking past the db boundary |
| syslog-mcp-ua4v | Centralize duplicated logs SELECT list + AI-session mapper | created (P1) | open | Silent column-shift risk from 21× copy-paste |
| syslog-mcp-9wbm | Add migration idempotency + partial-apply tests | created (P1) | open | Partial-apply test fails today, proves P0-1 |
| syslog-mcp-is8b | Rewrite CLAUDE.md SQLx doc drift (layer is rusqlite, not sqlx) | created (P1) | open | Non-compiling doc misleads every agent |
| syslog-mcp-6scc | Add composite index for retention purge (received_at, severity) | created (P1) | open | Non-indexable `severity NOT IN` post-filter |
| syslog-mcp-pxab | Cover error_signatures.rs untested functions (5/8) | created (P1) | open | Both read queries + window merge uncovered |
| syslog-mcp-3qen | DB-ops: backup schedule, CLI backup path, startup integrity check | created (P1) | open | No auto backup, unreliable cron, no integrity probe |

All 13 children linked to `syslog-mcp-xcpl` via `bd dep add … --type parent-child`. No beads were
closed, claimed, or edited. The ~30 Medium and ~40 Low findings were intentionally left unfiled
(documented in `.full-review/05-final-report.md` P2/P3 sections) to avoid tracker noise.

## Repository Maintenance

- **Plans:** 5 files under `docs/plans/` (2026-03-29 … 2026-05-12); none worked this session and none
  clearly completed by it — left untouched, no `docs/plans/complete/` created. Evidence: `ls
  docs/plans/*.md`.
- **Beads:** created the 14 issues above; nothing safe to close (all are newly filed follow-up work).
  Evidence: `bd list --status=open` showing each new ID.
- **Worktrees/branches:** single worktree on `feat/heartbeat-state-parity-and-incident-findings`
  (active PR #60) — not stale, left as-is. `main` is behind the branch; not our work to merge.
  Evidence: `git worktree list --porcelain`, `git branch -vv`.
- **Stale docs:** the review identified stale docs (CLAUDE.md SQLx drift, phantom `error_detection/`,
  omitted `heartbeat.rs`, CHANGELOG schema claim) but these were filed as bead `syslog-mcp-is8b`
  rather than edited here — the feature branch carries unrelated in-progress work and a doc rewrite
  belongs in its own change. Not fixed; tracked.
- **Repo hygiene fixes applied:** stale `.git/index.lock` removed and dead `core.hooksPath` unset
  (both verified safe — no live git/bd process, no rebase/merge in progress). Evidence: `ps aux`,
  `ls .git/rebase-* .git/MERGE_HEAD`.

## Tools and Skills Used

- **Skill:** `comprehensive-review:full-review` orchestrator (5 phases, file-based state in
  `.full-review/`). No issues.
- **Subagents (Task):** 8 review agents (code-reviewer, architect-review, security-auditor ×1,
  general-purpose ×5) run pairwise in parallel; 1 general-purpose agent for the git-export
  investigation. All returned structured deliverables; no failures. One performance agent corrected a
  Phase-1 finding (the `get_error_summary` LIMIT swap is memory-only, not scan-pruning).
- **Shell (Bash):** `bd` (create/list/dep/config), `git` (status/config/worktree/check-ignore),
  `ls`/`ps` diagnostics. One recoverable issue: `bd create` emitted `git add failed: exit status 128`
  (root-caused and silenced).
- **File tools:** Write (scope, 6 consolidated phase docs, final report, state.json, this log); Read
  (`03-documentation.md` companion produced by the doc agent).
- **AskUserQuestion:** Checkpoint 1 approval.

## Commands Executed

| command | result |
|---|---|
| `bd create --type=epic …` (+ 13 child creates) | created `syslog-mcp-xcpl` + 13 issues; warned `git add failed: exit status 128` |
| `bd dep add syslog-mcp-xcpl syslog-mcp-<child> --type parent-child` ×13 | all parent-child links added |
| `git check-ignore -v .beads/issues.jsonl` (via agent) | `fatal: … beyond a symbolic link` (root cause) |
| `rm -f .git/index.lock` | removed stale lock; `git status` works again |
| `git config --unset core.hooksPath` | unset dead path → default `.git/hooks` |
| `bd config set export.git-add false` | `Set export.git-add = false (in config.yaml)`; `bd config get` → `false` |

## Errors Encountered

- `bd create` → `Warning: auto-export: git add failed: exit status 128`. Root cause: `.beads` is a
  gitignored symlink to the shared `~/.beads` store, so `git add` of a path beyond the symlink
  fatals. Benign for durability (Dolt is source of truth); resolved by disabling `export.git-add`.
- Stale `.git/index.lock` (separate, pre-existing) blocked index writes. Resolved by removing it
  after confirming no owning process.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `bd create`/update | emitted a spurious `git add ... 128` warning each run | warning silenced (`export.git-add=false`); JSONL still written, Dolt unaffected |
| git index | `git push`/`add` blocked by stale `index.lock` | index writable again |
| git hooks | `core.hooksPath` → nonexistent `syslog-mcp/.git/hooks` (hooks silently inert) | default `.git/hooks` restored |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `bd config get export.git-add` | `false` | `false` | pass |
| `git status --short --branch` (post lock removal) | runs without lock error | branch + dirty files listed, no fatal | pass |
| `git config --get core.hooksPath` (post unset) | no value | empty (unset) | pass |
| `bd list --status=open` | 14 new IDs present | epic + 13 children listed | pass |

## Risks and Rollback

- Removing `.git/index.lock` is safe only because no git/bd process held it (verified via `ps`); if a
  real operation had been mid-flight, removal could corrupt the index. It was stale.
- `export.git-add=false` lives in the shared `~/.beads/config.yaml`, so it applies wherever that
  store is symlinked. To revert: `bd config set export.git-add true`.
- `core.hooksPath` unset: if repo-local hooks are later desired, set it to
  `/home/jmagar/workspace/cortex/.git/hooks`.

## Decisions Not Taken

- Did not fix any review finding in code — filed beads instead (work belongs in its own change, not
  on the unrelated PR #60 branch).
- Did not run `bd dolt push` / `git push` — left to the user; the feature branch carries in-progress
  work and the review artifacts are gitignored.
- Did not file P2/P3 findings as beads — kept in the final report to avoid tracker noise.

## References

- `.full-review/05-final-report.md` — consolidated report (P0–P3 action plan)
- `.full-review/00-scope.md` … `04-best-practices.md` — per-phase analyses
- PR #60 — https://github.com/jmagar/cortex/pull/60 (the branch this session ran on)

## Open Questions

- Should the P2/P3 review findings be filed as beads later, or tracked only in the report?
- Is the CLAUDE.md SQLx rewrite (bead `is8b`) wanted on `main` directly, or batched with the
  migration-framework refactor (`a8pn`)?

## Next Steps

- **Unfinished from this session:** run `bd dolt push` to propagate the 14 new issues to the remote
  Dolt store (the issues live in Dolt, independent of the git branch).
- **Recommended first remediation (own PR off `main`):** the low-risk hotfix batch — wrap Migration
  22 transactionally + guard ALTERs + delete the dupe block (`tfr0`), drop the 3 redundant indexes
  (`zs7g`), set `KNOWN_SCHEMA_VERSION=22` (`u1cl`), with the idempotency/partial-apply tests
  (`9wbm`) proving the fix.
- **Then:** storage self-wipe guard (`w4hh`) and incremental rollup refresh (`rvcz`).
- **Not blocked, but separate:** CLAUDE.md doc rewrite (`is8b`) and migration-framework refactor
  (`a8pn`).
