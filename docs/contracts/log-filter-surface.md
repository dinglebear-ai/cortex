---
title: "Log Filter Surface Contract"
created: 2026-05-24
updated: 2026-07-30
---

# Log Filter Surface Contract

Status: v1 contract, implemented by `cortex filter`, `GET /api/filter`, and MCP `action=filter`.

## Purpose

`filter` returns log rows by structured fields only. It is not full-text search and never accepts a `query` field. Use `search` when the message body needs SQLite FTS5 syntax.

## Shared Fields

- `hostname`
- `source_ip`
- `severity`
- `app_name`
- `facility`
- `exclude_facility`
- `process_id`
- `from`, `to`
- `received_from`, `received_to`
- `limit`

These fields match `search` without `query` and use the same timestamp parsing, severity validation, ordering, and limit behavior.

## Correlation Aliases

- `source_kind=docker-stream`: filters `source_ip` by the `docker://` prefix.
- `source_kind=docker-event`: filters `source_ip` by the `docker-event://` prefix.
- `source_kind=file-tail`: filters `source_ip` by the `file-tail://` prefix.
- `source_kind=agent-command`: filters `source_ip` by the `agent-command://` prefix.
- `source_kind=shell-history`: filters `source_ip` by the `shell-history://` prefix.
- `source_kind=transcript`: filters transcript rows when combined with `tool`, `project`, or `session_id`.
- `source_kind=claude`, `codex`, `gemini`: aliases for `tool=<name>`.

Docker refiners:

- `docker_host`
- `container`
- `stream`
- `event_action`

AI/session refiners:

- `tool`
- `project`
- `session_id`

## Rejections

- `query` is rejected by `filter`.
- CLI positional terms are rejected by `cortex filter`; use `cortex search`.
- `stream` is rejected unless `source_kind=docker-stream`.
- `source_kind=syslog-udp`, `source_kind=syslog-tcp`, and `source_kind=otlp` are rejected in v1 because transport protocol is not indexed separately.
- Unknown fields are rejected by REST and MCP.

## Cost Model

Cheap:

- exact indexed fields such as `hostname`, `source_ip`, `app_name`, `facility`, timestamps, and AI tuple fields.
- bounded source-prefix filters for Docker, shell history, and agent commands.

Moderate:

- broad source-prefix filters without a time window or app/container refiner.

Unsupported in v1:

- broad JSON metadata scans.
- transport-specific syslog/OTLP source-kind filtering without an indexed column.

## Trust Boundary

`source_ip` is the persisted source identifier. For network syslog it is the sender address; for internal importers it is a synthetic scheme such as `docker://...`, `agent-command://...`, or `shell-history://...`. Hostnames and app names are useful correlation keys, but they are not equivalent to network-verified identity.
