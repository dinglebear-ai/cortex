---
title: "Retention & Eviction Policy"
created: 2026-05-16
updated: 2026-08-24
---

# Retention & Eviction Policy

**Status:** Contract — source of truth
**Date:** 2026-05-16
**Pinning header:**

> Contract derived from cross-cutting audit of `src/config.rs`,
> `src/runtime.rs`, `src/db/maintenance.rs`, and all six Epic specs in
> `docs/superpowers/specs/`. **Supersedes** scattered retention notes in
> individual specs (Epic D probe-registry §6.x, Epic E digest §6, Epic F
> RAG §11) and the prior `CLAUDE.md` "Retention" section. Changing any
> retention number requires updating this contract and the cited code path.

**Current implementation note:** The active retention code covers `logs`,
`logs_fts`, AdGuard tag-window purges, and existing operational tables. The
Epic F incident/Qdrant rows below remain historical design targets until the
semantic incident pipeline lands.

---

## 1. Scope

This is the **single canonical place** for every persistent-table eviction
policy in cortex. Each row in §3 names exactly one (table, retention
mechanism) pair. If you can't find your table here, it has no eviction —
that's the policy.

Two policy axes interact:

1. **Time-based retention** (opportunistic): a periodic task evicts rows
   older than `retention_days`. This is the "normal" mechanism.
2. **Disk-budget guardrails** (forceful): if `max_db_size_mb` or
   `min_free_disk_mb` is breached, an aggressive prune runs *regardless of
   age* — oldest-first, no severity filter. This is the "emergency"
   mechanism.

The guardrail can override the time-based exemption for high-severity rows
(see §5). The retention task cannot.

---

## 2. Retention defaults at a glance

| Knob | Default | Source |
|------|---------|--------|
| `storage.retention_days` | **90 days** | `src/config.rs::default_retention_days()` |
| `storage.max_db_size_mb` | 1024 MB (1 GiB) | `src/config.rs::default_max_db_size_mb()` |
| `storage.recovery_db_size_mb` | 900 MB | `src/config.rs::default_recovery_db_size_mb()` |
| `storage.min_free_disk_mb` | 0 MB (disabled) | `src/config.rs::default_min_free_disk_mb()` |
| `storage.recovery_free_disk_mb` | 0 MB (disabled) | `src/config.rs::default_recovery_free_disk_mb()` |
| `storage.cleanup_interval_secs` | 60 s (guardrail tick) | `src/config.rs::default_cleanup_interval_secs()` |
| `storage.cleanup_chunk_size` | 2000 rows / chunk | `src/config.rs::default_cleanup_chunk_size()` |
| AdGuard tag retention (hardcoded) | **7 days** | `src/runtime.rs::ADGUARD_RETENTION_DAYS` |

The 90-day default reflects current production reality (the 4.9M-row prod
box). Operators who run on smaller hosts will commonly set
`CORTEX_RETENTION_DAYS=30` or lower.

---

## 3. Retention table (canonical)

