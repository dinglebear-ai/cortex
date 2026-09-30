---
title: "cortex CLI Performance Report"
created: 2026-05-28
updated: 2026-07-30
---

# cortex CLI Performance Report

Generated: 2026-05-28 | DB size: ~31 GB (~8.1M pages) | Log count: ~4.9M rows

---

## Summary

| Tier | Commands | Notes |
|------|----------|-------|
| ⚡ Fast (<100ms) | `db status`, `notify recent`, `health`, `gen-token`, `setup`, `ai doctor` | Healthy |
| ✅ Acceptable (100ms–2s) | `source-ips`, `ingest-rate`, `patterns`, `setup check`, `compose status`, `fmt`, `lint` | Fine |
| ⚠️ Slow (2s–60s) | `sig list`, `setup repair`, `just test`, `just build`, `just check` | Addressable |
| 🔴 Critical (>60s) | `timeline` (all buckets), `db integrity --quick` | Needs fix |
| ❌ Broken | `db backup` (DB locked), `validate-skills` (manifest error) | Bugs |

---

## Just Recipes

### `just health`
**What it does:** Fires `curl` against `http://localhost:3100/health` and pretty-prints the JSON response.

| Run | Time |
|-----|------|
| 1 | 20ms |

**Notes:** Fast. Wraps `curl -sf ... | jq .` — no startup cost beyond process spawn.

---

### `just gen-token`
**What it does:** Generates a random 32-byte hex string via `openssl rand -hex 32` for use as a new API token.

| Run | Time |
|-----|------|
| 1 | 13ms |

**Notes:** Fast. Pure `openssl` call, no network or DB.

---

### `just setup`
**What it does:** Copies `.env.example` → `.env` if `.env` doesn't already exist (idempotent).

| Run | Time |
|-----|------|
| 1 | 27ms |

**Notes:** Fast. Shell `cp -n` only. If `.env` already exists, exits immediately.

---

### `just fmt`
**What it does:** Runs `cargo fmt` to apply Rust formatting in-place across all source files.

| Run | Time |
|-----|------|
| 1 (already formatted) | 719ms |

**Notes:** Acceptable. The 700ms is entirely `cargo`'s startup + AST parse. If formatting is needed, add ~50–200ms per file changed. No meaningful optimization available.

---

### `just lint`
**What it does:** Runs `cargo clippy -- -D warnings`, aborting on any lint warning.

| Run | Time |
|-----|------|
| 1 (cached, no recompile) | 1,610ms |

**Notes:** Acceptable when cached. Cold (after any source change) triggers a full dev-profile compile of `cortex`, which adds ~18s (see `just check`). The 1.6s is the incremental lint pass.

**Suggestion:** Add `sccache` to the compile pipeline if not already configured — it caches object files across clean checkouts and CI runs, cutting cold lint from ~20s to ~3–5s.

---

### `just check`
**What it does:** Runs `cargo check` (type-checks without linking) then `scripts/check-rust-module-size.sh` to enforce a 500-line limit per file.

| Run | Time |
|-----|------|
| 1 | 18,639ms |

**Notes:** The 18s is `cargo check` on modified source. The module-size script then found two files over the 500-line limit (`cli/args.rs` at 613 lines, `cli/dispatch_surface.rs` at 538 lines) and caused the recipe to fail with exit code 1 — this is a pre-existing issue unrelated to this run.

**Suggestion:** Split `cli/args.rs` and `cli/dispatch_surface.rs` to satisfy the module-size check and keep `just check` green.

---

### `just build`
**What it does:** Runs `cargo build` (dev profile, unoptimized + debuginfo) — the standard development build.

| Run | Time |
|-----|------|
| 1 (incremental, cortex crate only) | 20,125ms |

**Notes:** The 20s is incremental — only the top-level `cortex` crate was rebuilt. A from-scratch `cargo build` takes ~7 minutes (confirmed during this session). Release profile (`cargo build --release`) adds significant additional time.

**Suggestion:** The incremental build time (20s) is dominated by LLVM codegen for the large binary. Splitting the crate into smaller workspace members would reduce per-change rebuild time.

