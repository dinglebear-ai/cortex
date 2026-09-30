# Service Layer Boundary Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Epic:** `syslog-mcp-yab3` - Centralize transport-owned business policy into the service layer.

**Goal:** Move shared business policy out of MCP, REST, and CLI adapters. Transports should parse transport-native payloads, attach caller context, call service/app operations, and render responses. Semantic defaults, caps, safety gates, notification policy, compose diagnostics policy, and audit identity semantics belong in the service layer or service-owned operation modules.

**Architecture:** Preserve `src/app/` as the application boundary. Add focused request/operation models where policy is currently duplicated or transport-owned. Keep response JSON compatible unless a bead explicitly documents an intentional correction. Reuse existing shared `Arc<Pool>` and `Arc<Semaphore>` patterns for service sub-boundaries; do not create per-transport concurrency limiters.

**Primary Beads:**
- `syslog-mcp-yab3.1` - Move compose diagnostics into a service-owned ops boundary.
- `syslog-mcp-yab3.2` - Centralize MCP request deserialization and action validation.
- `syslog-mcp-yab3.3` - Move AI correlation and investigation caps into service policy.
- `syslog-mcp-yab3.4` - Add service-owned notification request models and delivery policy.
- `syslog-mcp-yab3.5` - Move DB maintenance guardrails into service admin operations.
- `syslog-mcp-yab3.6` - Introduce typed request actor and audit identity.
- `syslog-mcp-yab3.7` - Refresh docs and cross-surface tests after service-boundary refactor.
- `syslog-mcp-5gcn` - Move ai watch-status host probing into service layer.

---

## Task 1: Establish Boundary Types

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app/service.rs`
- Add if useful: `src/app/context.rs` or `src/app/admin.rs`

- [x] Add a typed caller context, for example `RequestActor`, with surface, display name, and optional subject/email fields.
- [x] Add constructors for API, MCP, CLI, and internal/system actors.
- [x] Replace string-only service mutation entrypoints incrementally where audit provenance matters first: notification ack/unack, incident operations, and admin operations.
- [x] Add unit tests for actor display formatting and provenance retention.

## Task 2: Move AI Limit Policy Into Service

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app/service.rs`
- Modify: `src/api.rs`
- Modify: `src/mcp/tools.rs`
- Modify tests beside touched modules.

- [x] Add service-owned normalization for correlation/investigation request limits and `events_per_anchor`.
- [x] Return effective limit/cap/truncation metadata from the service instead of patching REST responses after execution.
- [x] Align MCP and REST behavior, or introduce an explicit service execution profile if compatibility requires different caps.
- [x] Fix `abuse_investigate` so MCP passes `incident_id` through when provided.
- [x] Add tests for defaults, caps, truncation metadata, and `incident_id` lookup.

## Task 3: Move Compose Diagnostics Policy Into Service

**Files:**
- Modify: `src/app/service.rs`
- Add if useful: `src/app/compose_ops.rs`
- Modify: `src/api.rs`
- Modify: `src/mcp/tools.rs`

- [x] Create a service-owned compose diagnostics operation for status and doctor.
- [x] Centralize target override rejection/defaulting, redacted projection, readiness rules, blocking execution, and error mapping.
- [x] Share one process-wide limiter across REST and MCP.
- [x] Preserve existing `compose_status` and `compose_doctor` response shape.
- [x] Add service tests for default target, rejected target override, projection shape, and limiter behavior where practical.

## Task 4: Centralize MCP Request Validation

**Files:**
- Modify: `src/mcp/tools.rs`
- Modify: `src/app/models.rs`
- Modify tests beside touched modules.

- [x] Convert high-risk MCP actions from manual `get_*` extraction to typed payload deserialization plus service-owned validation.
- [x] Prioritize `abuse_investigate`, AI correlation/investigation, notifications, and DB maintenance actions.
- [x] Keep generic JSON parsing errors in MCP, but move semantic bounds and required business fields into app request constructors.
- [x] Add regression tests for missing-field and invalid-limit errors.

## Task 5: Move Notification Policy Into Service

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app/service.rs`
- Modify: `src/mcp/tools.rs`
- Modify: `src/api.rs`
- Modify: `src/cli/dispatch_surface.rs`

- [x] Add `NotificationsRecentRequest` with shared default and limit normalization.
- [x] Route MCP, REST, and CLI recent-notification queries through that request model.
- [x] Change notification test delivery so config/destination policy is service-owned.
- [x] Do not expose arbitrary destination URL/config strings as generic service primitives unless an explicit trusted-admin operation documents the trust boundary.
- [x] Add tests for defaults, max limits, disabled notification config, and caller identity.

## Task 6: Move DB Maintenance Safety Gates Into Service

**Files:**
- Modify: `src/app/models.rs`
- Modify: `src/app/service.rs`
- Modify: `src/api.rs`
- Modify tests beside touched modules.

- [x] Move checkpoint mode allowlist into service/admin request validation.
- [x] Move vacuum force/size thresholds and single-flight policy into service/admin operations.
- [x] Move AI checkpoint prune dry-run, confirmation, and audit gates into service/admin operations.
- [x] Leave HTTP handlers responsible only for request extraction, auth, service call, and response rendering.
- [x] Add service tests that prove unsafe calls are rejected without going through HTTP.

## Task 7: Refresh Docs And Contract Tests

**Files:**
- Modify: `README.md`
- Modify: `docs/CLI.md`
- Modify: `docs/INVENTORY.md`
- Modify: `docs/mcp/*.md` as needed
- Modify tests for schema/docs drift where needed.

- [x] Update docs so they describe MCP as an exposure surface, not the owner of correlation or other business policy.
- [x] Clarify that runtime MCP schema is generated from `src/mcp/actions.rs` and exposed as `syslog://schema/mcp-tool`; maintained docs are drift-checked, not automatically generated unless generation is actually added.
- [x] Add or adjust drift tests for any new generated contract.
- [x] Run stale phrase checks against the old ownership language.

## Task 8: Version, Verification, And Closeout

**Files:**
- Modify version-bearing files required by repo policy.
- Modify: `CHANGELOG.md`

- [x] Bump patch version for the refactor branch unless the final commit semantics require a different bump.
- [x] Add a CHANGELOG entry summarizing the service-boundary cleanup.
- [x] Run `cargo fmt`.
- [x] Run `cargo test`.
- [x] Run `cargo clippy` if time permits before push.
- [x] Run `git diff --check`.
- [x] Update session documentation for the completed service-boundary pass.
- [x] Update and close completed child beads; leave explicit notes on deferred children.
- [x] Commit, push, and open or update the PR according to `work-it`.