| Table / namespace | Raw retention | Downsample / aggregate retention | Owner subsystem | Eviction cadence | Knob | Override mechanism | Source |
|---|---|---|---|---|---|---|---|
| `logs` (general) | `storage.retention_days` (default 90 d). **High-severity exempt:** rows with `severity IN ('err','crit','alert','emerg')` are NOT aged out by time-based purge. | n/a | `purge_old_logs` task in `RuntimeCore::spawn_retention_task` | hourly (1 h ticker) | `CORTEX_RETENTION_DAYS` env / `[storage].retention_days` TOML | Per-tag override for `adguard-*` (see next row); high-severity exemption hardcoded | `src/db/maintenance.rs::purge_old_logs` |
| `logs` with `app_name IN ('adguard-allowed','adguard-query','adguard-rewrite')` | **7 days, hardcoded** (overrides global retention) | n/a | `purge_by_tag_window` called from `RuntimeCore::spawn_retention_task` per-tag, BEFORE the global purge | hourly (same tick) | **none — hardcoded `ADGUARD_RETENTION_DAYS = 7` in `src/runtime.rs`**. Promotion to config is filed as an open question (§7). | High-severity exemption applies here too — `err+` adguard rows are kept. | `src/runtime.rs:55–59`, `src/db/maintenance.rs::purge_by_tag_window` |
| `logs_fts` (FTS5 shadow) | Follows `logs` (rows are mirrored via FTS5 triggers on INSERT only — DELETE triggers were intentionally dropped per Migration 1). Incremental merge runs after each purge cycle (`PRAGMA fts5(merge,M)`) with `M = CORTEX_FTS_MERGE_PAGES` (default 0 = unconditional). | n/a | `fts_incremental_merge` called from `purge_old_logs` and `purge_by_tag_window` | piggy-backs on log purge | `CORTEX_FTS_MERGE_PAGES` (0..=10000) | n/a | `src/db/maintenance.rs::fts_incremental_merge` |
| `hosts` | **Never evicted.** Derived table — populated as logs land; small (one row per host). | n/a | n/a | n/a | n/a | n/a | `current-schema.sql` |
| `docker_ingest_checkpoints` | **Never evicted.** One row per (Docker host, container); small. | n/a | n/a | n/a | n/a | n/a | `current-schema.sql` |
| `poller_checkpoints` | **Never evicted.** One row per `(poller, instance)`; small. Survives across restarts (purpose of the table). | n/a | n/a | n/a | n/a | n/a | Epic C spec §4 ("Checkpoint Store") |
| `transcript_sources`, `transcript_import_records`, `transcript_parse_errors` | **Never evicted.** Operational/audit tables for the AI transcript scanner. Small in steady state (one row per source / per import / per error). | n/a | n/a | n/a | n/a | n/a | `current-schema.sql` |
| `host_heartbeats` | **14 days, hardcoded** (`src/runtime.rs::HEARTBEAT_RETENTION_DAYS`). Deleted oldest-first in `cleanup_chunk_size` chunks. | n/a | `purge_old_heartbeats` in `RuntimeCore::spawn_retention_task` | hourly (same 1 h ticker as the log purge) | **none — hardcoded `HEARTBEAT_RETENTION_DAYS = 14`** | n/a | `src/db/maintenance.rs::purge_old_heartbeats` |
| `heartbeat_cpu`, `heartbeat_memory`, `heartbeat_disks`, `heartbeat_network`, `heartbeat_processes`, `heartbeat_containers` | Follows `host_heartbeats` — children are deleted in the **same transaction** as their parent, so the normal path never strands one. A separate orphan sweep exists as a repair path for legacy/residue rows; its cadence is **not** hourly, see §4.1. | n/a | `delete_heartbeat_chunk_where` (parent path) and `delete_orphan_heartbeat_children` (repair path) | parent: hourly; orphan sweep: see §4.1 | none | n/a | `src/db/maintenance.rs::HEARTBEAT_CHILD_TABLES` |
| `agents` (Epic A) | **Never evicted.** Revoked rows are kept indefinitely as an audit trail (`state = Revoked`, both `token_hash` columns NULL). Approx 1 row per host ever onboarded; bounded by fleet size. | n/a | n/a | n/a | n/a | n/a | `docs/superpowers/specs/2026-05-16-agent-mode-design.md` §11 |
| `host_metrics` (Epic A pre-create) | V1 drops writes (placeholder column). Once Epic D wires it, it follows `metrics_gauge` policy below. | n/a | n/a | n/a | n/a | n/a | Epic A spec §11 |
| `metrics_gauge` (Epic D) | **Raw 14 days.** Rows older than 14 d are deleted by the rollup task once they have been folded into the 5-minute rollup. | 5-min rollup: 90 d; 1-h rollup: 365 d | `db::maintenance::rollup_metrics_gauge` (new in Epic D) | hourly rollup + daily prune | `CORTEX_METRICS_RAW_DAYS` / `_5M_DAYS` / `_1H_DAYS` (proposed; defaults are normative) | n/a | Epic D spec §6, "Retention" lines 219–222 |
| `metrics_gauge_5m` (Epic D rollup) | 5-min downsampled rows: **90 days.** | n/a (terminal rollup tier for short queries) | Same task as `metrics_gauge` | hourly insert + daily prune | `CORTEX_METRICS_5M_DAYS` | n/a | Epic D spec §6. **DDL lives in `db-additions.sql` after the patches agent adds it** — the auditor noted this table is referenced in the spec but the DDL is currently missing from the additions file. |
| `metrics_gauge_1h` (Epic D rollup) | 1-h downsampled rows: **365 days.** | n/a (terminal rollup tier) | Same task as `metrics_gauge` | daily insert + weekly prune | `CORTEX_METRICS_1H_DAYS` | n/a | Epic D spec §6. **DDL lives in `db-additions.sql` after the patches agent adds it** — see note above. |
| `probe_results` (Epic D) | **Last N per `(host_id, probe_name)` where N=200**, AND a 30-day ceiling. Whichever fires first. | n/a | `db::maintenance::evict_probe_results` (new in Epic D) | hourly | `CORTEX_PROBE_RESULTS_PER_GROUP` (default 200) / `_PROBE_RESULTS_MAX_DAYS` (default 30) | n/a | Epic D spec §6.1 lines 259–262 (composite-index-friendly DELETE) |
| `alert_state` (Epic E) | Rows where `ack_at IS NOT NULL AND ack_at < now() - 30 days` are GC'd. Active rows (`ack_at IS NULL`) are never aged out. | n/a | Existing purge task — Epic E adds a sub-pass | piggy-backs on the hourly log purge | not configurable in V1 (30 d is hardcoded in the GC SQL — proposed `CORTEX_ALERT_STATE_GC_DAYS` for V1.1) | n/a | Epic E spec §6 line 312 ("Stale clear") |
| `incidents` (Epic F) | **90-day ceiling.** Resolved-and-acked incidents older than 90 d are GC'd from SQLite. Open incidents (`last_seen` within `window_close`) are never aged. | n/a | `db::maintenance::gc_incidents` (new in Epic F) | daily | `CORTEX_INCIDENT_RETENTION_DAYS` (default 90) | The Qdrant vector for the GC'd incident is also deleted (point id = `incident_id`); see §6. | Epic F spec §11; `docs/contracts/incident-card.md` §7 |
| Qdrant collection `incidents` (Epic F) | Follows `incidents` table — deletion cascades to Qdrant point on GC. **Re-embed on `schema_version` bump:** all incidents in the **last 90 days** are re-rendered and re-embedded. Older incidents stay at their previous schema version. | n/a | `db::maintenance::backfill_incident_embeddings` (new in Epic F) — re-embed task triggered by manual CLI or schema-version bump | on-demand | `CORTEX_INCIDENT_REEMBED_DAYS` (default 90, matches incident retention ceiling) | n/a | `docs/contracts/incident-card.md` §7 "Compatibility & schema bumps" |
| `/data/incidents/{incident_id}.md` (Epic F card staging) | Follows `incidents` table — file is deleted on GC of the SQLite row. Files for `embed_status != embedded` are kept indefinitely so the embed worker can retry. | n/a | `gc_incidents` cascades | daily | `CORTEX_INCIDENT_RETENTION_DAYS` | n/a | Epic F spec §4 |

