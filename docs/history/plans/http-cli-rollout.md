---
title: "cortex v0.26 rollout — HTTP CLI cutover"
created: 2026-05-18
updated: 2026-07-30
---

# cortex v0.26 rollout — HTTP CLI cutover

> **v0.26 BREAKING**: `CORTEX_API_ENABLED` was removed. The REST API at
> `/api/*` is now always-on and the container fails to start without a
> `CORTEX_API_TOKEN`. Run `cortex setup repair` BEFORE upgrading the
> container so the token is provisioned and `CORTEX_USE_HTTP=true` is
> written to `.env`. The CLI defaults to HTTP transport from v0.26
> onwards; to keep direct-DB behaviour, remove the line or set
> `CORTEX_USE_HTTP=false` before running `cortex`.

This document is the manual rollout playbook for upgrading a deployed
cortex host from a pre-v0.26 release. It assumes a single deploy
host running Docker Compose plus zero or more remote hosts running the
`cortex` CLI.

## Pre-deploy checklist

Run all five from the deploy host and capture their output before
touching the container. None of them mutate state.

```bash
# 1. Baseline DB shape — used to confirm post-deploy growth and verify
#    no schema migration re-applied.
sqlite3 ~/.cortex/data/cortex.db \
  'SELECT COUNT(*), MAX(id), (SELECT MAX(version) FROM schema_migrations) FROM logs'

# 2. Confirm the API token exists in .env. If missing, run
#    `cortex setup repair` BEFORE step 1 of "Deploy order".
grep CORTEX_API_TOKEN ~/.cortex/.env

# 3. Parity check: query the same data via local + HTTP and assert
#    the JSON shapes agree. Empty diff = safe to cut over.
cortex --json hosts | jq -S . > /tmp/syslog-local.json
CORTEX_USE_HTTP=1 cortex --json hosts | jq -S . > /tmp/syslog-http.json
diff /tmp/syslog-local.json /tmp/syslog-http.json && echo "parity OK"

# 4. sessions-watch daemon must be active + binary SHA recorded so we know
#    what we're replacing in step 4 of "Deploy order".
systemctl --user status syslog-sessions-watch
sha256sum ~/.local/bin/cortex

# 5. Compose diagnostics must be clean (0 issues). If non-zero,
#    resolve before deploying — `compose doctor` reports drift
#    between host bind mounts and the container.
cortex compose doctor --json | jq '.diagnostics | length'
```

## Deploy order

Order matters. Each step's failure mode is documented inline.

1. **`cortex setup repair`** — idempotent. Provisions
   `CORTEX_API_TOKEN` if missing and writes `CORTEX_USE_HTTP=true` if
   absent. Preserves any existing operator override (including
   `CORTEX_USE_HTTP=false`). Run BEFORE pulling the new image so the
   container has a token to start with.

2. **`cortex compose pull && cortex compose up`** — pull the v0.26
   image and recreate the container. The container fails fast if
   `CORTEX_API_TOKEN` is missing; step 1 prevents that. Wait until
   `cortex compose ps` reports `healthy` before proceeding.

3. **Install new CLI binary on the deploy host** —
   `cp ~/.cache/cargo/release/cortex ~/.local/bin/cortex` (or whatever
   path you use). Keep a backup at `~/.local/bin/cortex.backup` for
   the rollback section below.

4. **`systemctl --user restart syslog-sessions-watch`** — **CRITICAL**. The
   sessions-watch daemon is long-running; the process loaded into memory
   uses the binary that was on disk at the LAST `systemctl start`.
   Without this restart, the running daemon may diverge silently from
   the new server's expectations (schema, write-path semantics).

5. **Multi-host token propagation** — for every remote host that runs
   the `cortex` CLI, the new `CORTEX_API_TOKEN` must reach
   `~/.cortex/.env` (or wherever the host reads it). **DO NOT**
   run `export CORTEX_API_TOKEN=...` in an interactive shell — it
   leaks into shell history. Use one of:

   ```bash
   # File-based propagation over SSH (recommended)
   ssh remote-host "mkdir -p ~/.cortex && chmod 700 ~/.cortex"
   scp ~/.cortex/.env remote-host:~/.cortex/.env
   ssh remote-host "chmod 600 ~/.cortex/.env"
   ```

   For systemd-managed CLIs, use an `EnvironmentFile=` unit pointing
   at `~/.cortex/.env` rather than `Environment=CORTEX_API_TOKEN=...`
   (which is world-readable via `systemctl cat`).

## Post-deploy verification

Three verification windows. All checks are non-destructive.

### +5 minutes

```bash
# Container reports healthy, no recent errors in logs.
docker compose ps
cortex tail -n 10
docker compose logs cortex --since 5m | grep -E "500|ERROR|panic" | wc -l   # expect: 0
```

### +1 hour

```bash
# Total log count grew from the +0 baseline captured in pre-deploy step 1.
cortex stats
# CLI-to-API latency on a representative read.
time cortex tail -n 100 --json > /dev/null   # expect: < 0.2s on a warm cache
```

### +24 hours

```bash
# No migration re-apply lines in container logs.
docker compose logs cortex --since 24h | grep -i "applying migration" | wc -l   # expect: 0
# sessions-watch is still processing — checkpoint count grew.
cortex sessions checkpoints --json | jq '.checkpoints | length'
```

## Token rotation

Rotation forces every consumer to re-read the new token; the in-memory
old token must be dropped first.

```bash
# 1. Stop the container so the in-memory old token is released.
cortex compose down

# 2. Remove the existing line — keep your editor away from .env, the
#    sed is safer because it preserves every other key/value.
sed -i '/^CORTEX_API_TOKEN=/d' ~/.cortex/.env

# 3. Regenerate. setup repair writes a fresh 64-char hex token.
cortex setup repair

# 4. Restart the container with the new token.
cortex compose up

# 5. Propagate the new token to every remote host that runs the CLI.
#    Use the file-based pattern from "Multi-host token propagation"
#    above — never `export` an API token interactively.
```

## Rollback

There is no "disable API" rollback in v0.26 (the API is always-on).
Rollback consists of reverting the binary + clearing the HTTP default.

```bash
# 1. Restore the previous CLI binary.
cp ~/.local/bin/cortex.backup ~/.local/bin/cortex

# 2. Revert to direct-DB default. setup repair will NOT re-add the
#    line unless the key is fully absent, so deletion is sufficient.
sed -i '/^CORTEX_USE_HTTP=/d' ~/.cortex/.env

# 3. Restart the sessions-watch daemon so it picks up the restored binary.
systemctl --user restart syslog-sessions-watch
```

If you also need to roll back the container image, run
`cortex compose pull` against the previous tag and `cortex compose up`.
The schema is backwards-compatible across v0.25 → v0.26; no DB rollback
is required for a same-day revert.

## Notes

- **VACUUM on large databases**: `db vacuum --full` on a database
  larger than ~10 GB may exceed the 10-minute HTTP request timeout.
  Workaround: `CORTEX_USE_HTTP=false cortex db vacuum --full --force`
  to bypass the API and run VACUUM directly against the SQLite file.
  This is a known limitation tracked for the v0.27 successor.

- **`db backup` is local-only**: backup writes the file to a path on
  the host invoking the CLI, which the container has no way to
  service. Always invoke `db backup` in default mode
  (no `--http`, no `CORTEX_USE_HTTP=true`).

- **`ai watch`, `ai index`, `ai add`, `ai doctor`, `ai smoke-watch`,
  `ai watch-status`** — these read host filesystems or run host
  processes and remain local-only. The CLI bails with a descriptive
  error if `--http` is passed to any of them.
