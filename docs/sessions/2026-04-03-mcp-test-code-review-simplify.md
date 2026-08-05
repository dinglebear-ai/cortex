# Session: MCP Tool Test, Code Review, and Simplify

**Date:** 2026-04-03
**Branch:** `main`
**Repo:** `syslog-mcp`

---

## Session Overview

End-to-end quality pass on the syslog-mcp codebase: live smoke-tested all six MCP tools against the production server, ran a full beagle-rust code review (with tokio/axum/serde skill loading), dispatched a `rust-pro` agent to fix all identified issues, then ran the `/simplify` skill to catch secondary quality concerns. Net result: 6 bugs fixed, 3 additional simplifications applied, all 70 tests passing, clippy clean.

---

## Timeline

| Time | Activity |
|------|----------|
| Start | OAuth auth flow for `syslog.example.internal` MCP endpoint |
| +5 min | Live tool test: all 6 MCP tools exercised against production DB |
| +15 min | `/beagle-rust:review-rust` — loaded 4 skills, reviewed 5 source files |
| +35 min | `rust-pro` agent dispatched to fix all 6 review findings |
| +45 min | `/simplify` — 3 parallel agents found 3 additional issues |
| +55 min | Applied simplify fixes, verified 70 tests pass, clippy clean |

---

## Key Findings (Code Review)

### Critical
- **`src/mcp.rs:791`** — `&raw[..limit]` slices a `String` at an arbitrary byte offset. Multi-byte UTF-8 (non-ASCII syslog messages) causes a panic with "byte index N is not a char boundary". Triggered on every MCP request with non-ASCII tool arguments.

### Major
- **`src/db.rs:344`** — `delete_oldest_logs_chunk(pool, 1)` — chunk size of 1 row per enforcement cycle. Each iteration runs a WAL checkpoint + incremental vacuum + host reconciliation (5–10 queries). For large overages, storage recovery would loop millions of times.
- **`src/config.rs:201`** — `impl Default for Config` derivable; clippy `-D warnings` error blocking CI.
- **`src/db.rs:471`** — `is_transient_sqlite_lock` nested `match` where `matches!` suffices; clippy error.
- **`src/syslog.rs:170`** — `let close_reason; close_reason = loop {…}` unneeded late init; clippy error.
- **`src/syslog.rs:340`** — `batch.len() > 0` should be `!batch.is_empty()`; clippy error.

### Simplify Findings (post-review)
- **Char-boundary walk-back duplicated** — `summarize_json_value` in `mcp.rs` reinvented the same loop already in `syslog.rs:truncate()`. Cannot share directly (private fn, different module) but the duplication was noted; extraction to `util.rs` is a future improvement.
- **`cleanup_chunk_size == 0` not validated** — passing 0 would cause the enforcement loop to spin forever deleting 0 rows.
- **Three identical test helpers** — `test_storage_config()` was copy-pasted across `db.rs`, `mcp.rs`, `syslog.rs`. Any new `StorageConfig` field required updating all three.

---

## Technical Decisions

1. **`cleanup_chunk_size` made configurable** — Rather than hardcoding 2000, the agent added `cleanup_chunk_size: usize` to `StorageConfig` (default 2000, env `SYSLOG_MCP_CLEANUP_CHUNK_SIZE`). Test helpers use `cleanup_chunk_size: 1` for fine-grained per-row assertions.

2. **WAL checkpoint moved outside enforcement loop** — Previously ran once per deleted chunk (even with chunk_size=1). Now runs once after the entire recovery loop exits. Equivalent correctness, drastically fewer PRAGMA round-trips during bulk recovery.

3. **`floor_char_boundary` rejected (MSRV)** — The efficiency agent suggested `str::floor_char_boundary` (cleaner). Clippy blocked it: stable since Rust 1.91, but project MSRV is 1.86. Kept the manual walk-back loop.

4. **`db.rs` test helper not consolidated** — `db.rs` uses `min_free_disk_mb: 512` (disk enforcement enabled in tests); `mcp.rs` and `syslog.rs` use `0` (disabled). Legitimately different. Only the two identical helpers were consolidated into `StorageConfig::for_test()`.

---

## Files Modified

| File | Change |
|------|--------|
| `src/mcp.rs` | UTF-8 safe truncation in `summarize_json_value`; new multibyte test; `test_storage_config` delegates to `StorageConfig::for_test()` |
| `src/db.rs` | `delete_oldest_logs_chunk` uses `config.cleanup_chunk_size`; WAL checkpoint moved after loop; `is_transient_sqlite_lock` simplified to `matches!`; test helpers updated with new field |
| `src/config.rs` | `#[derive(Default)]` on `Config`; `cleanup_chunk_size` field added to `StorageConfig`; `default_cleanup_chunk_size()` fn; env var parsing; `cleanup_chunk_size == 0` validation; `#[cfg(test)] StorageConfig::for_test()` |
| `src/syslog.rs` | `let close_reason = loop {…}` (late init fix); `!batch.is_empty()`; `test_storage_config` delegates to `StorageConfig::for_test()` |