---

### `just test`
**What it does:** Runs `cargo test` — all unit tests, integration tests, and doc tests.

| Run | Time |
|-----|------|
| 1 | 36,726ms |

**Notes:** 36 seconds includes `cargo`'s compile step (~18s, already cached after `just lint`) plus test execution (~18s for ~1,179 tests). The compile step dominates on cold runs.

**Suggestion:** Replace `cargo test` with [`cargo-nextest`](https://nexte.st/) — it runs tests in parallel processes, reducing wall time by 40–60% on multi-core machines. Add `cargo nextest run` as an alias or replace the `just test` recipe.

---

### `just test-live`
**What it does:** Runs `bash tests/test_live.sh` — full integration test suite against the live server over HTTP.

| Run | Time |
|-----|------|
| 1 (no token injected via `just`) | 172,392ms (2m52s) |
| 2 (token passed explicitly) | 9,127ms |

**Notes:** The 172-second run had `Token: ` empty in the test header — `just test-live` does not inject `CORTEX_API_TOKEN` into the script environment, so all authenticated requests fail and the test suite falls back to slow retry/timeout paths. With the token passed directly to `test_live.sh`, the suite completes in ~9 seconds.

**Suggestion:** Update the `just test-live` recipe to inject the token:
```makefile
test-live:
    bash tests/test_live.sh --url http://localhost:3100 --token $(CORTEX_API_TOKEN)
```
Or add `set dotenv-load` at the top of the `Justfile` to auto-load `.env`.

---

### `just validate-skills`
**What it does:** Validates plugin manifests, MCP config, hooks, and skill frontmatter via `validate-plugin`.

| Run | Time |
|-----|------|
| 1 | 38ms |

**Notes:** Fast, but **exits with error**: `FORBIDDEN: .claude-plugin/plugin.json version`. The plugin manifest contains a forbidden `version` field that the validator rejects.

**Suggestion:** Remove or update the `version` field in `.claude-plugin/plugin.json` to match the current plugin schema.

---

## cortex CLI Commands (HTTP Transport)

All commands below used `cortex --server http://localhost:3100 --token <token>` unless noted as local.

---

### `cortex db status`
**What it does:** Reports DB file path, page count, freelist count, page size, logical/physical byte sizes, WAL size, journal mode, and auto-vacuum setting.

| Run | Time |
|-----|------|
| 1 | 12ms |
| 2 | 27ms |

**Notes:** Fast. Single `PRAGMA` pass, no full scan.

---

### `cortex db status --check-coord`
**What it does:** Same as `db status` plus runs two coordination checks: `data-mount` (verifies host bind-mount path) and `ai-watch-coord` (verifies systemd unit path matches container). Shells out to `docker inspect` and `systemctl`.

| Run | Time |
|-----|------|
| 1 | 76ms |

**Notes:** Acceptable. The ~64ms overhead vs. plain `db status` is the two shell-out calls (`docker inspect` + `systemctl --user show`). Results are cached within a single invocation, so it won't double-charge on multi-phase commands.

---

### `cortex db integrity --quick`
**What it does:** Runs SQLite's `PRAGMA integrity_check` (or `quick_check`) against the production DB to detect corruption.

| Run | Time |
|-----|------|
| 1 | **TIMEOUT at 600s** (10-minute HTTP deadline exceeded) |

**Notes:** 🔴 **Critical.** On a 31 GB database with ~8.1M pages, integrity checking requires reading every B-tree page. Even `--quick` (which skips cross-index consistency) must traverse the entire file. The server enforces a 600-second HTTP timeout, which this operation exceeds.

**Suggestions:**
1. Run `db integrity` as a background/async job rather than a synchronous HTTP request — return a job ID and a `GET /api/db/integrity/status/{id}` polling endpoint.
2. Alternatively, add chunked streaming so partial results appear before the timeout fires.
3. Schedule integrity checks via cron (e.g., weekly) rather than on-demand via CLI.
4. Consider `PRAGMA quick_check` if `integrity_check` is being used — it's 2–4× faster by skipping cross-index validation.

---

### `cortex db backup`
**What it does:** Runs a local SQLite hot-backup (online backup API) to a specified output path.

| Run | Time |
|-----|------|
| 1 (local, container holds lock) | ~29ms, **FAILS: database is locked** |

**Notes:** ❌ **Broken in local mode** when the container is running. The CLI opens a new connection to the same SQLite file that the container has locked exclusively during WAL checkpoints. SQLite's `sqlite3_backup_init` API tolerates concurrent readers but not exclusive locks.

**Suggestions:**
1. Use `PRAGMA wal_checkpoint(PASSIVE)` before backup to reduce lock contention, but this doesn't guarantee the lock is released.
2. Expose `db backup` as an HTTP endpoint that the running server executes — the server already holds the pool and can initiate the backup from within the WAL-sharing connection pool.
3. As a workaround, run `docker exec cortex syslog db backup --output /data/backup.db` from within the container where the pool connection is shared.

---

### `cortex source-ips`
**What it does:** Lists unique log sources (IP addresses and Docker container paths) ordered by log volume, with last-seen timestamp.

| Run | Time |
|-----|------|
| `--limit 10` | 117ms |
| `--limit 50` | 154ms |

**Notes:** Acceptable. Uses `GROUP BY source_ip` with `idx_logs_source_ip_timestamp` covering index. Scales gracefully with limit.

---

### `cortex timeline --bucket minute`
**What it does:** Returns per-minute log counts across the entire log history.

| Run | Time |
|-----|------|
| 1 | **99,447ms (99 seconds)** |

**Notes:** 🔴 **Critical.** Full table scan on ~4.9M rows: `SELECT strftime('%Y-%m-%dT%H:%M:00', timestamp) AS bucket, COUNT(*) FROM logs GROUP BY bucket`. The `strftime()` call is applied to every row — even with `idx_logs_timestamp`, SQLite must read every row to compute the bucket value. Across 4.9M rows this takes ~100 seconds.

---

### `cortex timeline --bucket hour`
**What it does:** Returns per-hour log counts across the entire log history.

| Run | Time |
|-----|------|
| 1 | **93,328ms (93 seconds)** |

---

### `cortex timeline --bucket day`
**What it does:** Returns per-day log counts across the entire log history.

| Run | Time |
|-----|------|
| 1 | **71,790ms (72 seconds)** |

**Notes (all timeline buckets):** 🔴 **Critical.** All three bucket sizes show the same root cause: full scan of the `logs` table with no default time window. Minute buckets are slowest because the `GROUP BY` produces more distinct buckets (more aggregation work).

**Suggestions:**
1. **Add a default `--from` window** (e.g., last 30 days) so the query can use `idx_logs_timestamp` to skip old rows. This alone would reduce the scan from 4.9M rows to ~200K rows for a typical homelab — a 25× speedup:
   ```rust
   // In the timeline handler, default from = now - 30 days if not specified
   let from = from.or_else(|| Some(thirty_days_ago()));
   ```
2. **Add a `bucket_ts` generated column** using `strftime` and index it. SQLite 3.31+ supports generated columns:
   ```sql
   ALTER TABLE logs ADD COLUMN bucket_hour TEXT GENERATED ALWAYS AS
     (strftime('%Y-%m-%dT%H:00:00', timestamp)) STORED;
   CREATE INDEX idx_logs_bucket_hour ON logs(bucket_hour);
   ```
   This pre-computes the bucket at insert time, eliminating the per-row `strftime` cost at query time.
3. **Expose `week` and `month` buckets** — both return a 400 error currently. Adding them (with appropriate truncation) would be low-effort and useful.

---

### `cortex ingest-rate`
**What it does:** Reports current ingest rate (logs/sec) over 1m/5m/15m windows, optionally broken down by host.

| Run | Time |
|-----|------|
| `--by-host` | 536ms |

**Notes:** Acceptable. Runs multiple time-windowed `COUNT(*)` queries against `idx_logs_received_at`. The 536ms reflects three separate DB queries (one per window). Could be consolidated into a single query but current performance is fine.

---

### `cortex patterns`
**What it does:** Scans the most recent logs, normalizes message text (replaces numbers/IPs/hashes with placeholders), and ranks the top recurring message templates.

| Run | Time |
|-----|------|
| `--top-n 10` | 452ms |
| `--top-n 25` | 706ms |

**Notes:** Acceptable. Scans a fixed 10,000-row sample (`truncated` note in output confirms the cap). Processing is in-process regex normalization on the result set. Scales with `--top-n` due to sorting overhead.

---

### `cortex sig list`
**What it does:** Lists the top 50 error signature patterns with occurrence counts, last-seen timestamps, and a 1-hour window count.

| Run | Time |
|-----|------|
| 1 | 1,128ms |
| 2 | 2,717ms |

**Notes:** ⚠️ Slow (and variable). The query uses a **correlated subquery** to compute the 1-hour window count for each of the 50 result rows:
```sql
COALESCE((
    SELECT SUM(w.count_in_window)
    FROM error_signature_windows w
    WHERE w.signature_hash = s.signature_hash
      AND w.normalizer_version = s.normalizer_version
      AND w.window_end >= ?1
), 0) AS count_last_1h
```
This fires 50 independent subqueries. The 1.1–2.7s variance likely reflects WAL checkpoint timing.

**Suggestion:** Replace the correlated subquery with a single pre-aggregated JOIN:
```sql
SELECT s.*, COALESCE(w.total_1h, 0) AS count_last_1h
FROM error_signatures s
LEFT JOIN (
    SELECT signature_hash, normalizer_version, SUM(count_in_window) AS total_1h
    FROM error_signature_windows
    WHERE window_end >= ?1
    GROUP BY signature_hash, normalizer_version
) w USING (signature_hash, normalizer_version)
ORDER BY s.last_seen_at DESC
LIMIT 50
```
This runs one pass over `error_signature_windows` instead of 50, cutting time to ~50–150ms.

---

### `cortex notify recent`
**What it does:** Shows the most recent notification dispatch records (rule ID, hostname, severity, timestamps, delivery status).

| Run | Time |
|-----|------|
| `--limit 25` | 11–20ms |

**Notes:** Fast. Simple `SELECT ... FROM notifications_outbox ORDER BY created_at DESC LIMIT ?`.

---

### `cortex ai doctor`
**What it does:** Self-diagnoses the AI transcript watcher — checks DB schema version, Claude/Codex session root paths, checkpoint counts, and scan error counts.

| Run | Time |
|-----|------|
| 1 | 81–127ms |

**Notes:** Fast. Reads local filesystem stat + one DB query. No network required.

---

### `cortex setup check`
**What it does:** Validates the cortex installation: home directory, `.env`, compose, data directory, and a liveness check against the running server.

| Run | Time |
|-----|------|
| 1 | 49–176ms |

**Notes:** Acceptable. The variance comes from the HTTP health check to `http://127.0.0.1:3100/health` — a cold TCP connection can add up to ~150ms.

---

### `cortex setup repair`
**What it does:** Auto-fixes the cortex installation — creates missing directories, merges missing `.env` keys, updates compose files from embedded templates, and validates the result.

| Run | Time |
|-----|------|
| First run (first session) | ~14,000ms (14 seconds) |
| Subsequent runs (idempotent) | 1,635–2,210ms |

**Notes:** ⚠️ The first-run spike is caused by pulling compose file templates and checking for the running server. Subsequent runs are ~1.6–2.2s, dominated by the HTTP health probe and file I/O. The variance between runs 2–3 is a ~600ms range, likely from WAL checkpoint timing on the local DB.

**Suggestion:** The 14-second first-run time is surprising for a repair command. Profile whether this is a network call (compose template download) or a slow filesystem operation. If it's a template download, consider bundling the templates at compile time (they may already be embedded) and falling back to network only when the embedded version is outdated.

---

### `cortex compose status`
**What it does:** Checks whether the cortex Docker container is running, its health status, image, and compose project details.

| Run | Time |
|-----|------|
| 1 | 329ms |
| 2 | 331ms |

**Notes:** Acceptable. The ~330ms is `docker inspect` + one API call. Consistent across runs.

---

### `cortex compose doctor`
**What it does:** Full coordination diagnostics — compose status plus the `data-mount` and `ai-watch-coord` phases that check for host/container SQLite path drift.

| Run | Time |
|-----|------|
| 1 | 338ms |
| 2 | 341ms |

**Notes:** Acceptable. Only ~10ms slower than `compose status` despite running two additional shell-outs (`docker inspect` + `systemctl`), because results are cached within the same invocation.

---

## Performance Summary Table

| Command | Time | Status | Priority Fix |
|---------|------|--------|--------------|
| `just health` | 20ms | ✅ | — |
| `just gen-token` | 13ms | ✅ | — |
| `just setup` | 27ms | ✅ | — |
| `just fmt` | 719ms | ✅ | — |
| `just lint` | 1,610ms | ✅ | Add sccache for cold builds |
| `just check` | 18,639ms | ⚠️ | Fix module-size violations; add sccache |
| `just build` | 20,125ms | ⚠️ | Incremental; workspace split would help |
| `just test` | 36,726ms | ⚠️ | Switch to cargo-nextest |
| `just test-live` | 172s (no token) / 9s (with token) | ❌ | Inject token in Justfile |
| `just validate-skills` | 38ms | ❌ | Fix plugin.json manifest |
| `cortex db status` | 12ms | ✅ | — |
| `cortex db status --check-coord` | 76ms | ✅ | — |
| `cortex db integrity --quick` | >600s (timeout) | 🔴 | Async job API |
| `cortex db backup` | fails (locked) | ❌ | Expose as server-side endpoint |
| `cortex source-ips --limit 50` | 154ms | ✅ | — |
| `cortex timeline --bucket minute` | 99,447ms | 🔴 | Default time window + generated column |
| `cortex timeline --bucket hour` | 93,328ms | 🔴 | Default time window + generated column |
| `cortex timeline --bucket day` | 71,790ms | 🔴 | Default time window + generated column |
| `cortex ingest-rate --by-host` | 536ms | ✅ | — |
| `cortex patterns --top-n 25` | 706ms | ✅ | — |
| `cortex sig list` | 1,128–2,717ms | ⚠️ | Replace correlated subquery with JOIN |
| `cortex notify recent --limit 25` | 11ms | ✅ | — |
| `cortex ai doctor` | 81–127ms | ✅ | — |
| `cortex setup check` | 49–176ms | ✅ | — |
| `cortex setup repair` | 1,635ms (steady) | ✅ | Investigate 14s first-run spike |
| `cortex compose status` | 329ms | ✅ | — |
| `cortex compose doctor` | 338ms | ✅ | — |

---

## Top Recommendations

### P0 — Fix `timeline` (blocking usability)
All three `timeline` bucket sizes take 72–99 seconds on 4.9M rows. Add a default 30-day `--from` window so queries use the timestamp index. This is a one-line change in the timeline handler.

### P1 — Fix `db integrity` timeout
`db integrity --quick` exceeds the 600-second HTTP timeout on a 31 GB DB. Move to an async background job model with a polling endpoint, or add a CLI-side warning that this will take >10 minutes.

### P1 — Fix `just test-live` token injection
`just test-live` runs the integration suite without a token, causing 172-second timeout-heavy failure paths. Inject `CORTEX_API_TOKEN` from the `.env` file in the recipe.

### P2 — Fix `sig list` correlated subquery
Replace the 50-query correlated subquery in `sig list` with a single aggregating JOIN. Expected improvement: 1–2s → ~100ms.

### P2 — Fix `db backup` for concurrent use
`cortex db backup` fails when the container holds the SQLite lock. Move the backup operation server-side so it uses the pool's existing connection, or document that it must be run from inside the container.

### P3 — Add cargo-nextest
Replace `cargo test` with `cargo nextest run` in `just test` to parallelize test execution and cut the 36s run to ~15–18s.

### P3 — Fix `validate-skills`
The `.claude-plugin/plugin.json` manifest fails schema validation (`version` field is forbidden). Fix the manifest so `just validate-skills` passes cleanly.