---

## 4. Eviction order and concurrency

Within each hourly retention tick, the order is deterministic (per
`src/runtime.rs::spawn_retention_task`):

1. **Tag-window purges first** (currently only `adguard-*`). Reason: smaller
   working set than the global purge, and FTS merge work consolidates if a
   single purge cycle covers multiple tags.
2. **Global `purge_old_logs`** with the configured `retention_days`. Excludes
   high-severity rows.
3. **FTS5 incremental merge** (one merge call per purge that deleted > 0
   rows, controlled by `CORTEX_FTS_MERGE_PAGES`).
4. **WAL passive checkpoint** to prevent unbounded WAL file growth between
   restarts.

Epic D, E, F retention tasks layer on top of this and run on their own
cadences (see §3). They MUST acquire `MaintenanceHandles::permit` (the
same single-writer semaphore the existing purge uses) before deleting from
`logs` or `alert_state` — concurrent chunked DELETEs over the same table
cause SQLite write-lock contention that stalls the batch writer.

### 4.1 Orphan heartbeat-child sweep cadence

Heartbeat child rows (§3) are deleted in the same transaction as their
parent, both on the write path and in `delete_heartbeat_chunk_where`. Neither
can strand a child, so **orphans are not a steady-state condition** — any
that exist are legacy rows or residue from a manual repair.

Finding them, however, means a full scan of all six child tables, because a
`LIMIT`ed `DELETE ... WHERE NOT EXISTS` still scans the whole table before
returning nothing when there are zero orphans. Running that on the hourly
retention tick is pure waste. One production retention tick (~79 GB database,
2026-08-24) spent 15m25s in this sweep, dominated by those scans.

