> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-07-14 04:56:40 EST
repo: git@github.com:jmagar/cortex.git
branch: claude/canonical-entity-resolution-ea34c0
head: c2ead9fa
plan: docs/superpowers/plans/2026-07-13-canonical-entity-resolution.md
working directory: /home/jmagar/workspace/cortex/.claude/worktrees/canonical-entity-resolution-ea34c0
worktree: /home/jmagar/workspace/cortex/.claude/worktrees/canonical-entity-resolution-ea34c0
pr: "#133 feat: canonical entity resolution for the investigation graph — https://github.com/jmagar/cortex/pull/133"
beads: syslog-mcp-vkln9 (epic, .1–.7), syslog-mcp-mmidy, syslog-mcp-jklp4, syslog-mcp-q414s, syslog-mcp-hk9g8, syslog-mcp-2g5g7, syslog-mcp-auy5f, syslog-mcp-8kbki, syslog-mcp-he2g3, syslog-mcp-nzm6e, syslog-mcp-g3fgk, syslog-mcp-kr2gr, syslog-mcp-ofznu, syslog-mcp-5k1zb, syslog-mcp-6ipjl, syslog-mcp-k5i1x, syslog-mcp-csukc, syslog-mcp-sfm5o, syslog-mcp-4hfzi, syslog-mcp-k9jnf, syslog-mcp-9n4g8, syslog-mcp-jd0j1
---

# work-it: canonical entity resolution (PR #133)

## User Request

`work-it 2026-07-13-canonical-entity-resolution.md` — execute the canonical entity-resolution plan to completion in a tracked worktree PR with mandatory review waves.

## Session Overview

Executed the full 7-task plan via a dedicated implementation agent, then ran two complete review waves (7 independent lavra reviewers + 6 PR-review-toolkit passes) and three fix waves. Result: 25 commits on `claude/canonical-entity-resolution-ea34c0`, PR #133 (draft) with all CI green, all quality gates green (fmt, clippy, 1987 lib + 521 bin + integration tests, identity scan, npm launcher check), 12 review-finding beads fixed and closed, 9 follow-up/pre-existing beads filed for triage.

## Sequence of Events

1. Reused the harness-created worktree; ran worktree-sync (copied `.env`, `CLAUDE.md.local`; linked caches; mise trust) to full parity.
2. Pushed branch with an empty bootstrap commit; created draft PR #133.
3. Implementation agent executed all 7 plan tasks TDD-style (commits `c330cc23`..`7f95dfae`), claimed/closed beads `syslog-mcp-vkln9.1`–`.7` and the epic.
4. Fixed CI failure: npm-launcher README byte-parity (`2143bd28`).
5. Review wave 1 (lavra equivalent): architecture, security, performance, data-integrity, patterns, goal-verifier, simplicity — filed 12 fix beads + 7 pre-existing beads with LEARNED/PATTERN knowledge comments.
6. Fix wave 1: all 12 beads fixed in commits `8c417504`..`659f2dc9` (query-plan rewrite, classifier guards, dot-preserving keys, prune leak, fan-out scoping, marker gating, migration rework, resolver wiring, dedupe, hot-path memo, evidence redaction, misc P3s).
7. Review wave 2 (PR review toolkit): code (no blockers ≥80 confidence), tests, comments/docs-config, silent-failures, type-design — consolidated 30 findings.
8. Fix wave 2: five batches in commits `7101616d`..`786aa2c8` (trust-doc + mixed-trust test, vocab hoisting + ResolverStatus through db layer, prefix-gate validation + skip tracing, stranded rustdoc + doc alignment, migration-cascade/fan-out/key-grammar tests); filed 2 deferred beads.
9. Simplifier pass: shared UNION-ALL runner, redundant marker scan removed (`c2ead9fa`).
10. Re-verified all gates independently after each wave; updated PR body to final state; confirmed CI green.

## Key Findings

