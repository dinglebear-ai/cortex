> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

# Homelab Log Analysis Report
**Period:** 2026-05-07 04:00 UTC → 2026-05-08 03:57 UTC (24 hours)
**Generated:** 2026-05-08T04:00 UTC
**Data source:** cortex (918,919 total logs across 9 hosts)

---

## Executive Summary

The fleet is mostly healthy but has **3 active issues requiring immediate attention** and several lower-priority recurring failures. The most severe event was a repeated OOM killer on `devhost` killing the `codex-acp` process (consuming 26–34 GB RAM each time), with at least **8 kill events** in the past 24 hours. `nashost`'s Radarr container is in a **persistent permission-denied crash loop** that was still firing at report time. The `axon-qdrant` container on `198.51.100.1` experienced a sustained **5-second panic loop** for ~20 minutes mid-afternoon before recovering.

| Severity | Issue | Host | Status |
|----------|-------|------|--------|
| 🔴 CRITICAL | OOM killer repeatedly killing `codex-acp` (26–34 GB) | devhost | Active |
| 🔴 CRITICAL | Radarr `UnauthorizedAccessException` every 90s on movie file | nashost | Active — ongoing |
| 🔴 CRITICAL | `axon-qdrant` panic crash loop every 5s for ~20 min | 198.51.100.1 | Resolved (~19:07 UTC) |
| 🟠 HIGH | tracearr Plex 401 Unauthorized — API token invalid | nashost | Active |
| 🟠 HIGH | `wsl-pro.service` crash loop (restart counter at 162) | WINHOST | Active |
| 🟡 MEDIUM | High volume of nginx unauthorized hits (33K warnings) | edgehost | Monitored by fail2ban |
| 🟡 MEDIUM | `sccache.service` restarting repeatedly (counter at 13) | devhost | Active |
| 🟢 LOW | WSL clock sync issues (`Time jumped backwards` 95x) | laptophost | Intermittent |
| 🟢 LOW | WSL DNS failures (`getaddrinfo failed: -3`) | laptophost | Intermittent |
| 🟢 INFO | Snap service failures (firmware-updater, portal, prompting-client) | devhost, edgehost | Benign/known |

---

## Fleet Overview

| Host | Logs (24h) | Warnings | Errors | Last Seen |
|------|-----------|----------|--------|-----------|
| devhost | ~418,180 | 4,214 | 129 | 03:56 UTC |
| laptophost | ~239,957 | 34 | 38 | 03:56 UTC |
| edgehost | ~96,002 | **33,493** | 10 | 03:56 UTC |
| nashost | ~84,258 | 5,448 | 0 | 03:56 UTC |
| 198.51.100.1 | ~73,941 | **13,327** | 0 | 03:56 UTC |
| WINHOST | 4,472 | 16 | 0 | 03:39 UTC |
| backuphost | 1,351 | 0 | 0 | 03:56 UTC |
| BACKUPHOST | 5,039 | 1 | 1 | Last seen 13:00 UTC (stopped reporting) |
| smoke-test-host | 4 | 1 | 2 | One-off test only |

**DB stats:** 900 MB logical / 1,320 MB physical, 462 GB free disk, 0 phantom FTS rows, write-block: false.

> **Note:** `BACKUPHOST` and `backuphost` are the same host — hostname casing changed around 13:00 UTC yesterday when the host reconnected.

---

## Critical Issues

### 1. 🔴 devhost — Repeated OOM Kills of `codex-acp`

**Status:** Active — multiple events in the past 6 hours, most recent at 03:00 UTC.

The Linux OOM killer has repeatedly terminated `codex-acp` across multiple sessions throughout the day. At kill time, the process is consuming an extraordinary amount of memory:

| Kill Event (UTC) | PID | Anon RSS | Virtual Memory |
|-----------------|-----|----------|----------------|
| ~22:47 | 119901 | 26.5 GB | 33.2 GB |
| ~02:29 | 128556 | 28.2 GB | 36.2 GB |
| ~02:45 | 3441520 | 34.0 GB | 39.0 GB |

