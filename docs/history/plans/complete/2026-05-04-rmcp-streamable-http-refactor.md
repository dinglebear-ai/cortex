# RMCP Streamable HTTP Refactor Plan

> **Tracker:** `cortex-hea` and children `cortex-hea.1` through `cortex-hea.6`.
> **Implementation mode:** Claim one bead at a time with `bd update <id> --claim`, keep commits scoped to that bead, and update/close the bead only after verification passes.

## Goal

Replace the hand-rolled MCP JSON-RPC HTTP transport with a proper `rmcp` Streamable HTTP server while preserving syslog ingest, SQLite storage, storage guardrails, bearer auth, `/health`, and the existing seven MCP tools:

- `search_logs`
- `tail_logs`
- `get_errors`
- `list_hosts`
- `correlate_events`
- `get_stats`
- `syslog_help`

## Target Contract

The first production RMCP migration targets stateless Streamable HTTP JSON-response mode:

- `/mcp` is served by RMCP, not local `dispatch`.
- `POST /mcp` accepts JSON-RPC with `Content-Type: application/json`.
- Clients send `Accept: application/json, text/event-stream`.
- Server config uses `with_stateful_mode(false)` and `with_json_response(true)`.
- Request/response tool calls return `Content-Type: application/json`.
- Full RMCP stateful sessions are deferred to `cortex-hea.6`.

## Non-Negotiables

- Do not expand the local JSON-RPC implementation.
- Do not leave duplicate production protocol paths after migration.
- Do not move syslog ingest, DB maintenance, retention, or storage-budget logic into RMCP types.
- MCP handlers must be thin adapters over the shared typed app layer from `cortex-0zo`.
- Preserve external tool names, parameter names, defaults, caps, and response semantics unless a change is explicitly documented.
- Preserve bearer auth when configured; `/health` remains unauthenticated.
- Do not log bearer tokens, session IDs, request headers, search terms, raw log contents, or returned log text at info level.
- Configure RMCP Host/Origin validation deliberately; do not rely on CORS as DNS rebinding protection.

## Sources

- Beads epic: `cortex-hea`
- Shared app-layer prerequisite: `cortex-0zo`
- RMCP Repomix output: `/tmp/repomix/mcp-outputs/uAac1G/repomix-output.xml`, output ID `232b9bd061ad68c8`
- Official MCP Streamable HTTP spec: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
- Official RMCP SDK: https://github.com/modelcontextprotocol/rust-sdk
- Current local protocol: `src/mcp/protocol.rs`
- Current routes/auth/SSE: `src/mcp/routes.rs`
- Current tool dispatch/business logic: `src/mcp/tools.rs`
- Current tool schemas: `src/mcp/schemas.rs`

## Execution Order

1. `cortex-0zo.1` and `cortex-0zo.2`: library boundary and typed `LogService`.
2. `cortex-hea.1`: add RMCP dependency and compatibility harness.
3. `cortex-hea.2`: implement RMCP tool/server adapter over `LogService`.
4. `cortex-hea.3`: mount RMCP `StreamableHttpService` at `/mcp`.
5. `cortex-hea.4`: remove manual protocol code and migrate tests/smoke coverage.
6. `cortex-hea.6`: decide whether stateful RMCP sessions are required.
7. `cortex-hea.5`: final docs and packaging manifest cleanup.

`hea.1` may be done before the full shared app layer if it remains test-only. Production RMCP handler work should not start before `cortex-0zo.2`.

## Task 1: Add RMCP Dependency And Compatibility Harness

**Bead:** `cortex-hea.1`

**Files:**

- `Cargo.toml`
- `Cargo.lock`
- `tests/*` or `src/mcp/*_tests.rs`

**Steps:**

- [ ] Claim `cortex-hea.1`.
- [ ] Add `rmcp` with the smallest feature set that supports server handlers, tools/macros, and Streamable HTTP server transport.
- [ ] Prefer the current crates.io RMCP release if it exposes `StreamableHttpService` and `StreamableHttpServerConfig`; otherwise use the official `modelcontextprotocol/rust-sdk` git dependency.
- [ ] Add a compatibility test that starts a minimal RMCP service without replacing production `/mcp`.
- [ ] Assert stateless JSON-response mode returns HTTP 200 with `Content-Type: application/json`.
- [ ] Assert the request uses `Content-Type: application/json` and `Accept: application/json, text/event-stream`.
- [ ] Assert default or stateless-SSE mode is distinct so the selected target mode is explicit.
- [ ] Do not remove or modify `src/mcp/protocol.rs` or production `src/mcp/routes.rs` in this bead.

**Verification:**

```bash
cargo check --all-targets
cargo test rmcp -- --nocapture
cargo test --test rmcp_compat -- --nocapture
```

If the test name/location differs, run the focused RMCP compatibility test plus:

