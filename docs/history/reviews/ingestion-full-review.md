# Ingestion Full-Review Report

Date: 2026-05-07

Scope: syslog UDP/TCP receive path, syslog parsing/enrichment handoff, batch writer, SQLite insert/checkpoint path, Docker log ingestion, and runtime/maintenance behavior that can block or drop ingest.

Tracker epic: `cortex-vzg8`

## Artifact Policy

`.full-review/` is an ignored scratch workspace for live review notes. Final review reports that need to survive branch handoff must be copied to a tracked `docs/reviews/` path. This file is the tracked ingestion report and should be treated as the durable source for the ingestion remediation epic.

## Findings

### Fixed or Covered in Current Remediation

1. TCP live smoke coverage was incomplete.
   - Evidence: `scripts/smoke-test.sh` seeded UDP messages only.
   - Remediation: default smoke now seeds one TCP syslog frame with a unique marker and validates the frame through both `tail` and `search`.
   - Bead: `cortex-vzg8.10`

2. Docker ingest did not have a practical smoke-test boundary.
   - Evidence: Docker ingest requires either a deployed host-local agent with Docker socket access or a real/mocked legacy Docker HTTP endpoint.
   - Remediation: default smoke remains practical; host-local agent parity is covered separately and legacy central pull ingest is documented/tested through a mocked Docker HTTP fixture.
   - Bead: `cortex-vzg8.10`

3. TCP message-size documentation could overstate protection.
   - Evidence: config docs described a generic max message size without TCP frame behavior.
   - Current code state: TCP receive uses bounded newline-delimited frames and drops oversized frames without treating one persistent TCP connection as one unbounded message.
   - Remediation: README and config docs now state the limit is per UDP datagram or per TCP frame.
   - Bead: `cortex-vzg8.11`

4. Heavy SQLite migrations were operator-visible in code but not in runbooks.
   - Evidence: `src/db/pool.rs` documents migration 3 startup cost and logs start/completion.
   - Remediation: deployment and config docs now include backup, downtime, monitoring, health-check, stats, and rollback steps for populated databases.
   - Beads: `cortex-vzg8.9`, `cortex-vzg8.15`

5. Review artifact drift made the ingestion report non-durable.
   - Evidence: the original final report was under ignored `.full-review/`, which can be overwritten by later review scopes.
   - Remediation: this tracked `docs/reviews/ingestion-full-review.md` report records the ingestion scope and remediation state.
   - Bead: `cortex-vzg8.17`

## Remaining Follow-Up

- A full Docker ingest integration fixture can still be automated later. The current branch documents the required fixture path and keeps the default smoke test local and fast.
- Future heavyweight migrations should add an explicit release-note callout and keep the startup log wording operator-visible.

## Verification

- Run `bash -n scripts/smoke-test.sh` after smoke script edits.
- Run the full live smoke test only against a running cortex daemon with `mcporter`, `nc`, `curl`, and `python3` available.
