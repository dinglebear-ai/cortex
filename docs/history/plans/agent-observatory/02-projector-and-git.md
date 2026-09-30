# Phase 2: run projection and live Git topology

Prerequisite: Phase 1 green.
Phase gates: G2 Projection and G3 Git.

## AO-019 Implement versioned identity helpers

- **Deliverable:** canonical tool normalization and run/repository/worktree/actor/event keys.
- **Files:** `src/agent_observatory/identity.rs`, sidecar tests.
- **RED:** contract vectors including Unicode byte lengths, delimiter characters, whitespace, and empty parts fail.
- **GREEN:** implement length-prefixed identities and deterministic event-key validation.
- **Proof:** property-style fixture set produces exact keys from contract examples and rejects >1024-byte keys.
- **Gate:** no random IDs or new dependency.
- **References:** contract §2, contract Rust/TS helpers.

## AO-020 Implement lifecycle reducer

- **Deliverable:** pure status reducer with explicit-evidence precedence and configurable windows.
- **Files:** `src/agent_observatory/lifecycle.rs`, tests.
- **RED:** table-driven tests cover every status/reason and boundary second.
- **GREEN:** implement pure function taking fixed `now` and evidence inputs.
- **Proof:** all table cases pass with no sleeps; mutation of one precedence branch breaks a test.
- **Gate:** missing signal yields `not_observed`, not terminal state.
- **References:** contract §4.

## AO-021 Implement attribution scoring and primary selection

- **Deliverable:** evidence defaults, trust rank, refutation, and deterministic primary-worktree selection.
- **Files:** `src/agent_observatory/attribution.rs`, tests.
- **RED:** ambiguous/equal-confidence/refuted/below-threshold fixtures fail.
- **GREEN:** implement sorting and 0.75 primary threshold.
- **Proof:** randomized input ordering produces the same primary result.
- **Gate:** timestamp-only evidence never becomes primary.
- **References:** contract §5.

## AO-022 Add repository/worktree DB upserts

- **Deliverable:** transactional upsert/read methods preserving first_seen and removal history.
- **Files:** `src/db/agent_observatory.rs`, tests.
- **RED:** second reconcile must update mutable fields without changing IDs/first_seen.
- **GREEN:** parameterized SQL upserts and get/list helpers.
- **Proof:** create/update/remove/reappear fixture passes.
- **Gate:** canonical paths only and no raw SQL string interpolation.
- **References:** existing DB query conventions.

## AO-023 Parse worktree porcelain -z

- **Deliverable:** strict parser for normal, bare, detached, locked, and prunable records.
- **Files:** `src/git_observer/porcelain.rs`, tests with checked-in byte fixtures.
- **RED:** fixtures fail because parser is absent; malformed record reports bounded error.
- **GREEN:** parse NUL-delimited records without lossy human-output assumptions.
- **Proof:** outputs match `git worktree list --porcelain -z` fixtures exactly.
- **Gate:** unknown future labels are retained in metadata or skipped safely, not panic.
- **References:** Git worktree documentation in research ledger.

## AO-024 Parse status porcelain v2 -z

- **Deliverable:** branch/upstream/ahead/behind and staged/unstaged/untracked counts.
- **Files:** same parser module/tests.
- **RED:** fixtures for clean, dirty, rename, conflict, detached, no-upstream fail.
- **GREEN:** parse headers and records with NUL-safe names.
- **Proof:** generated temporary repos agree with direct expected state.
- **Gate:** filenames are counted but not persisted here.
- **References:** `git status --porcelain=v2 --branch -z`.

## AO-025 Add deterministic temporary Git fixture builder

- **Deliverable:** reusable test helper creates repo, commits, branches, linked worktree, detach, lock, reset, and rebase states with fixed identity/time.
- **Files:** `src/git_observer/test_support.rs` behind `cfg(test)`.
- **RED:** one smoke test expects exact SHAs/timestamps and fails.
- **GREEN:** use real system Git with isolated env and absolute paths.
- **Proof:** repeated fixture builds produce identical topology and commit subjects; skip with explicit reason only if Git missing.
- **Gate:** never touches global config.
- **References:** current inventory command runner tests.

## AO-026 Implement bounded repository discovery