```bash
cargo test
```

**Done When:**

- RMCP compiles in this repo.
- A checked-in test documents the selected RMCP stateless JSON-response config.
- No production behavior changed.
- `bd close cortex-hea.1` has verification evidence.

## Task 2: Extract Or Confirm Shared Log Service Prerequisite

**Beads:** `cortex-0zo.1`, `cortex-0zo.2`

**Files:**

- `src/lib.rs`
- `src/app.rs` or `src/app/*`
- `src/db/*`
- `src/mcp/tools.rs`
- app-layer tests

**Steps:**

- [ ] Ensure the crate exposes reusable modules without requiring `main.rs`.
- [ ] Create or confirm a typed `LogService` that owns log-intelligence use cases.
- [ ] Move defaults, caps, timestamp normalization, severity expansion, correlation grouping, truncation semantics, and stats orchestration behind typed app APIs.
- [ ] Keep DB as persistence/query primitives; do not push MCP JSON or RMCP concerns into DB.
- [ ] Decide whether `source_ip` is part of the shared filter/output contract before RMCP cements hostname-only semantics.
- [ ] Preserve current lenient optional argument behavior where compatibility matters, or document/test intentional stricter validation.

**Verification:**

```bash
cargo test app -- --nocapture
cargo test mcp::tools -- --nocapture
cargo clippy -- -D warnings
```

**Done When:**

- Core tool behavior is testable without constructing MCP JSON-RPC.
- App-layer responses contain enough data for MCP, CLI, and API adapters.
- MCP tool code no longer owns core correlation math or severity expansion.

## Task 3: Implement RMCP Syslog Server Handler

**Bead:** `cortex-hea.2`

**Files:**

- `src/mcp.rs`
- `src/mcp/rmcp_server.rs` or `src/mcp/server.rs`
- `src/mcp/schemas.rs` if schema text is reused
- `src/mcp/tools.rs` only as temporary migration source
- RMCP handler tests

**Steps:**

- [ ] Claim `cortex-hea.2`.
- [ ] Add a cloneable RMCP server type that holds the shared `LogService` or equivalent app state.
- [ ] Define all seven tools through RMCP handler/tool abstractions.
- [ ] Preserve tool names and parameter names exactly.
- [ ] Reuse current tool descriptions unless RMCP schema generation requires small formatting changes.
- [ ] Decode parameters into typed request structs or explicit adapter structs.
- [ ] Map app validation errors to invalid-params style RMCP errors.
- [ ] Map DB/internal failures to safe client-facing errors while logging detailed internal context safely.
- [ ] Return tool content compatible with current smoke expectations unless deltas are intentional and documented.
- [ ] Avoid duplicating correlation math, severity expansion, or timestamp normalization in the RMCP adapter.

**Verification:**

```bash
cargo test mcp::schemas
cargo test mcp::tools
cargo test integration_tools_list integration_get_stats -- --nocapture
cargo clippy -- -D warnings
```

Add focused RMCP handler tests for:

- [ ] `tools/list` exposes all seven tools.
- [ ] `get_stats` works against a temp DB.
- [ ] At least one parameterized query tool works against seeded temp DB data.
- [ ] `correlate_events` validates bad `reference_time`.
- [ ] `correlate_events` validates bad `severity_min`.
- [ ] `correlate_events` preserves truncation and host grouping behavior.

**Done When:**

- RMCP can list and call all seven tools through the handler path.
- App-layer tests remain the business behavior contract.
- Handler code is adapter-only.

## Task 4: Replace Production `/mcp` Route With RMCP Service

**Bead:** `cortex-hea.3`

**Files:**

- `src/mcp/routes.rs`
- `src/mcp.rs`
- `src/main.rs`
- `src/mcp/routes_tests.rs`
- `docs/mcp/AUTH.md` if auth behavior changes
- `docs/mcp/TRANSPORT.md` if method/SSE behavior changes

**Steps:**

- [ ] Claim `cortex-hea.3`.
- [ ] Build `StreamableHttpServerConfig` with `with_stateful_mode(false)` and `with_json_response(true)`.
- [ ] Configure RMCP `allowed_hosts` for local and deployed hostnames instead of disabling Host validation.
- [ ] Configure RMCP `allowed_origins` deliberately if browser-origin requests are expected.
- [ ] Mount RMCP with the Axum pattern `Router::new().nest_service("/mcp", service)`.
- [ ] Wrap the whole RMCP service with bearer auth when a token is configured.
- [ ] Keep `/health` outside MCP auth.
- [ ] Keep `/health` response minimal and avoid returning internal DB error strings.
- [ ] Decide whether `/sse` is removed, retained as deprecated legacy discovery, or replaced later by stateful RMCP sessions.
- [ ] Preserve request body limits or replace them with equivalent RMCP/Axum layer behavior.
- [ ] Preserve trace/CORS behavior where still applicable, but do not treat CORS as security control.

