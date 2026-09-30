# Consolidated Issue Register

This document consolidates every distinct issue surfaced across all `.full-review/` phase documents and the final report. Duplicate mentions are collapsed into one issue with source traceability.

## Source Documents

- `.full-review/00-scope.md`
- `.full-review/01-quality-architecture.md`
- `.full-review/02-security-performance.md`
- `.full-review/03-testing-documentation.md`
- `.full-review/04-best-practices.md`
- `.full-review/05-final-report.md`

## Summary Counts

| Severity | Count |
|---|---:|
| Critical | 0 |
| High | 3 |
| Medium | 12 |
| Low | 6 |
| Total | 21 |

## High Priority

### CFR-001 — OAuth `allowed_emails` is accepted but not enforced

- Severity: High
- Categories: Security, Configuration, Auth
- Source documents: `02-security-performance.md`, `05-final-report.md`
- References: `src/config.rs:1039`, `src/runtime.rs:574`
- Issue: OAuth startup validation accepts a non-empty `allowed_emails` list as satisfying the allowlist requirement, but `build_auth_policy` documents that `lab-auth` has no `allowed_emails` field and only enforces `admin_email`.
- Impact: Operators can configure `allowed_emails = ["ops@example.com"]` with no enforced allowlist and believe access is restricted when it is not.
- Fix: Either enforce `allowed_emails` in cortex before issuing sessions, or reject OAuth configs that rely on `allowed_emails` until `lab-auth` supports it. Update docs and tests to match the chosen contract.

### CFR-002 — Tests lock in unsupported OAuth allowlist behavior

- Severity: High
- Categories: Testing, Auth
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `src/config_tests.rs:814`, `src/runtime.rs:574`
- Issue: `oauth_mode_accepts_non_empty_allowlist` expects a config with only `allowed_emails` to pass even though runtime integration only enforces `admin_email`.
- Impact: The test suite prevents a correct fail-closed change unless the tests are updated first.
- Fix: Replace the test with `oauth_mode_rejects_allowed_emails_without_admin_until_enforced`, or add a full callback/auth integration test proving `allowed_emails` enforcement.

### CFR-003 — OAuth docs incorrectly state `allowed_emails` is enforced

- Severity: High
- Categories: Documentation, Security, Auth
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `docs/OAUTH.md:87`, `docs/OAUTH.md:107`, `docs/OAUTH.md:128`, `docs/OAUTH.md:141`
- Issue: OAuth operator docs say `allowed_emails` is enforced at callback time and can be changed with SIGHUP/restart, conflicting with `src/runtime.rs:574` and `docs/contracts/config-schema.md:240`.
- Impact: Operators can follow documentation and deploy a configuration that does not enforce the intended multi-user allowlist.
- Fix: Document `admin_email` as the only enforced allowlist until multi-user enforcement lands, or implement and test `allowed_emails` before restoring the instructions.

## Medium Priority

### CFR-004 — Mutating MCP actions lose per-request caller identity

- Severity: Medium
- Categories: Security, Architecture, Auditability
- Source documents: `01-quality-architecture.md`, `02-security-performance.md`, `05-final-report.md`
- References: `src/mcp/tools.rs:525`, `src/mcp/tools.rs:588`, `src/mcp/tools.rs:601`, `src/mcp/tools.rs:629`
- Issue: `ack_error`, `unack_error`, and `notifications_test` derive the audit actor from `AppState` auth mode, collapsing OAuth users into `"mcp:oauth"` and bearer callers into `"mcp:bearer"`.
- Impact: The append-only acknowledgement and notification audit trail cannot identify the actual actor for administrative actions.
- Fix: Thread `Option<&AuthContext>` from `SyslogRmcpServer::call_tool` into `execute_tool` / `tool_syslog`, then pass the subject or email into the service methods.

### CFR-005 — CLI and core modules are oversized and tightly coupled