The OOM was triggered by `zed-remote-serv` (the Zed editor remote server) and in some cases by Rust compiler codegen units (`opt cgu.0`), which pushes the system over the memory limit. The kernel calls `out_of_memory()` → `oom_kill_process()`, and systemd then notifies that the process in `session-*.scope`, `user.slice`, `user@1000.service`, `app.slice`, and `-.slice` was killed.

**Representative log sequence:**
```
kernel:  zed-remote-serv invoked oom-killer: gfp_mask=0x140cca, order=0, oom_score_adj=0
kernel:  oom_kill_process.cold+0x8/0xac
kernel:  oom-kill: task=codex-acp, pid=3441520, uid=1000
kernel:  Out of memory: Killed process 3441520 (codex-acp) total-vm:39003532kB, anon-rss:34039092kB
systemd: session-139.scope: A process of this unit has been killed by the OOM killer.
systemd: app.slice: A process of this unit has been killed by the OOM killer.
```

**Root cause:** `codex-acp` (likely an AI agent backend) is allocating 26–34 GB of anonymous RSS. When Rust builds (via sccache) or Zed remote-server activity peaks concurrently, the system exhausts physical memory. No swap is configured or is insufficient.

**Impact:** `sccache.service` is also restarting frequently (counter at 13 by ~02:30 UTC), likely because OOM events kill it or its parent processes. The Zed remote server invocation correlates directly with the OOM trigger.

**Recommended actions:**
- Set `oom_score_adj` to a higher value for `codex-acp` so it's preferentially killed before system-critical processes
- Add a swap file/partition (16–32 GB recommended given memory pressure)
- Consider adding a cgroup memory limit for `codex-acp` to cap its maximum allocation
- Investigate why `codex-acp` is allocating 34 GB of anon-RSS — this is likely a memory leak or unbounded context/cache growth

---

### 2. 🔴 nashost — Radarr Permission Denied (Ongoing Crash Loop)

**Status:** Active — firing every ~90 seconds, still ongoing at 03:57 UTC.

Radarr v6.2.0.10409 is repeatedly throwing an `UnauthorizedAccessException` when attempting to access a specific movie file:

```
[v6.2.0.10409] System.UnauthorizedAccessException: Access to the path
'/data/media/movies/Ready or Not 2 Here I Come (2026)/
Ready or Not 2 Here I Come (2026) {tmdb-1266127} {edition-Trailer}.mp4' is denied.
 ---> System.IO.IOException: Permission denied
```

This has been looping since at least 03:00 UTC (and likely much longer based on the log volume). Each error pair fires at approximately 90-second intervals — consistent with Radarr's retry/scan scheduler.

**Root cause:** The file `/data/media/movies/Ready or Not 2 Here I Come (2026)/...` has incorrect ownership or permissions relative to the UID/GID that the `radarr` container runs as. The `{edition-Trailer}` tag suggests this is a metadata file that Radarr is trying to rename or delete.

**Recommended actions:**
1. SSH into `nashost` and check permissions:
   ```bash
   ls -la '/data/media/movies/Ready or Not 2 Here I Come (2026)/'
   docker inspect radarr | grep -E 'User|PUID|PGID'
   ```
2. Fix ownership: `chown -R <radarr-uid>:<radarr-gid> '/data/media/movies/Ready or Not 2 Here I Come (2026)/'`
3. Or delete the trailer file if it's not needed — the exception suggests Radarr wants to clean it up

---

### 3. 🔴 198.51.100.1 — Qdrant Panic Crash Loop (Resolved)

**Status:** Resolved — recovered at approximately 19:07 UTC. Was panicking every 5 seconds for ~20 minutes prior.

`axon-qdrant` entered a tight crash loop between approximately 18:49 and 19:07 UTC, panicking with:

```
ERROR qdrant::startup: Panic occurred in file
  lib/collection/src/update_handler.rs at line 374:
  Optimization error: Service internal error: IO Error: No such file or directory (os error 2)
```