**Route Tests:**

- [ ] `/health` succeeds without auth when token is configured.
- [ ] Missing bearer token on `/mcp` is rejected.
- [ ] Wrong bearer token on `/mcp` is rejected.
- [ ] Correct bearer token on `/mcp` reaches RMCP.
- [ ] `POST /mcp` requires/handles `Accept: application/json, text/event-stream`.
- [ ] `POST /mcp` requires/handles `Content-Type: application/json`.
- [ ] Unsupported `MCP-Protocol-Version` returns the RMCP/spec bad-request behavior.
- [ ] `GET /mcp` and `DELETE /mcp` match the selected stateless contract.
- [ ] `/sse` behavior matches the explicit decision.

**Verification:**

```bash
cargo test mcp::routes -- --nocapture
cargo clippy -- -D warnings
rg "dispatch\\(" src/mcp src/main.rs
```

With the server running:

```bash
curl -i -X POST http://localhost:3100/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

curl -i -X GET http://localhost:3100/mcp -H 'Accept: text/event-stream'
curl -i -X DELETE http://localhost:3100/mcp
mcp-remote http://localhost:3100/mcp --transport http-only
```

**Done When:**

- Production `/mcp` is RMCP-served.
- Local `dispatch` is not called by production routes.
- Auth and `/health` boundaries are preserved.
- Host/Origin validation is explicit.

## Task 5: Remove Manual Protocol And Migrate Tests

**Bead:** `cortex-hea.4`

**Files:**

- `src/mcp/protocol.rs`
- `src/mcp/protocol_tests.rs`
- `src/mcp/routes_tests.rs`
- `src/mcp/tools_tests.rs`
- `src/mcp.rs`
- `bin/smoke-test.sh`
- `tests/mcporter/test-tools.sh`
- `docs/mcp/TESTS.md`
- `docs/mcp/MCPORTER.md`

**Steps:**

- [ ] Claim `cortex-hea.4`.
- [ ] Delete or fully quarantine `JsonRpcRequest`, `JsonRpcResponse`, `DispatchResult`, and local protocol dispatch.
- [ ] Remove protocol tests that call dead internal dispatch paths.
- [ ] Rewrite route tests to exercise the RMCP production route/service.
- [ ] Update smoke tests to send RMCP-compatible headers where raw HTTP is used.
- [ ] Unify tool-count expectations to seven tools.
- [ ] Remove or update docs that describe raw local JSON-RPC envelopes as the live test contract.
- [ ] Replace info-level request/result logging with safe structured fields only.

**Safe Logging Contract:**

Log at info:

- tool name
- request id or generated correlation id when safe
- elapsed time
- success/failure status
- result counts
- safe error class

Do not log at info:

- bearer token values
- auth headers
- session IDs
- search query text
- raw arguments
- raw log messages
- returned tool content

**Verification:**

```bash
cargo test
cargo clippy -- -D warnings
rg "JsonRpcRequest|JsonRpcResponse|DispatchResult|tools/list|tools/call|dispatch\\(" src/mcp
bash bin/smoke-test.sh --skip-seed
bash tests/mcporter/test-tools.sh --verbose
```

**Done When:**

- No production hand-rolled MCP protocol remains.
- Tests cannot pass by exercising deleted/internal-only protocol code.
- Smoke coverage calls all seven tools through the RMCP path.

## Task 6: Decide Stateful RMCP Sessions

**Bead:** `cortex-hea.6`

**Files If Implemented:**

- `src/mcp/routes.rs`
- `src/mcp.rs`
- `src/main.rs`
- `src/config.rs` if a mode toggle is added
- `src/mcp/routes_tests.rs`
- `docs/mcp/TRANSPORT.md`
- `docs/mcp/CONNECT.md`
- `docs/mcp/AUTH.md`
- `docs/mcp/DEPLOY.md` if SSE proxy guidance changes

**Decision Work:**

- [ ] Claim `cortex-hea.6`.
- [ ] Test target clients against stateless JSON-response mode.
- [ ] Decide whether full stateful sessions are required for compatibility or conformance goals.
- [ ] If not required, close the bead with evidence and ensure docs say stateless RMCP is the supported mode.
- [ ] If required, use RMCP `LocalSessionManager` or optional `SessionStore`; do not implement custom sessions.

**If Stateful Sessions Are Implemented:**

- [ ] Enable RMCP stateful mode deliberately.
- [ ] Ensure auth wraps `POST`, `GET`, and `DELETE /mcp`.
- [ ] Do not log session IDs.
- [ ] Do not store syslog query results or log contents in a session store.
- [ ] Add CORS method support for `DELETE` if needed.
- [ ] Add reverse-proxy SSE buffering docs for `GET /mcp`.

