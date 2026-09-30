# Service Layer Boundary Refactor Session

Date: 2026-05-25
Branch: service-layer-boundaries
Epic: syslog-mcp-yab3

## Scope

Moved shared transport-owned policy into the service/application boundary for the service-layer audit epic. The branch keeps MCP, REST, and CLI as adapters that extract request context and call service-owned request/operation models.

## Completed

- Added typed `RequestActor` provenance for MCP, API, CLI, and internal/service callers.
- Moved AI correlation/search cap policy into service-owned limit profiles and fixed MCP `abuse_investigate` incident id forwarding.
- Centralized compose status/doctor projection behind one service-owned operation with a process-wide limiter.
- Added service-owned request models for notification recent queries, notification test delivery, DB checkpoint, DB vacuum, and AI checkpoint pruning.
- Refreshed README, CLI, inventory, and MCP schema docs to describe MCP as an exposure surface and clarify that runtime schema is generated from `ACTION_SPECS` while Markdown docs are maintained and drift-checked.
- Bumped version to 0.32.7 with a CHANGELOG entry.

## Verification

- `cargo test app:: -- --nocapture`
- `cargo test ai_correlate -- --nocapture`
- `cargo test db_vacuum -- --nocapture`
- `cargo test compose -- --nocapture`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `git diff --check`
- `rg -n "SCHEMA\\.md.*generated|generated.*SCHEMA\\.md|automatically generated|MCP.*own(s|er)|correlation.*MCP" README.md docs/mcp docs/CLI.md docs/INVENTORY.md`

## Tracker Notes

Completed child beads: syslog-mcp-yab3.1 through syslog-mcp-yab3.7.

Deferred child bead: syslog-mcp-5gcn remains open because moving `ai watch-status` host probing out of CLI is a separate host-runtime refactor and was not included in this branch.