The panic repeated on a ~5-second cadence (matching Docker restart-policy backoff or a built-in retry), generating hundreds of log entries. The stack trace shows `qdrant::startup::setup_panic_hook` catching an unhandled optimization error from the collection update handler — a missing file during a segment optimization or merge operation.

At 19:07 UTC, the service successfully restarted: `INFO qdrant::actix: Qdrant HTTP listening on 6333`. The `labby-master` peer (`lab-labby-master-1`) registered the qdrant upstream at 19:07 and again at 19:38. At 02:00 UTC, the `axon` collection was created successfully.

**Root cause:** Likely a corrupted or incomplete segment file left over from a previous unclean shutdown. Qdrant's WAL-based recovery couldn't reconstruct the missing file, and the optimizer kept retrying.

**Recommended actions:**
- Check Qdrant storage directory on `198.51.100.1` for orphaned segment files
- Monitor for recurrence — if the panic loop returns after reindex operations, consider enabling Qdrant's `on_disk_payload` option to reduce memory-mapped file complexity
- Review Qdrant version for known bugs in `update_handler.rs:374`

---

## High Severity Issues

### 4. 🟠 nashost — tracearr Plex 401 Unauthorized

**Status:** Active — firing intermittently throughout the report period.

The `tracearr` container is failing to authenticate with Plex:

```
[SSEProcessor] Error fetching session 528: HttpClientError: plex request failed: 401 Unauthorized
Error polling server NASHOST: HttpClientError: plex request failed: 401 Unauthorized
statusText: 'Unauthorized'
```

This affects both SSE session streaming and the primary polling loop for server `NASHOST`.

**Root cause:** The Plex API token stored in tracearr's configuration is expired, revoked, or belongs to an account that no longer has access to the `NASHOST` Plex server. This is a common occurrence after Plex account password changes or server re-claims.

**Recommended actions:**
1. Log into the Plex web UI and generate a new API token
2. Update tracearr's environment configuration with the new token
3. Restart the tracearr container

---

### 5. 🟠 WINHOST — `wsl-pro.service` Crash Loop

**Status:** Active — restart counter reached **162** by 03:27 UTC.

The `wsl-pro.service` (Ubuntu Pro / Landscape agent for WSL) on the Windows host `WINHOST` has been restarting continuously throughout the 24-hour period:

```
systemd: wsl-pro.service: Scheduled restart job, restart counter is at 162.
```

Restart events observed at: 01:23, 01:54, 02:25, 02:56, 03:27 UTC (approximately every 30 minutes visible in logs, actual restarts likely much more frequent).

**Root cause:** `wsl-pro.service` is a background service for Ubuntu Pro subscription management. At restart counter 162, this service has been failing for an extended period — likely hundreds of hours. The root cause is most likely network connectivity issues between WSL2 and the Ubuntu Pro endpoint, or an expired/invalid Ubuntu Pro token.

**Recommended actions:**
1. On WINHOST, run: `sudo systemctl status wsl-pro.service` and `journalctl -u wsl-pro.service -n 50`
2. If Ubuntu Pro is not needed: `sudo systemctl disable --now wsl-pro.service`
3. If needed: `sudo pro detach && sudo pro attach <token>`

---

## Medium Severity Issues

### 6. 🟡 edgehost — High-Volume Nginx Unauthorized Access (fail2ban Active)

**Status:** Being handled by fail2ban — no breach detected.

`edgehost` generated **33,493 warning-level logs** in 24 hours, almost entirely from `fail2ban` tracking `nginx-unauthorized` hits. Active attacker IPs:

| IP | Pattern | fail2ban Action |
|----|---------|----------------|
| `76.213.118.20` | Repeated hits across multiple time windows | Banned then unbanned at 03:23 UTC |
| `69.155.6.42` | Hits at 23:15 and 23:38 UTC | Under monitoring |
| `2600:1702:ad0:ed40::46` (IPv6) | Hits at 23:48 UTC | Under monitoring |

