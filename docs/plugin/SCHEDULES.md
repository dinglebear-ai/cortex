---
title: "Scheduled Tasks -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Scheduled Tasks -- cortex

cortex does not use Claude Code scheduled tasks (triggers). Internal scheduled operations are handled by the Rust binary itself.

## Internal schedules

`cortex serve mcp` runs two periodic background tasks:

| Task | Interval | Purpose |
| --- | --- | --- |
| Retention purge | Hourly | Delete logs older than `retention_days` |
| Storage budget enforcement | Every `cleanup_interval_secs` (default 60s) | Delete oldest logs when DB size or free disk thresholds are breached |

These tasks run inside the tokio runtime and do not depend on external schedulers or Claude Code triggers.

## See also

- [../CONFIG.md](../CONFIG.md) -- retention and storage budget configuration