**Stateful Verification:**

```bash
curl -i -X POST http://localhost:3100/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"curl","version":"1"}}}'

curl -i -X GET http://localhost:3100/mcp \
  -H 'Accept: text/event-stream' \
  -H 'Mcp-Session-Id: <session-id>'

curl -i -X DELETE http://localhost:3100/mcp \
  -H 'Mcp-Session-Id: <session-id>'

mcp-remote http://localhost:3100/mcp
mcp-remote http://localhost:3100/mcp --transport http-only
```

**Done When:**

- The repo either supports and tests stateful RMCP sessions, or documents stateless RMCP as the intentional supported contract with evidence.

## Task 7: Update Docs And Packaging Manifests

**Bead:** `cortex-hea.5`

**Files:**

- `README.md`
- `docs/mcp/TRANSPORT.md`
- `docs/mcp/CONNECT.md`
- `docs/mcp/TESTS.md`
- `docs/mcp/MCPORTER.md`
- `docs/plugin/CONFIG.md`
- `docs/plugin/PLUGINS.md`
- `docs/CONFIG.md`
- `docs/INVENTORY.md`
- `server.json`
- `gemini-extension.json`
- `.mcp.json`
- `.codex-plugin/plugin.json`
- `.claude-plugin/plugin.json`
- `.env.example`
- `CHANGELOG.md` if version bump is required

**Steps:**

- [ ] Claim `cortex-hea.5`.
- [ ] Replace hand-rolled JSON-RPC transport language with RMCP Streamable HTTP language.
- [ ] Distinguish stateless RMCP JSON-response mode from full stateful sessions.
- [ ] Remove direct stdio support claims unless a real stdio wrapper exists.
- [ ] Document stdio-only clients through an HTTP-to-stdio bridge such as `mcp-remote`.
- [ ] Fix `server.json` so it no longer advertises stdio for the long-running daemon.
- [ ] Fix `gemini-extension.json` so it does not launch the daemon as a command-style MCP server unless a real stdio wrapper exists.
- [ ] Keep `.mcp.json` in HTTP shape with bearer header injection.
- [ ] Reconcile token docs with code: `CORTEX_TOKEN` is primary, `CORTEX_API_TOKEN` is deprecated fallback.
- [ ] Remove unsupported or misleading `.env.example` knobs such as `CORTEX_NO_AUTH` and `CORTEX_TRANSPORT` unless code implements them.
- [ ] Reconcile all six/seven tool references to seven tools.
- [ ] Follow repo version-bump policy if version-bearing manifests change for a pushed feature branch.

**Verification:**

```bash
rg "stdio|Streamable|/sse|JSON-RPC|rmcp|CORTEX_API_TOKEN|CORTEX_TOKEN|6 MCP|7 MCP" \
  README.md docs server.json gemini-extension.json .mcp.json .codex-plugin .claude-plugin .env.example

jq empty server.json gemini-extension.json .mcp.json .codex-plugin/plugin.json .claude-plugin/plugin.json

bash bin/check-version-sync.sh
```

Run smoke docs command if the server is available:

```bash
bash bin/smoke-test.sh --skip-seed
```

**Done When:**

- Docs and manifests agree with runtime behavior.
- No stale direct-stdio claims remain.
- Transport docs say exactly which RMCP mode is implemented.
- HTTP and stdio-bridge connection examples work.

## Final Verification

Run the widest reasonable local suite:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
bash bin/check-version-sync.sh
```

With the server running:

```bash
curl -sf http://localhost:3100/health
mcp-remote http://localhost:3100/mcp --transport http-only
bash bin/smoke-test.sh --skip-seed
```

Final grep gates:

```bash
rg "JsonRpcRequest|JsonRpcResponse|DispatchResult|dispatch\\(" src/mcp
rg "stdio|JSON-RPC|/sse|Streamable|rmcp" README.md docs server.json gemini-extension.json .mcp.json
```

Expected:

- No production local MCP protocol dispatch remains.
- `/mcp` is RMCP-served.
- All seven tools list and call successfully.
- `/health` is unauthenticated and minimal.
- MCP auth applies to every active MCP method when token is configured.
- Docs and manifests match the implemented transport.

## Open Decisions

- Whether RMCP should be pinned to a crates.io release or official git revision.
- Whether `source_ip` should become a first-class app-layer filter/output field.
- Whether current lenient wrong-type parameter handling must be preserved or can become stricter under RMCP typed schemas.
- Whether `/sse` is removed during stateless RMCP migration or retained as deprecated old-transport discovery.
- Whether stateful RMCP sessions are needed after stateless RMCP works with target clients.
- Which host/origin config surface should drive RMCP `allowed_hosts` and `allowed_origins`.