- Severity: Medium
- Categories: Code Quality, Maintainability
- Source documents: `01-quality-architecture.md`, `05-final-report.md`
- References: `src/cli.rs:24`, plus large modules `src/mcp/tools.rs`, `src/db/queries.rs`, `src/scanner.rs`, `src/config.rs`, `src/app/service.rs`
- Issue: CLI command model, parsing, request mapping, output formatting, smoke helpers, and system probes live in one 3,171-line file; several core modules exceed 1,000 lines.
- Impact: New command or action work must touch large mixed-concern modules, increasing regression risk and review cost.
- Fix: Split along existing boundaries: `cli/parse.rs`, `cli/run.rs`, `cli/output.rs`, `cli/ai.rs`, `cli/db.rs`, `cli/compose.rs`; apply similar grouping to MCP actions and database AI-query helpers.

### CFR-006 — Full doctor orchestration is embedded in `main.rs`

- Severity: Medium
- Categories: Architecture, Maintainability
- Source documents: `01-quality-architecture.md`, `05-final-report.md`
- References: `src/main.rs:143`
- Issue: `run_doctor_full` performs setup, compose, binary freshness, AI doctor, JSON shaping, text shaping, and exit-code policy inside the binary entrypoint.
- Impact: `main.rs` contains orchestration logic that belongs in a library/CLI module and duplicates the broader CLI orchestration surface.
- Fix: Move doctor report collection and formatting into a dedicated module with typed report structs; keep `main.rs` as dispatch only.

### CFR-007 — `search_ai_sessions` uses a per-group correlated full-history count

- Severity: Medium
- Categories: Performance, Database
- Source documents: `02-security-performance.md`, `05-final-report.md`
- References: `src/db/queries.rs:365`
- Issue: `search_ai_sessions` computes `event_count` with a correlated `SELECT COUNT(*) FROM logs total` for every grouped session.
- Impact: A bounded FTS candidate query can still trigger repeated scans or index probes across full AI log history on large transcript databases.
- Fix: Pre-aggregate event counts in a CTE joined once per group, add an index aligned to `(ai_project, ai_tool, ai_session_id, hostname)`, and capture `EXPLAIN QUERY PLAN` evidence.

### CFR-008 — `ai_correlate` serializes many DB searches per request

- Severity: Medium
- Categories: Performance, Database, MCP
- Source documents: `02-security-performance.md`, `05-final-report.md`
- References: `src/app/service.rs:229`
- Issue: `correlate_ai_logs` fetches anchors, then performs one database search per anchor. Clamps cap this to 50 anchors, but the request can still serialize up to 51 blocking DB jobs.
- Impact: Concurrent expensive AI correlation requests can occupy the DB semaphore and delay normal search, tail, and status calls.
- Fix: Batch related-window lookup into one database query where practical, or route expensive analytics through a lower concurrency class / explicit timeout.

### CFR-009 — OAuth tests do not cover callback allowlist behavior

- Severity: Medium
- Categories: Testing, Auth
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `tests/oauth_flow.rs:26`
- Issue: JWT-level OAuth tests validate issuer, audience, signature, expiry, and scopes but do not exercise the OAuth callback allowlist path or a config using `allowed_emails`.
- Impact: The highest-risk auth mismatch remains outside integration coverage.
- Fix: Add a lab-auth callback/authorization test for allowed and denied email identities, or test startup rejection when `allowed_emails` is configured without enforced support.

### CFR-010 — Admin action tests do not prove identity propagation

- Severity: Medium
- Categories: Testing, Auditability
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `src/mcp/rmcp_server_tests.rs:79`, `src/mcp/tools.rs:525`
- Issue: Tests prove `AuthContext` reaches RMCP handlers and scope gates work, but they do not prove mutating tools receive the request identity.
- Impact: The current mode-level actor implementation can pass all auth/scope tests while still producing weak audit records.
- Fix: Add an admin-action test that injects two different `AuthContext` subjects and verifies distinct `acknowledged_by` / notification actor values in the database.