---

## Commands Executed

```bash
# Live tool test
curl -s https://syslog.example.internal/health
# → {"status":"ok"}

# MCP tools via OAuth-authenticated session
mcp__syslog-mcp__get_stats      → 4.9M logs, 22 hosts, 2.6GB DB, write_blocked=false
mcp__syslog-mcp__list_hosts     → 6 active hosts (devhost dominant at 4.4M logs)
mcp__syslog-mcp__tail_logs n=5  → live tailscale + kernel AppArmor entries
mcp__syslog-mcp__get_errors     → WINHOST 12 alerts, devhost 1906 warnings
mcp__syslog-mcp__search_logs query=error limit=3  → WSL relay errors on WINHOST
mcp__syslog-mcp__list_hosts     → all hosts with timestamps

# Verification after fixes
rtk cargo clippy --all-targets --all-features -- -D warnings
# → No issues found

rtk cargo test
# → 70 passed (1 suite, 0.31s)
```

---

## Behavior Changes (Before / After)

| Area | Before | After |
|------|--------|-------|
| Non-ASCII MCP requests | Panic (`byte index N is not a char boundary`) → 500 response | Safe truncation at char boundary |
| Storage enforcement (large overage) | 1 row/iteration × WAL checkpoint × vacuum = potentially millions of operations | 2000 rows/iteration, single WAL checkpoint after loop |
| `cleanup_chunk_size=0` | Silent infinite loop in enforcement | Config validation rejects it at startup |
| `cargo test` / CI | 4 clippy errors blocked compilation | All errors resolved, tests pass |
| Test helper duplication | 3 independent `test_storage_config()` fns diverge on every new field | `mcp.rs`+`syslog.rs` delegate to `StorageConfig::for_test()` in config |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo clippy --all-targets --all-features -- -D warnings` | No errors | No issues found | ✅ |
| `cargo test` | 70 passed | 70 passed (0.31s) | ✅ |
| `curl https://syslog.example.internal/health` | `{"status":"ok"}` | `{"status":"ok"}` | ✅ |
| `mcp__syslog-mcp__get_stats` | DB stats returned | 4.9M logs, 22 hosts | ✅ |
| `mcp__syslog-mcp__tail_logs` | 5 recent entries | 5 entries returned | ✅ |
| `mcp__syslog-mcp__search_logs query=error` | FTS5 results | 3 results returned | ✅ |

---

## Source IDs + Collections Touched

N/A — No Axon embed/retrieve or vector search was performed during this session.

---

## Risks and Rollback

- **Storage enforcement chunk change (2000 rows)**: Larger chunks hold the write lock longer per iteration. At ~300 bytes/row, 2000 rows ≈ 600KB deleted per chunk. WAL lock duration remains in the tens-of-milliseconds range — acceptable. Rollback: set `SYSLOG_MCP_CLEANUP_CHUNK_SIZE=1` in env.
- **WAL checkpoint moved outside loop**: During a long recovery loop, the WAL may grow before the final checkpoint. This is bounded by the enforcement cycle (default 60s) and does not affect correctness. Rollback: move `checkpoint_wal_and_incremental_vacuum` back inside the while loop body.
- **`#[derive(Default)]` on Config**: Behaviorally identical to the removed manual impl. No risk.

---

## Decisions Not Taken

- **`str::floor_char_boundary`** — Cleaner than the manual walk-back loop, but blocked by MSRV 1.86 (requires 1.91). Would eliminate 3 lines of code; defer until MSRV is bumped.
- **Extract `truncate()` to `src/util.rs`** — Would eliminate the code duplication between `mcp.rs` and `syslog.rs`. Not done because it requires creating a new module and updating both import sites; the duplication is 3 lines and low-risk.
- **Consolidate `db.rs` test helper** — `db.rs` tests use `min_free_disk_mb: 512` (disk enforcement enabled); the shared `for_test()` uses 0. Consolidating would silently change test behavior for enforcement tests that don't explicitly override this field.

---

## Open Questions

- **Timestamp-as-hostname artifacts**: `list_hosts` returns entries like `"2026-03-29T02:45:44.291Z"` as hostnames (from early testing). These have `log_count: 1` each and are harmless, but they pollute the hosts table. No cleanup mechanism exists.
- **`WINHOST` 12 alerts**: From today's `get_errors` run — 12 alert-level events on WINHOST. Source is WSL relay errors. Worth investigating if they are recurring.
- **`devhost` AppArmor denials**: Continuous `snap.tailscale.tailscaled` ptrace denials in the live tail. Volume is high (many per second). Not a new finding but worth monitoring.

---

## Next Steps

1. Consider bumping MSRV to 1.91 to enable `str::floor_char_boundary` and other modern std APIs.
2. Investigate the 12 alert-level events on `WINHOST` (WSL relay).
3. Add a cleanup job or TTL for the timestamp-as-hostname test artifacts in the `hosts` table.
4. Consider extracting `truncate()` to a shared `src/util.rs` when the next cross-module utility is needed.