- The plan's inventory-resolver adapter (`observations_from_inventory_service`) shipped dead in the first implementation; `src/db/graph_inventory.rs` reimplemented identity inline. Fixed by wiring the adapter through `resolve_observations`.
- `search_logs_for_service_instances` originally produced MULTI-INDEX OR + temp b-tree plans (EXPLAIN-verified); rewritten as per-key UNION ALL arms with per-arm LIMIT (`src/db/queries.rs`), pinned by an EXPLAIN QUERY PLAN test.
- `classify_legacy_shape` rejected any colon-bearing free text (`error: disk full`, `10.0.0.5:443`); guards added for whitespace, non-name colon segments, and absolute paths (`src/db/entity_resolution/vocab.rs`).
- `canonical_component` mapped `.`→`-`, silently breaking FQDN host matching; dots are now preserved.
- The `[cortex-agent-docker-meta:...]` marker was spoofable by any 1514 sender at Verified trust; now optionally gated by `CORTEX_AGENT_DOCKER_SOURCE_PREFIXES` (Authelia-style octet-boundary matching) with the trust boundary documented, merge scoped to `agent_docker`, no key overwrites.
- Migration 41 (`src/db/pool.rs`) now excludes legacy rows inside the `INSERT…SELECT` (no copy-then-delete), adds `idx_graph_entities_canonical_key`, and the incremental refresh probes for contract drift (downgrade→re-upgrade) forcing cleanup + full rebuild.
- Operator-visible: after migration 41 the graph projection is marked stale; `cortex graph rebuild` must run before `topic_correlate` service results populate (documented in README + openwiki).

## Technical Decisions

- Trust aggregation is strongest-wins (`min()` over `ResolverTrust` with Verified first): matches the plan's "verified outranks claimed" intent; doc corrected, mixed-trust test pins it.
- Hostname case sensitivity NOT changed at ingest (BINARY matching stands, limitation documented + test-pinned); decision deferred to bead `syslog-mcp-jd0j1`.
- Log-driven and inventory-driven `instance_of` edges keep distinct `relationship_key` shapes (verified no orphaned evidence; asserted by test rather than forcing convergence).
- `graph_walk_n_hops` unboundedness left as pre-existing scope (`syslog-mcp-k5i1x`); the new `graph_walk_service_topic` is aggregate-budget-bounded.
- Legacy `service` string kept in migration CHECK constraints for old-row tolerance, but removed from `ENTITY_TYPES` validation so lookups reject it.

## Files Changed

56 files, ~3,900 insertions net across 25 commits. Highlights (all under the worktree root):