### CFR-011 — AI analytics tests lack query-plan/load regression coverage

- Severity: Medium
- Categories: Testing, Performance
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `src/db/queries_tests.rs:397`, `tests/test_live.sh:547`
- Issue: AI session search tests cover grouping and candidate-cap behavior, and live tests cover `ai_correlate` response shape, but neither captures query-plan properties or load behavior for large per-session histories.
- Impact: Future changes can reintroduce expensive query plans or excessive per-request DB work without failing tests.
- Fix: Add focused `EXPLAIN QUERY PLAN` evidence or benchmark-style tests for `search_ai_sessions`, plus a service-level test that asserts `ai_correlate` uses a bounded/batched database strategy.

### CFR-012 — Main CI and crates publish workflows use unpinned actions

- Severity: Medium
- Categories: Standards, Supply Chain, CI
- Source documents: `04-best-practices.md`, `05-final-report.md`
- References: `.github/workflows/ci.yml:17`, `.github/workflows/publish-crates.yml:15`
- Issue: CI and publish workflows use unpinned third-party actions such as `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `rustsec/audit-check@v2.0.0`, and `gitleaks/gitleaks-action@v2`.
- Impact: The main CI and crates publish paths do not match the safer SHA-pinned pattern already used in the Docker workflow.
- Fix: Pin all third-party actions to full commit SHAs and use Dependabot or a scheduled process for updates.

### CFR-013 — Version and changelog sync are not enforced by CI/publish

- Severity: Medium
- Categories: Standards, Release, CI
- Source documents: `04-best-practices.md`, `05-final-report.md`
- References: `.github/workflows/ci.yml:51`, `.github/workflows/publish-crates.yml:19`, `scripts/check-version-sync.sh:82`
- Issue: The repo has a version-sync script and instructions requiring all version-bearing files plus `CHANGELOG.md` to stay aligned, but CI does not run the script. The crates publish workflow only checks `Cargo.toml` against the tag, and the script only warns on missing changelog entries.
- Impact: Release artifacts can drift across `Cargo.toml`, `server.json`, plugin metadata, README, and changelog.
- Fix: Run `scripts/check-version-sync.sh` in CI and publish workflows, and make missing changelog entries fail when releasing.

### CFR-014 — `just publish` bypasses release rules and tolerates build failure

- Severity: Medium
- Categories: Standards, Release
- Source documents: `04-best-practices.md`, `05-final-report.md`
- References: `Justfile:102`
- Issue: `just publish` edits only `Cargo.toml`, runs `cargo check 2>/dev/null || true`, commits, tags, and pushes.
- Impact: The release command can publish without synchronized versions, changelog updates, plugin/server metadata updates, or a passing `cargo check`.
- Fix: Replace the inline recipe with `scripts/bump-version.sh`, `scripts/check-version-sync.sh`, `cargo test`, and `cargo clippy -- -D warnings` before tagging.

### CFR-015 — AI query SQL assembly lacks reusable structure

- Severity: Medium
- Categories: Code Quality, Database
- Source documents: `01-quality-architecture.md`, `05-final-report.md`
- References: `src/db/queries.rs:290`
- Issue: AI session search uses hand-assembled SQL strings with correlated subqueries and repeated filter assembly.
- Impact: Query-plan reasoning and future changes are harder, even though parameter binding avoids SQL injection.
- Fix: Extract reusable filter builders for AI transcript queries and add query-plan fixtures for high-volume paths.

## Low Priority

### CFR-016 — MCP action dispatch/help duplicates schema and docs

- Severity: Low
- Categories: Code Quality, Documentation
- Source documents: `01-quality-architecture.md`, `05-final-report.md`
- References: `src/mcp/tools.rs:1`
- Issue: MCP action dispatch, argument parsing, help text, and result shaping are coupled in one module. Embedded help text repeats schema knowledge from `src/mcp/schemas.rs` and docs.
- Impact: Schema/help drift is likely as actions evolve.
- Fix: Generate help from action metadata or centralize action descriptors with parser, schema, scope, and help fields.

### CFR-017 — MCP and API CORS allow any request header

- Severity: Low
- Categories: Security, Standards
- Source documents: `02-security-performance.md`, `04-best-practices.md`, `05-final-report.md`
- References: `src/mcp/routes.rs:144`, `src/api.rs:181`
- Issue: MCP and API CORS layers use `allow_headers(Any)`.
- Impact: This is not currently an access bypass because auth and Host/Origin validation exist, but it is looser than necessary for a service carrying log data and admin actions.
- Fix: Allowlist `Authorization`, `Content-Type`, `Accept`, MCP protocol headers, and any explicitly required custom headers.

### CFR-018 — Deferred `/v1/traces` OTLP endpoint does not share auth behavior

- Severity: Low
- Categories: Security, API Consistency
- Source documents: `02-security-performance.md`, `05-final-report.md`
- References: `src/otlp.rs:225`
- Issue: `/v1/traces` returns a deferred 404 without checking bearer token, unlike `/v1/logs` and `/v1/metrics`.
- Impact: Current impact is low because the handler does not ingest or reveal data, but the shared OTLP surface has inconsistent auth semantics.
- Fix: Make deferred OTLP endpoints share the same authorization helper as `/v1/logs`, or document intentionally public 404 behavior and test it.

### CFR-019 — README top-level MCP action list is stale

- Severity: Low
- Categories: Documentation
- Source documents: `03-testing-documentation.md`, `05-final-report.md`
- References: `README.md:27`
- Issue: The README top-level action list omits newer administrative/notification actions: `unaddressed_errors`, `ack_error`, `unack_error`, `notifications_recent`, `notifications_test`.
- Impact: Discoverability gap for supported MCP actions.
- Fix: Generate the README action list from `src/mcp/schemas.rs` or update it whenever schema actions change.

### CFR-020 — Migration 13 is less drift-tolerant than earlier migrations

- Severity: Low
- Categories: Operations, Database
- Source documents: `04-best-practices.md`, `05-final-report.md`
- References: `src/db/pool.rs:610`
- Issue: Migration 13 uses one `BEGIN IMMEDIATE` batch with unconditional `ALTER TABLE ... ADD COLUMN` statements. It is guarded by `schema_migrations`, but manually repaired or partially migrated databases with missing version rows can fail on duplicate columns.
- Impact: Recovery from migration drift is harder than with earlier column-exists guarded migrations.
- Fix: Follow the column-exists pattern used in earlier migrations before each `ALTER TABLE`, or document explicit recovery steps for migration drift.

### CFR-021 — Live review artifacts are ignored by git

- Severity: Low
- Categories: Operations, Documentation
- Source documents: `05-final-report.md`
- References: `.full-review/`
- Issue: `.full-review/` is ignored by git in the current checkout.
- Impact: A normal `git add .` will not preserve the review artifacts.
- Fix: Force-add the review artifacts if they should be committed, or copy/move the consolidated report into a tracked review/documentation location.

## Recommended Fix Order

1. CFR-001, CFR-002, CFR-003: Fix the OAuth allowlist contract across code, tests, and docs.
2. CFR-004, CFR-010: Thread real request identity into mutating actions and prove it in tests.
3. CFR-007, CFR-008, CFR-011, CFR-015: Add query-plan/load coverage, then optimize AI analytics paths.
4. CFR-013, CFR-014: Enforce release/version rules in CI and `just publish`.
5. CFR-012: Pin remaining CI/publish actions.
6. CFR-005, CFR-006, CFR-016: Split oversized modules and centralize action metadata.
7. CFR-017, CFR-018, CFR-019, CFR-020, CFR-021: Clean up lower-risk security, docs, migration, and artifact-preservation issues.
