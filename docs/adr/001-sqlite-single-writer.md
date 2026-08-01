# ADR 001: SQLite Single-Writer Architecture

**Status:** Accepted
**Date:** 2026-05-21

## Context

cortex uses a single SQLite file (`/data/cortex.db`) for all persistence: log ingest, FTS5, notifications outbox, error signatures, AI transcript index, docker checkpoints, and schema migrations. WAL mode enables concurrent readers but enforces a single writer at a time.

All write paths contend on this file:
- Syslog batch writer (up to 10k entries/flush)
- Docker log ingest
- OTLP ingest
- Notification outbox / firings
- Error signature acks
- Retention purge + storage enforcement
- WAL checkpoint + incremental vacuum
- AI transcript scanner

## Scale Ceiling

This architecture is validated for homelab scale:
- **Hosts:** up to ~50 syslog senders
- **Ingest rate:** up to ~5,000 events/second sustained
- **Database size:** up to ~50 GB (tested)
- **Retention:** configurable; default 30 days

Symptoms of approaching the ceiling already visible in codebase:
- FTS5 DELETE/UPDATE triggers removed (`db/pool.rs:117-124`) because retention purge held write lock long enough to starve the batch writer.
- AdGuard retention hard-capped at 7 days (`runtime.rs:81`) because query volume would otherwise dominate the FTS5 index.
- Dual semaphores (`maintenance_permit`, `dispatcher_permit`) serialise background writers to prevent write contention.

## Decision

Accept the single-writer ceiling for the homelab use-case. Do not add multi-writer complexity until the symptoms listed above consistently manifest in production (sustained ingest drops, WAL > 1 GB between checkpoints, or retention lag > 1 hour).

## Planned Mitigation (when ceiling is hit)

Use `ATTACH DATABASE` to split into:
- **Hot ingest DB** (`cortex.db`): `logs` table + FTS5 index. Write-heavy. Retention and WAL tuned for throughput.
- **Control-plane DB** (`control.db`): `notifications_outbox`, `notification_firings`, `error_signatures`, `transcript_sources`, `docker_checkpoints`, `schema_migrations`, `hosts`. Write-occasional. Different durability requirements.

Both databases share the same `r2d2` pool abstraction and can be split with minimal API changes. This is the first escalation step before considering a different storage engine.

## Consequences

- Any new feature that writes to the database adds write pressure. Document expected write rate in the PR.
- Long-running DB maintenance (VACUUM, CREATE INDEX) must use the `maintenance_permit` semaphore.
- If WAL grows beyond ~500 MB between checkpoint cycles, reduce the `storage.cleanup_interval_secs` or add a periodic `PRAGMA wal_checkpoint(PASSIVE)` task.