| status | path | purpose |
|---|---|---|
| created | src/db/entity_resolution.rs + entity_resolution/{vocab,observation,adapters,resolver}.rs + entity_resolution_tests.rs | resolver module: key grammar, observations, deterministic decisions, diagnostics |
| created | scripts/validate-canonical-plex-graph.sh | read-only Plex proof workflow (refuses rebuild, exit 2) |
| created | docs/sessions/2026-07-14-canonical-entity-resolution-work-it.md | this session log |
| modified | src/db/{pool,graph,graph_inventory,graph_findings,queries,models}.rs (+sql, +tests) | migration 41, resolver-backed projection, UNION-ALL fan-out, prune fixes |
| modified | src/app/services/{graph,graph_support,graph_safety,map_answers,topic_correlate}.rs, map_findings/risky_mounts.rs (+tests) | legacy-shape rejection, redaction, inclusion metadata |
| modified | src/agent/docker.rs, src/receiver/enrichment.rs (+tests), src/ingest_metadata.rs | structured agent-docker identity marker + gated extraction |
| modified | src/mcp/schemas.rs, src/cli/{commands/graph.rs,dispatch/surface/gap.rs,output} (+tests) | logical_service/service_instance surfaces |
| modified | docs/contracts/*, docs/mcp/{TOOLS,SCHEMA}.md, docs/CLI.md, openwiki/*, README.md, CLAUDE.md, config.toml, plugins/cortex/skills/topology/SKILL.md | contract + operator docs, env var table, [enrichment] example |
| modified | scripts/rust-module-size.allow, packages/cortex-rmcp/README.md | pre-existing oversized modules allowlisted; README byte-parity |

## Beads Activity

- Claimed + closed: `syslog-mcp-vkln9` epic and children `.1`–`.7` (plan tasks).
- Created + fixed + closed (review wave 1 findings): `syslog-mcp-mmidy` (P1 query plan), `jklp4`, `q414s`, `hk9g8`, `2g5g7`, `auy5f`, `8kbki`, `he2g3`, `nzm6e`, `g3fgk`, `kr2gr`, `ofznu` — each with LEARNED/PATTERN knowledge comments.
- Created for triage (pre-existing/deferred, open): `syslog-mcp-5k1zb` (agent triplet APP-NAME decision), `6ipjl` (module extraction), `k5i1x` (unbounded graph_walk_n_hops, P2), `csukc` (topic-entity LIKE scans), `sfm5o` (metadata parse dedup/memo for non-resolver paths), `4hfzi` (aliases trust CHECK + evidence bucket key), `k9jnf` (container key colon-split), `9n4g8` (gated-marker counter + walk_truncated flag), `jd0j1` (hostname case normalization decision).
- `bd dolt push` performed by the fix agents after closes.

## Repository Maintenance

- Plans: `docs/superpowers/plans/2026-07-13-canonical-entity-resolution.md` is complete but was NOT moved during quick-push per skill constraints; follow-up: move to a `complete/` location in a later docs pass. Other plans under `docs/plans/` untouched (not this session's scope).
- Beads: see Beads Activity — all session-relevant beads created/closed; verified via `bd list`.
- Worktrees/branches: no cleanup — this worktree backs open PR #133; `main` checkout untouched; no stale branches removed (none proven merged).
- Stale docs: contract docs, openwiki, README, CLAUDE.md env table updated in-session where the implementation proved them stale.
- No-ops: `.cache`/`dist` untracked symlinks left in place (worktree-sync artifacts, gitignored dirs).

## Tools and Skills Used

- Skills: `vibin:work-it` (orchestration), `vibin:worktree-setup` (sync/doctor), `superpowers:executing-plans` (implementation agent), `lavra:lavra-review` (wave 1), `vibin:review-pr` (wave 2), `vibin:quick-push` + `vibin:save-to-md` (this log).
- Agents: 1 implementation, 7 wave-1 reviewers (architecture/security/performance/data-integrity/patterns/goal/simplicity), 2 fix agents, 5 wave-2 toolkit reviewers, 1 simplifier. Two fix agents stalled waiting on background test monitors and were resumed via SendMessage — work was unaffected.
- Shell: git/gh, cargo (fmt/clippy/test), bd, npm, sqlite via scripts.
- Issues: `bd create --tags` flag does not exist in this bd version (use `-l/--labels`); initial bead batch silently failed under `tail -1` and was redone.

## Commands Executed

| command | result |
|---|---|
| cargo fmt --check | clean (every wave) |
| cargo clippy --all-targets | zero warnings (every wave) |
| cargo test | final: 1987 lib + 521 bin + 12 integration binaries, 0 failed |
| bash scripts/check-public-identity.sh | OK |
| bash scripts/validate-canonical-plex-graph.sh | exit 0; old_key_count=3, new_key_count=0 on dev DB (expected pre-rebuild) |
| npm run check --prefix packages/cortex-rmcp | ok |
| gh pr checks 133 | all checks pass on final HEAD (3 pending at last snapshot completed pass) |

## Errors Encountered

- CI `npm launcher` failed on first push: package README must be byte-identical to repo README after Task 7's README edit; fixed by `sync-readme.js` (`2143bd28`).
- `bd create --tags` unknown flag — silent failure hidden by `tail -1`; re-ran with `-l`.
- Two fix agents ended their turn while a background test run was still executing; resumed via message, gates then run in foreground.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| graph identity | `service:nashost:plex`, `service:nashost:plex:plex`, `app:plex/plex/plex` | `logical_service:plex`, `service_instance:nashost/plex`, `instance_of` edges; legacy shapes rejected with `rejected_legacy_shape` |
| topic_correlate plex | host-splitting fan-out over all host logs | service-instance predicates with `inclusion_reason`/`resolver_status`/`fallback_kind`; explicit degraded host fallback only |
| docker identity source | app-label string surgery | structured `metadata_json.agent_docker` from host agents (optional source-prefix anti-spoof gate) |
| migration | n/a | migration 41 rebuilds graph tables excluding legacy rows; marks projection stale (operator must rebuild) |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| cargo test (final HEAD) | 0 failures | 1987+521+integration passed, 0 failed | pass |
| cargo clippy --all-targets | 0 warnings | 0 warnings | pass |
| gh pr checks 133 | all pass | all pass (Tests, Clippy, Coverage, MCP Integration, build-and-push, npm launcher, secret scans) | pass |
| EXPLAIN QUERY PLAN test (queries) | index search, no temp b-tree | pinned in `service_instance_fanout_arms_use_index_search_without_temp_btree` | pass |

## Risks and Rollback

- Migration 41 is one-shot and runs at next startup on a populated DB; it deletes legacy `service`/nested-`app` graph rows (the graph is a rebuildable projection — no log data is touched). Rollback: restore from `cortex db backup` WAL-safe backup taken before deploy; downgrade tolerance is built in (legacy values remain valid in CHECKs; contract-drift probe self-heals on re-upgrade).
- Post-deploy, `topic_correlate` service results are empty until `cortex graph rebuild` runs (documented in README/openwiki; surfaced as `resolver_status: degraded` no_instances annotation).
- Mixed-case syslog hostnames won't match canonical service-instance predicates (documented limitation; bead `syslog-mcp-jd0j1`).

## Decisions Not Taken

- Did not downgrade agent-docker trust to Claimed (plan mandates Verified for agent metadata); chose optional source-prefix gating + documentation instead.
- Did not normalize hostname case at ingest (cross-surface impact; deferred to bead).
- Did not implement per-level walk caps (doc corrected to match aggregate budget instead).
- Did not force log/inventory `instance_of` edge convergence (distinct key shapes verified safe).

## Open Questions

- Should the agent stop emitting the `proj/svc/name` triplet APP-NAME now that identity rides metadata (`syslog-mcp-5k1zb`)? Affects not-yet-upgraded agents' graph identity.
- Production deploy scheduling for migration 41 + rebuild on nashost (off-peak, backup first — runbook in openwiki/inventory-graph.md).

## Addendum: CodeRabbit round (post-log)

After this log was first saved at HEAD `c2ead9fa`+`07467fce`, the PR was marked ready for review, triggering CodeRabbit. All 10 of its review comments were valid and fixed in commits `cfa0ab8c`, `e37d66ee`, `a8da1859`, `dc6ca31e`, `41c707b7`: contract clarification (schema-tolerated vs lookup-supported `service`), compose-project key separator in the Plex proof doc, `sqlite3 -readonly` in the validation script, shared `AGENT_DOCKER_SOURCE_KIND` constant, canonicalization of slash-form `service_dependencies` input, accurate resolver evidence paths, URL-safe legacy-shape classifier (`http://…` no longer rejected), chunked legacy-topology cleanup (2000-row phases releasing the write lock), startup warning when the agent-docker source gate is empty, and IPv6-aware `source_ip_matches`. All gates re-ran green (2500+ tests, 0 failed); replies posted and all 10 threads resolved. Untracked `.cache`/`dist` worktree symlinks were excluded via `.git/info/exclude`, and the PR was undrafted.

## Next Steps

1. Merge PR #133 when desired — all gates, review waves, and PR comments are resolved; it is marked ready for review.
2. After merge + deploy: `cortex db backup`, deploy, then `cortex graph rebuild`, then `scripts/validate-canonical-plex-graph.sh` (expect old_key_count=0, new_key_count>0).
3. Triage the 9 open follow-up beads (`bd list` — labels `review-sweep`/`pre-existing`); highest value: `syslog-mcp-k5i1x` (unbounded generic walk, P2) and `syslog-mcp-jd0j1` (hostname case policy).
4. Move the completed plan file to a `complete/` folder in a future docs pass (not done during quick-push by design).