The sweep therefore runs on its own cadence, decided by the **caller** and
passed in as `db::OrphanSweep`:

| Caller | Cadence | Why |
|---|---|---|
| `RuntimeCore::spawn_retention_task` → `purge_old_heartbeats` | **First retention tick after start, then every 24th tick (daily).** `src/runtime.rs::ORPHAN_SWEEP_EVERY_N_TICKS = 24`, against the 1 h ticker. | The first-tick run clears any backlog left by a crash promptly; daily thereafter is enough for a condition that should never arise. Every other hourly tick passes `OrphanSweep::Skip` and reports orphan counts as zero. |
| `db::maintenance::enforce_storage_budget_with_state` | **Unconditionally, once per triggered recovery** — before the trim loop, not inside it. | Orphan rows are free space, so reclaiming them is the cheapest way to recover before trimming live telemetry. It is hoisted out of the trim loop because `delete_orphan_heartbeat_children` already loops internally until no orphans remain; running it per iteration re-scanned all six tables on every pass. A failure here is logged and the trim continues — reclaiming orphans is an optimisation, and a transient pool timeout must not abort the trim that is meant to relieve a full disk. |

The sweep itself is chunked and **does not hold the global write lock across
the scan**: phase 1 identifies orphan rowids as a plain WAL read, and the
write lock is taken only for a bounded `DELETE` of rowids already known.
Chunks are clamped to 500 rowids per `DELETE` to stay under SQLite's bind-
parameter ceiling regardless of `cleanup_chunk_size`.

This split is safe only because `host_heartbeats.id` is `AUTOINCREMENT`:
ids are never reissued, so a rowid seen as an orphan in phase 1 cannot have
acquired a live parent by phase 2. If that column is ever rebuilt without
`AUTOINCREMENT`, the sweep must revert to a single atomic
`DELETE ... WHERE NOT EXISTS`.

---

## 5. Disk-budget guardrails (forceful eviction)

The guardrail task (`src/db/maintenance.rs::enforce_storage_budget`, ticking
every `cleanup_interval_secs = 60 s`) is **separate** from the retention
task. It runs only when a trigger is breached:

- `db_file_size_mb > storage.max_db_size_mb`, OR
- `free_disk_mb < storage.min_free_disk_mb`.

When triggered, it deletes the **oldest rows first** in chunks of
`cleanup_chunk_size` (default 2000), with **no severity filter** — until
the recovery target is reached (`recovery_db_size_mb` / `recovery_free_disk_mb`).
The high-severity exemption that protects err+ rows from time-based purge
is **overridden** here. A WARN-level log line announces the override.

Validation rules (enforced at config load, `validate_storage_config`):

- `recovery_db_size_mb < max_db_size_mb` (cannot recover above the trigger).
- `recovery_free_disk_mb > min_free_disk_mb` (cannot recover below the
  trigger).
- `cleanup_interval_secs >= 5` (avoid hammering).
- `cleanup_chunk_size > 0` and `<= 1_000_000` (avoid holding the write
  lock for seconds at a time).

**Interaction with time-based retention:** time-based retention is the
preferred normal path. Once the disk-budget guardrail kicks in, the operator
is by definition over-provisioned for their disk — the recommended response
is to (a) reduce `CORTEX_RETENTION_DAYS`, or (b) raise `max_db_size_mb`
on a host with more disk. The guardrail is meant as a backstop, not the
steady-state mechanism.

---

## 6. The AdGuard 7-day exception (explainer)

`runtime.rs` hardcodes a 7-day retention on three log tags:

- `adguard-allowed`
- `adguard-query`
- `adguard-rewrite`

**Why:** AdGuard Home running on a homelab pi-hole replacement generates DNS
query volume on the order of 50k–500k rows/day per active client subnet. At
30-day retention, this dominates the FTS5 index (cardinality is per-domain,
which is high) and degrades `logs.search` latency for the operationally
interesting tags. The 7-day cap holds DNS volume at roughly the same row
budget as 30 days of regular syslog at the current production fleet.

**Production data point:** the 4.9M-row prod box would 10x without this
exception, per the deployment audit. The cap is load-bearing.