- **Deliverable:** reusable discovery respecting roots, depth, count, symlinks, and ignored dirs.
- **Files:** `src/git_observer/discovery.rs`, tests; refactor `src/inventory/projects.rs` later to call it.
- **RED:** symlink, depth, cap, linked-worktree `.git` file, and permission fixtures fail.
- **GREEN:** canonical bounded traversal recognizes `.git` directories and files.
- **Proof:** test reports exact discovered set and warnings.
- **Gate:** no recursion into target/node_modules/.git/cache.
- **References:** current `discover_repos` behavior.

## AO-027 Implement one-repository reconciliation

- **Deliverable:** run bounded Git commands, parse output, and persist one repository plus all worktrees.
- **Files:** `src/git_observer/reconcile.rs`, DB helpers/tests.
- **RED:** temporary fixture expected DB rows absent.
- **GREEN:** use existing process runner; record discovered/status/head observations only when state changes.
- **Proof:** second unchanged reconcile adds no duplicate observation and updates last_seen.
- **Gate:** timeout/error produces health warning without deleting prior state.
- **References:** architecture “Reconcile commands.”

## AO-028 Detect removed and reappeared worktrees

- **Deliverable:** reconciliation marks missing worktrees removed and clears removal on reappearance.
- **Files:** reconcile/DB tests.
- **RED:** remove/re-add fixture leaves incorrect active rows.
- **GREEN:** compare observed set transactionally.
- **Proof:** history timestamps retained and stable worktree ID reused for same host/path.
- **Gate:** run/evidence foreign keys remain intact.
- **References:** spec §7.2.

## AO-029 Parse exact commit metadata

- **Deliverable:** NUL-safe parser for one bounded `git show` batch including parents, author, times, subject, numstat/path summary.
- **Files:** `src/git_observer/commits.rs`, tests.
- **RED:** merge, binary file, rename, non-UTF8-safe path fixture fails.
- **GREEN:** define one machine format and bounded parser.
- **Proof:** parsed SHA/parents/counts match real Git fixture.
- **Gate:** no patch/blob body and email only hashed when configured.
- **References:** spec §7.4-7.5.

## AO-030 Import fast-forward HEAD transitions

- **Deliverable:** enumerate `old..new`, persist exact commits and one head observation.
- **Files:** commits/reconcile/DB tests.
- **RED:** two-commit fast-forward fixture imports zero exact commits.
- **GREEN:** bounded `rev-list --reverse` plus batch metadata import.
- **Proof:** correct order, SHA, parents, and idempotent repeated reconcile.
- **Gate:** transition cap sets truncation warning and never runs unbounded.
- **References:** architecture “Exact commit attribution.”

## AO-031 Handle rewind, reset, rebase, and detached transitions

- **Deliverable:** preserve old commits, update reachability, and record non-fast-forward observation.
- **Files:** commit/reconcile tests.
- **RED:** reset/rebase fixture incorrectly deletes rows or marks all reachable.
- **GREEN:** classify ancestry with bounded Git checks and update observed reachability.
- **Proof:** historical commit remains queryable after reset; new head is correct.
- **Gate:** no destructive Git commands.
- **References:** spec §3.2 and §7.4.

## AO-032 Build Git watch-set planner

- **Deliverable:** pure planner returns project-root and Git control paths, not source-tree files.
- **Files:** `src/git_observer/watcher.rs`, tests.
- **RED:** large repo fixture expects bounded path count and fails.
- **GREEN:** include common-dir HEAD/index/refs/packed-refs/worktrees and per-worktree control dirs.
- **Proof:** adding 10,000 source files does not change watch count.
- **Gate:** path cap and canonical containment enforced.
- **References:** architecture “Watch set.”

## AO-033 Implement debounced Git watcher queue

- **Deliverable:** notify events coalesce by repository and schedule reconcile.
- **Files:** watcher/pending sidecars similar to `src/ai_watch`.
- **RED:** burst test expects one queued repo after many events.
- **GREEN:** bounded channel, pending map, debounce, and new-control-dir discovery.
- **Proof:** deterministic synthetic event test passes without sleeps by injected clock where practical.
- **Gate:** queue overflow signals full reconcile.
- **References:** `src/ai_watch.rs` overflow/debounce pattern.

## AO-034 Add overflow and periodic repair reconcile

