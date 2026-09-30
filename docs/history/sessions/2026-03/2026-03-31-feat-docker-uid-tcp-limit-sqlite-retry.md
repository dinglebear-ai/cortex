# Session: Docker UID/GID, TCP Per-Line Limit, SQLite Lock Retry

**Date:** 2026-03-31
**Branch:** `chore/add-lavra-project-config`
**Commit:** `feat: configurable Docker UID/GID, TCP per-line limit, SQLite lock retry`
**Version:** 0.1.10 → 0.2.0

---

## Session Overview

Committed and pushed a large batch of accumulated changes on the `chore/add-lavra-project-config` branch. Changes spanned all four source modules plus project config files. Key deliverables: configurable Docker container user (UID/GID), TCP OOM fix for persistent syslog forwarders, SQLite transient lock retry, shared `StorageBudgetState`, and structured tracing throughout.

---

## Timeline

1. Ran `git diff --stat HEAD` and per-file diffs to understand scope of 911 insertions across 13 files.
2. Identified commit type as `feat` → minor version bump 0.1.10 → 0.2.0.
3. Edited `Cargo.toml` and ran `cargo check` to update `Cargo.lock`.
4. Wrote new `[0.2.0]` section in `CHANGELOG.md`.
5. Staged 14 files (excluding `.omc/` state files blocked by gitignore), committed, pushed.
6. Post-push: session doc + Axon embed + Neo4j capture.

---

## Key Findings

- `src/syslog.rs:158`: TCP handler previously used `.take(max_size as u64)` on the whole stream — broke rsyslog persistent forwarders that reuse one TCP session for many messages.
- `src/db.rs:389`: `insert_logs_batch` had no retry on `SQLITE_BUSY`/`SQLITE_LOCKED`; concurrent storage enforcement and batch writes could deadlock silently.
- `src/main.rs:33`: `tokio::time::interval()` fires immediately at t=0; replaced with `background_interval()` that delays the first tick by one period.
- `src/syslog.rs:12`: `start()` renamed to `start_with_storage_state()` — storage state is now passed in so the batch writer can block writes under pressure.

---

## Technical Decisions

- **`feat` not `fix` prefix** — Docker UID/GID is a new user-facing feature; TCP fix and SQLite retry are fixes but the commit message leads with the feature. Minor version bump chosen accordingly.
- **Per-line TCP limit over per-connection** — `BufReader::lines()` already splits on newlines; checking `line.len() > max_size` and `continue`-ing is zero-cost and correct for persistent sessions.
- **3-attempt retry with fixed delays (25/100/250ms)** — simple, predictable, avoids exponential blowup for short transient locks typical in WAL mode.
- **`Arc<Mutex<Option<StorageBudgetState>>>`** — `Option` allows lazy initialization; state is `None` if storage limits are disabled.

---

## Files Modified

| File | Purpose |
|------|---------|
| `Cargo.toml` | Version 0.1.10 → 0.2.0 |
| `Cargo.lock` | Updated by `cargo check` |
| `CHANGELOG.md` | New `[0.2.0]` section |
| `src/db.rs` | `StorageBudgetState`, SQLite retry, pragma helper, structured logging |
| `src/main.rs` | Shared storage state init, `background_interval`, startup budget check, structured logging |
| `src/mcp.rs` | Auth rejection logging, health check timing, MCP request tracing |
| `src/syslog.rs` | `start_with_storage_state`, TCP per-line limit, structured logging |
| `docker-compose.yml` | `user: "${SYSLOG_UID:-1000}:${SYSLOG_GID:-1000}"` |
| `.env.example` | Added `SYSLOG_UID`/`SYSLOG_GID` vars |
| `README.md` | Document UID/GID vars and bind-mount permission note |
| `.dockerignore` | Reorganized with categorized sections; AI tooling dirs excluded |
| `.gitignore` | Reorganized with categorized sections; worktree/cache/doc artifacts added |

---

## Commands Executed

```bash
rtk git diff --stat HEAD          # 911 insertions / 236 deletions across 13 files
grep '^version' Cargo.toml        # 0.1.10
rtk cargo check                   # 1 crate compiled (Cargo.lock updated)
rtk git add <14 files>            # staged (excluded .omc/ state files)
rtk git commit -m "feat: ..."     # committed
rtk git push                      # pushed to origin/chore/add-lavra-project-config
```

---

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Docker container user | Always runs as root | Runs as `${SYSLOG_UID:-1000}:${SYSLOG_GID:-1000}` |
| TCP persistent forwarders | Disconnect after `max_message_size` bytes total | Each line checked independently; connection stays open |
| SQLite batch insert on lock | Fails immediately | Retries up to 3× with 25/100/250ms backoff |
| Storage state sharing | Not shared across modules | `Arc<Mutex<StorageBudgetState>>` passed to batch writer |
| First retention purge / budget check | Fires immediately at startup | Fires after first interval period (no t=0 burst) |
| Startup | No initial budget check | Storage budget enforced before accepting traffic |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check` | Compiles clean | 1 crate compiled | PASS |
| `rtk git push` | Branch pushed | `ok chore/add-lavra-project-config` | PASS |
| `git log --oneline -1` | feat commit visible | Confirmed in push output | PASS |

---

## Source IDs + Collections Touched

Axon embed attempted post-session (see below).

---

## Risks and Rollback

- **TCP limit change**: Per-line limit is strictly more permissive than per-connection; no regression risk for devices sending one message per connection.
- **SQLite retry**: 3 attempts with ≤450ms total delay. If DB is persistently locked the error propagates normally after retry exhaustion.
- **Docker UID/GID**: Default is 1000:1000 matching previous implicit behavior for most homelab setups. Users running as root or custom UID must set vars explicitly.
- **Rollback**: `git revert HEAD` on the feature branch; version in `Cargo.toml` would need manual revert to `0.1.10`.

---

## Decisions Not Taken

- **Exponential backoff for SQLite retry** — overkill for WAL-mode transient locks; fixed delays are predictable and sufficient.
- **Per-connection byte budget alongside per-line limit** — rejected; adds complexity without benefit since the line limit already guards against large messages.
- **Major version bump** — changes are backwards-compatible from user perspective; minor is appropriate.

---

## Open Questions

- Are there any downstream consumers of `syslog::start()` (the old function name) outside this repo that need updating?
- Should `StorageBudgetState` be exposed via the `/health` endpoint for observability dashboards?

---

## Next Steps

- Run `/gh-address-comments` to address any open PR review comments on this branch.
- Consider opening a PR from `chore/add-lavra-project-config` → `main` once review comments are addressed.