One confirmed ban/unban cycle for `76.213.118.20`: banned after hitting nginx-unauthorized threshold, unbanned after the 300-second window expired at 03:23 UTC.

Additionally, `sshd-session` on `devhost` logged: `error: connect_to 198.51.100.2 port 22: failed` — this is `nashost`'s Tailscale IP, suggesting a brief network hiccup between `devhost` and `nashost` at ~13:19 UTC.

**Recommended actions:**
- Consider increasing the fail2ban ban time for `nginx-unauthorized` from the default — repeated bans/unbans suggest the current window is too short
- Review nginx access logs on edgehost for the specific endpoints being probed by `76.213.118.20`
- Consider a persistent blocklist for IPs that trigger multiple ban cycles

---

### 7. 🟡 devhost — `sccache.service` Restart Loop

**Status:** Active — restart counter at **13** by 02:41 UTC.

`sccache` (the Rust compilation cache daemon) is being killed repeatedly, likely as a collateral victim of the OOM events. Each time `codex-acp` is killed by OOM, it appears `sccache.service` is either directly killed or loses its backing process:

```
systemd: sccache.service: Scheduled restart job, restart counter is at 13.
```

This directly degrades Rust compilation performance, forcing full recompiles rather than cache hits. The restart intervals correlate closely with OOM kill times.