**Why this is in `runtime.rs` and not config:** the original intent was a
quick hardcode pending a "per-app retention" feature that never landed.
Promoting to `[storage].adguard_retention_days` is filed as an **open
question** for V1.1 (see §7) — it is a small change but the existing
hardcode is well-understood and operators can override via the open path
(disable the AdGuard parser to drop the tag entirely, or lower
`retention_days` globally if their entire corpus fits).

**Discoverability hazard:** an operator reading `[storage]` config sees
`retention_days = 90` and may believe AdGuard logs are kept for 90 days.
They are not. This document is the discovery mechanism; the in-code comment
at `src/runtime.rs:55–59` is the secondary mechanism.

---

## 7. Eviction failure handling

The retention task is **idempotent** under crash:

- Each chunked DELETE runs inside its own SQLx transaction and releases the
  write lock before sleeping 50 ms between chunks. If the task is killed
  mid-pass, the rows already deleted stay deleted; the next tick picks up
  where it stopped (the cutoff timestamp is computed fresh each tick).
- FTS5 incremental merge is best-effort — if it fails, a warning is logged
  and the next purge cycle retries. The FTS index stays correct because the
  INSERT triggers keep it in sync; DELETEs accumulate phantoms which the
  next merge will collapse.
- WAL checkpoint is best-effort; failure is logged at WARN and the next
  cycle retries.

If a retention task **panics**, the supervising task in `RuntimeCore`
catches the panic, logs ERROR, and restarts the task on the next tick. No
data is lost; the only consequence is a delayed eviction.

Epic D, E, F tasks MUST adopt the same chunk-and-yield pattern. The 2000-row
chunk size and 50 ms inter-chunk sleep are normative and exist to prevent
batch-writer starvation.

---

## 8. Re-embed and backfill policy

### Qdrant re-embed (Epic F)

The Qdrant collection `incidents` is re-embedded on a
schema-version bump for **incidents in the last 90 days** (the freshness
window). Older incidents stay at their prior `schema_version` — recall@5
against ancient incidents is already noisy, and re-embedding a year of
history burns axon/TEI capacity for diminishing returns. See
`docs/contracts/incident-card.md` §7 for the trigger conditions.

When `embed_status` is `pending`, the staged card file at
`/data/incidents/{incident_id}.md` is preserved across retention cycles so
the embed worker can retry. Files are deleted only when the SQLite row is
GC'd (90 d post-resolution-and-ack).

### Backfill — additive columns are NOT auto-backfilled

This is normative: **the retention task does not backfill historical rows
when a new column is added.**

- Epic B (enrichment framework) explicitly punts retroactive backfill of
  the existing 4.9M rows to V1.1 via an opt-in `cortex backfill --since
  <ts>` CLI subcommand (per Epic B spec §11, lines 372–376). No live
  retention-task backfill in V1.
- Epic D (probes), Epic E (alerts), Epic F (incidents) introduce new
  tables — there is no "old data to backfill" by definition. Forward-only.

If a backfill is needed it is **its own subcommand**, run on operator
intent, NOT a side effect of the maintenance loop. The maintenance loop
deletes; it does not transform.

---

## 9. Disk budget envelope (summary)

The envelope is the interaction between time-based retention and the
disk-budget guardrails:

```text
                  cleanup_interval_secs (60s)
                  ┌─────────────────────────┐
   db_size  ──────│                          │── guardrail forceful prune
   free_disk──────│  enforce_storage_budget  │     (oldest-first, no
                  │                          │      severity filter,
                  └──────────────┬───────────┘      chunked)
                                 │
                  retention_days │
                  ┌──────────────┴───────────┐
   age      ──────│  purge_old_logs (hourly) │── opportunistic time prune
   tag (adguard-*)│  purge_by_tag_window     │    (err+ exempt)
                  └──────────────────────────┘
```

**Defaults imply ~1 GiB hot DB, 512 MiB free disk headroom.** Operators on
larger hosts should bump `max_db_size_mb` and `min_free_disk_mb` together
(disabling either by setting to 0 also requires zeroing its `recovery_*`
counterpart, per the validation rules in §5).

If retention is keeping pace, the guardrail should never fire in steady
state. If the guardrail is firing regularly, retention is too long for the
hardware — reduce `CORTEX_RETENTION_DAYS`. The guardrail is an
emergency brake, not a normal mechanism.

---

## 10. Operator levers

### Reduce overall retention to N days

```toml
# config.toml
[storage]
retention_days = 14
```

or:

```sh
export CORTEX_RETENTION_DAYS=14
```

### Disable retention entirely (debug / forensics)

```toml
[storage]
retention_days = 0   # 0 = keep forever
max_db_size_mb = 0   # 0 = disable size guardrail
min_free_disk_mb = 0 # 0 = disable disk guardrail
```

The matching `recovery_*` values MUST also be zeroed (validation rejects
mismatched pairs). **Use only on isolated forensic hosts** — without
guardrails the DB will grow without bound.

### Trigger a manual prune

There is no `cortex maintain prune` CLI command in V1. Retention runs
automatically on its hourly tick. The operator levers are:

- **Reduce `CORTEX_RETENTION_DAYS`** and restart — the next hourly
  tick will prune according to the new value.
- **Reduce `storage.max_db_size_mb`** — the storage guardrail task will
  evict oldest rows until the budget is satisfied within one 60-second tick.
- **AdGuard tag retention** is hardcoded at 7 days (see §6); there is no
  operator CLI to force an immediate AdGuard prune.

MCP exposes only `stats` (read-only) for the storage budget state — destructive
operations are not accessible from the MCP tool surface.

---

## 11. Anti-policies (deliberately NOT supported)

- **Per-host retention overrides.** All hosts share `retention_days`. No
  `retention_days = { host="jenny" days=7 }` config. Reason: the operator
  has not asked for it; adding per-host knobs explodes the validation
  surface and the cost-of-a-mistake (a typo silently keeping/deleting more
  than intended). If you need per-host retention, run two cortex
  instances or use tag-window retention.
- **Retention as a rule DSL.** No conditional retention based on log
  content, regex, or app_name (beyond the hardcoded AdGuard cap). The same
  rationale: validation surface and operational opacity.
- **Tiered retention within `logs`.** No "warm 7 d, cold 30 d, frozen S3"
  story. Single tier; single eviction policy per row class.
- **Soft delete / tombstones.** Eviction is hard DELETE + FTS merge. There
  is no recovery path post-eviction. Re-ingest from a higher-retention
  upstream if needed.
- **Backup-aware retention.** The retention task does not coordinate with
  backups. If backups run mid-prune, they capture a consistent point-in-time
  but possibly mid-prune-cycle snapshot — operators should run
  `cortex maintain prune` to a stable state before snapshotting if a
  bit-for-bit minimum is desired.
- **Per-tag retention beyond `adguard-*`.** The infrastructure for per-tag
  retention exists (`purge_by_tag_window`) but is hardcoded to AdGuard. We
  do NOT expose this as configurable in V1; see §7 in the open-questions
  filing for promotion to a `[storage].tag_retention]` map in V1.1.

---

## 12. Open questions filed against this contract

These are normative pointers to V1.1+ work, not open commitments for V1.

1. **Promote AdGuard 7-day to config.** `[storage].adguard_retention_days`
   (or generic `[storage].tag_retention.<tag>`). Current hardcode in
   `src/runtime.rs:58–59` works but is non-discoverable.
2. **`alert_state` 30-day GC knob.** Add `CORTEX_ALERT_STATE_GC_DAYS`
   to make Epic E's stale-clear configurable.
3. **`cortex backfill --since`.** Epic B punted to V1.1. Out of scope for
   this contract; cited for completeness.

---

## 13. Self-check

Every persistent table created by `current-schema.sql` or by any of the six
Epic spec migrations is covered in §3:

- `logs`, `logs_fts`, `hosts` — current schema
- `docker_ingest_checkpoints`, `poller_checkpoints` — current schema +
  Epic C
- `transcript_sources`, `transcript_import_records`,
  `transcript_parse_errors` — current schema
- `agents`, `host_metrics` — Epic A
- `host_heartbeats` and its six child tables (`heartbeat_cpu`,
  `heartbeat_memory`, `heartbeat_disks`, `heartbeat_network`,
  `heartbeat_processes`, `heartbeat_containers`) — current schema; the
  orphan-sweep cadence for the children is in §4.1
- `metrics_gauge`, `metrics_gauge_5m`, `metrics_gauge_1h`,
  `probe_results` — Epic D
- `alert_state` — Epic E
- `incidents` (+ Qdrant `incidents` + `/data/incidents/*.md`) —
  Epic F

If a future migration adds a persistent table, this contract MUST be
updated in the same patch.