- **Deliverable:** overflow schedules bounded full reconcile no more than configured rate; periodic 60s reconcile repairs missed events.
- **Files:** watcher supervisor/tests.
- **RED:** overflow storm triggers repeated scans or no repair.
- **GREEN:** atomic/coalesced flags and injected scheduler.
- **Proof:** 100 overflow notifications produce one reconcile in interval; missed state fixed on periodic tick.
- **Gate:** canonical ingest unaffected by failure.
- **References:** AI watcher overflow constants.

## AO-035 Add run/event/evidence DB write transaction

- **Deliverable:** one DB method upserts run/actor/evidence/event/outbox and updates counts atomically.
- **Files:** `src/db/agent_observatory.rs`, tests.
- **RED:** injected failure after event insert leaves partial state.
- **GREEN:** explicit transaction and conflict-safe deterministic keys.
- **Proof:** rollback test leaves no event/count/outbox; retry creates exactly one.
- **Gate:** outbox row appears only when materialized state changes.
- **References:** architecture projector idempotency.

## AO-036 Project transcript log rows

- **Deliverable:** classifier/projector converts Claude/Codex/Gemini transcript logs to run and transcript events.
- **Files:** `classifier.rs`, `projector.rs`, fixtures/tests.
- **RED:** one fixture per provider yields no run/event.
- **GREEN:** use existing `ai_tool/project/session/transcript_path` fields and bounded metadata.
- **Proof:** expected run keys/event keys/timestamps and idempotent replay.
- **Gate:** malformed/missing session row is skipped with diagnostic, not guessed.
- **References:** scanner and `ai_transcript_ingest.rs`.

## AO-037 Project agent commands and shell history

- **Deliverable:** command/shell events and cwd attribution evidence.
- **Files:** classifier/projector tests.
- **RED:** agent-command fixture fails to link verified cwd; Atuin fixture lacks claimed evidence.
- **GREEN:** distinguish source prefixes and metadata shapes.
- **Proof:** command exit/severity/duration preserved; weaker shell evidence never overrides verified.
- **Gate:** scrubbed command remains scrubbed.
- **References:** `src/command_log.rs`, `src/shell_history_ingest.rs`.

## AO-038 Project MCP, hook, skill, and LLM sources

- **Deliverable:** source adapters emit correctly typed events and optional actors/lifecycle evidence.
- **Files:** classifier/projector plus source fixtures.
- **RED:** one fixture per source table is absent from timeline.
- **GREEN:** page each table by ID and map existing session/project fields.
- **Proof:** exact source IDs, event kinds, and no duplication after second pass.
- **Gate:** source-specific payload caps and unknown event fallback.
- **References:** current DB modules and incident extraction.

## AO-039 Implement transactional source cursors and projector loop

- **Deliverable:** bounded page processing, same-transaction cursor advance, fallback poll, wake signal, retry health.
- **Files:** `projector.rs`, `supervisor.rs`, DB cursor methods/tests.
- **RED:** crash-before-commit and crash-after-commit simulations show gap/duplicate.
- **GREEN:** cursor and materialization share transaction; supervisor retries retry-safe errors.
- **Proof:** replay suite shows no loss/duplicates and source lag reaches zero.
- **Gate:** writer page cap <=500 rows/4MiB default.
- **References:** ADR 001 and architecture source cursors.

## AO-040 Implement resumable backfill and end-to-end Git attribution

- **Deliverable:** backfill engine with progress plus integration fixture combining transcript, command cwd, Git head change, and exact commit relation.
- **Files:** projector backfill/status models, integration tests.
- **RED:** cancellation/resume restarts from zero or duplicates; commit remains unattached.
- **GREEN:** persist job/cursor progress using existing jobs pattern or narrow new status storage; apply evidence scorer.
- **Proof:** cancel halfway, resume, compare final DB checksum/counts to uninterrupted run; exact commit linked with expected confidence.
- **Gate:** live source rows inserted during backfill are processed once.
- **References:** existing job models, contract admin backfill.

## Phase 2 gate

```bash
cargo test agent_observatory --lib
cargo test git_observer --lib
cargo test db::agent_observatory::projection --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Required proof:

- all supported source adapters replay idempotently
- crash/cancel/resume tests show no loss or duplicates
- exact Git SHA attribution works for fast-forward and stays historically correct after reset/rebase
- watcher path count remains bounded independent of source-file count
- overflow and periodic repair tests pass
- canonical log insert succeeds while projector is intentionally failing