**Recommended actions:** Resolving the OOM issue (item #1) should resolve this as a side effect. In the meantime, `sccache` can be set to a higher `oom_score_adj` value so it's deprioritized for killing.

---

## Low Severity / Informational

### 8. 🟢 laptophost — WSL Clock Sync & DNS Instability

Two related issues:

**Time sync:** `systemd-journald[61]: Time jumped backwards, rotating.` — observed **95 times** in one batch around 02:00 UTC and **46 times** around 03:00 UTC. This is a known WSL2 issue where the system clock jumps when the host Windows machine wakes from sleep or when Hyper-V time sync fires. WSL2 doesn't run a hardware clock, so clock jumps cause journald to log rotation events.

**DNS failures:** `WSL (220) ERROR: CheckConnection: getaddrinfo() failed: -3` — DNS resolution failing inside WSL at approximately 23:09 and 23:11 UTC. `tailscale.tailscaled` restarted `resolved` at 02:41 UTC, suggesting Tailscale's DNS handling was the recovery action.

**Recommended actions:**
- Install `ntp` or `systemd-timesyncd` in the WSL distro to smooth out clock jumps
- The DNS failures are likely transient and resolved by tailscale's resolver restart — monitor for recurrence

### 9. 🟢 devhost — Snap Service Failures (Benign/Known)

Several snap-managed services fail on every boot and on their hourly scheduled runs:

| Service | Frequency | Pattern |
|---------|-----------|---------|
| `snap.firmware-updater.firmware-notifier` | Hourly (every :00) on devhost AND edgehost | Fails, restarts, fails — counter reaching 4-5 per run |
| `snap.prompting-client.daemon` | After reboots | Likely missing kernel feature flag |
| `xdg-desktop-portal.service` | After session starts | Portal service fails when no desktop session active |
| `xdg-desktop-portal-gtk.service` | After session starts | Same |

These are well-known Ubuntu/snap packaging issues. The `firmware-updater.firmware-notifier` snap attempts to check for firmware updates hourly but frequently fails due to connectivity or snap confinement issues. The XDG portal failures happen when systemd user session services start before a full desktop session is available.

**Recommended actions:**
- `sudo snap remove firmware-updater` if firmware update notifications aren't needed
- The portal failures are cosmetic — no action required unless specific D-Bus-dependent apps are breaking

### 10. 🟢 nashost — High-Volume SSH from 198.51.100.2 (Automation, Not Attack)

At approximately 03:18 UTC, `nashost` received a burst of rapid-fire SSH connections from `198.51.100.2` (devhost's Tailscale IP), all authenticated via the same ED25519 key (`SHA256:REDACTED_SYNTHETIC_FINGERPRINT`). Sessions opened and closed within milliseconds, consistent with SSH multiplexing or scripted parallelism (e.g., Ansible, fabric, or rsync-over-SSH). All authenticated successfully as `root`.

This appears to be legitimate automation from `devhost` → `nashost`. No concern, but worth noting that root-as-SSH-target is elevated risk if the key is ever compromised.

---

## Docker Container Health Summary

| Host | Container | Status | Issues |
|------|-----------|--------|--------|
| nashost | radarr | Running | 🔴 Permission denied loop (every 90s) |
| nashost | tracearr | Running | 🟠 Plex 401 Unauthorized |
| nashost | immich_redis | Healthy | Saving DB every 5 minutes normally |
| nashost | tracearr-redis | Healthy | Saving DB every 5 minutes normally |
| edgehost | paperless-cache | Healthy | Saving DB every 5 minutes normally |
| 198.51.100.1 | axon-qdrant | Recovered | 🔴 Panic loop 18:49–19:07 (resolved) |
| 198.51.100.1 | lab-labby-master-1 | Healthy | Qdrant peer re-registered successfully |

**Notable:** `devhost` runs a scheduled `artifact-prune.service` approximately every 15 minutes (`Rust target/ + Docker`) that completes successfully each time — disk maintenance is healthy.

---

## Activity Timeline

```
2026-05-07
  13:00 UTC  ─── BACKUPHOST stops reporting (hostname casing change to backuphost)
  13:19 UTC  ─── devhost → nashost SSH connection failure (brief network hiccup)
  18:49 UTC  ─┐  axon-qdrant panic loop begins (every 5s)
  19:07 UTC  ─┘  axon-qdrant recovers, HTTP/gRPC listeners up
  19:38 UTC  ─── labby-master re-registers qdrant peer
  22:47 UTC  ─── OOM kill #1: codex-acp (26.5 GB RSS)
  23:09 UTC  ─── laptophost DNS failures
  23:33 UTC  ─── OOM kills #2/3 (app.slice events)

2026-05-08
  01:12 UTC  ─── OOM kill #4: user@1000.service cascading OOM events
  02:00 UTC  ─── qdrant creates 'axon' collection
  02:14 UTC  ─── OOM kills #5/6 (app.slice events)
  02:21 UTC  ─── OOM kill sequence in session-scope
  02:29 UTC  ─── OOM kill #7: codex-acp (28.2 GB RSS)
  02:45 UTC  ─── OOM kill #8: codex-acp (34.0 GB RSS) — worst event
  02:57 UTC  ─── fail2ban bans 76.213.118.20 on edgehost
  03:00 UTC  ─── OOM kill #9: app.slice/-.slice
  03:18 UTC  ─── SSH burst from devhost → nashost (automation)
  03:23 UTC  ─── fail2ban unbans 76.213.118.20
  03:27 UTC  ─── WINHOST wsl-pro.service restart #162
  03:57 UTC  ─── Report end; Radarr permission loop still active
```

---

## Recommended Action Priority

| Priority | Action | Host | Estimated Effort |
|----------|--------|------|-----------------|
| P0 — Do now | Add swap + set `oom_score_adj` for `codex-acp` | devhost | 15 min |
| P0 — Do now | Fix Radarr file permissions on movie directory | nashost | 5 min |
| P1 — Today | Rotate Plex API token in tracearr config | nashost | 10 min |
| P1 — Today | Disable or fix `wsl-pro.service` | WINHOST | 5 min |
| P2 — This week | Investigate `axon-qdrant` segment corruption root cause | 198.51.100.1 | 30 min |
| P2 — This week | Increase fail2ban ban duration for `nginx-unauthorized` | edgehost | 10 min |
| P3 — Low priority | Remove `snap firmware-updater` if not needed | devhost, edgehost | 2 min |
| P3 — Low priority | Investigate laptophost WSL2 time sync | laptophost | 20 min |
