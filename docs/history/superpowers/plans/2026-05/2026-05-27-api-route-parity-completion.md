# API Route Parity Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining HTTP API parity gaps so every read action exposed on the MCP surface (and `fleet_state` on the service layer) is callable via `/api/*`, and unblock the three orphaned routes whose CLI `--http` mode is currently disabled.

**Architecture:** Add three new GET handlers to `src/api.rs` (`/api/host-state`, `/api/context`, `/api/fleet-state`), register `fleet_state` as a first-class MCP action so the new HTTP route does not introduce surface drift, and remove the stale `--http` guards from `src/cli/dispatch_ai.rs` after wiring `HttpClient` methods for `similar_incidents`, `ask_history`, and `incident_context`. No changes to the service layer; every route just decodes its `Query`, calls an existing `SyslogService` method, and re-uses the shared `respond()` helper.

**Tech Stack:** Rust 2024, axum 0.7, serde, sqlx (SQLite), rmcp, tokio, reqwest, anyhow.

---

## Background — what's actually missing

From the parity audit run at the start of this session:

| Action | MCP | HTTP | CLI | Status before this plan |
|---|---|---|---|---|
| `host_state` | yes | **no** | no | MCP-only (oversight from heartbeat post-v1 commit `afd77e4`) |
| `context` | yes | **no** | no | MCP-only (pivot-window log context) |
| `fleet_state` | **no** | **no** | no | `SyslogService::fleet_state` exists but is unexposed |
| `similar_incidents` | yes | yes | guarded | route exists, CLI `--http` rejects with `bail!` |
| `ask_history` | yes | yes | guarded | route exists, CLI `--http` rejects with `bail!` |
| `incident_context` | yes | yes | guarded | route exists, CLI `--http` rejects with `bail!` |

After this plan: every row above has working MCP + HTTP coverage, and the three orphaned routes have `HttpClient` wrappers so CLI `--http` reaches them. CLI verb additions for `host_state`, `context`, and `fleet_state` are intentionally out of scope — operator UIs (Aurora) consume the new routes directly; CLI verbs land in a follow-up.

## File Structure

**Modified:**
- `src/api.rs` — three new handlers (`host_state`, `context`, `fleet_state`), three new route entries.
- `src/api_tests.rs` — one smoke test per new route, asserting `200 OK` with a bearer token against an empty DB.
- `src/mcp/actions.rs` — register `fleet_state` in `ACTION_SPECS` (single row).
- `src/mcp/tools.rs` — add `"fleet_state" => tool_fleet_state(...)` arm + `tool_fleet_state` helper, mirroring `tool_host_state`.
- `src/cli/http_client.rs` — three new methods: `similar_incidents`, `ask_history`, `incident_context`.
- `src/cli/dispatch_ai.rs` — replace the three `bail!("...currently runs locally only; omit --http.")` arms with HTTP delegations.

**Not modified:** `src/app/service.rs`, `src/app/models.rs`. Every service method and request struct needed by this plan already exists.

---

## Task 1: `GET /api/host-state`

**Files:**
- Modify: `src/api.rs:204-262` (route registration), append new handler after `hosts` (line 415)
- Test: `src/api_tests.rs` (append at end-of-file, before any trailing `}`)

- [ ] **Step 1: Write the failing test**

Append to `src/api_tests.rs`:

```rust
#[tokio::test]
async fn host_state_returns_400_without_host_id_or_hostname() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(app, "/api/host-state", Some("secret")).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(
        value.get("error").is_some(),
        "missing error message: {value}"
    );
}

#[tokio::test]
async fn host_state_returns_404_for_unknown_host() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) =
        get_json(app, "/api/host-state?hostname=nonexistent", Some("secret")).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib host_state_returns_400_without_host_id_or_hostname host_state_returns_404_for_unknown_host
```

Expected: both FAIL with `404 NOT_FOUND` (route does not exist).

- [ ] **Step 3: Add the route registration**

In `src/api.rs`, locate the `// --- surface parity routes ---` block (around line 221) and add the new route after `.route("/api/get", get(get_log))`:

```rust
        .route("/api/host-state", get(host_state))
```

- [ ] **Step 4: Add the import for `HostStateRequest`**

In `src/api.rs`, locate the existing `use crate::app::{ ... }` import (line 16-27). Add `HostStateRequest` to the alphabetically-sorted import list (it slots between `GetLogRequest` and `IncidentContextRequest`):

```rust
use crate::app::{
    AbuseSearchRequest, AckErrorRequest, AiCheckpointsRequest, AiCorrelateLimitPolicy,
    AiCorrelateRequest, AiIncidentRequest, AiInvestigateRequest, AiLimitPolicy,
    AiParseErrorsRequest, AiPruneCheckpointsRequest, AnomaliesRequest, AskHistoryRequest,
    ClockSkewRequest, CompareRequest, CorrelateEventsRequest, DbCheckpointRequest,
    DbIntegrityRequest, DbVacuumRequest, FilterLogsRequest, GetErrorsRequest, GetLogRequest,
    HostStateRequest, IncidentContextRequest, IngestRateRequest, ListAiProjectsRequest,
    ListAiToolsRequest, ListAppsRequest, ListSessionsRequest, ListSourceIpsRequest,
    NotificationsRecentRequest, PatternsRequest, ProjectContextRequest, RequestActor,
    SearchLogsRequest, SearchSessionsRequest, ServiceError, SilentHostsRequest,
    SimilarIncidentsRequest, SyslogService, TailLogsRequest, TimelineRequest, UnackErrorRequest,
    UnaddressedErrorsRequest, UsageBlocksRequest,
};
```

- [ ] **Step 5: Add the handler**

In `src/api.rs`, append after the `get_log` handler (around line 573):

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostStateQuery {
    host_id: Option<String>,
    hostname: Option<String>,
    since: Option<String>,
    limit: Option<u32>,
}

async fn host_state(
    State(state): State<ApiState>,
    Query(query): Query<HostStateQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .host_state(HostStateRequest {
                host_id: query.host_id,
                hostname: query.hostname,
                since: query.since,
                limit: query.limit,
            })
            .await,
    )
}
```

The service already maps `"host_state requires host_id or hostname"` → `ServiceError::InvalidInput` (→ 400) and a missing host → `ServiceError::NotFound` (→ 404), so no extra mapping is needed in the handler. See `src/app/service.rs:487-525`.

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test --lib host_state_returns_400_without_host_id_or_hostname host_state_returns_404_for_unknown_host
```

Expected: both PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add src/api.rs src/api_tests.rs
rtk git commit -m "feat(api): expose host_state via GET /api/host-state

Closes the MCP-only gap on host_state by adding a thin GET handler that
deserialises into HostStateRequest and delegates to the existing service
method. 400/404 mapping is inherited from the shared respond() helper."
```

---

## Task 2: `GET /api/context`

> **Deviation note (resolved in commit `cad1349`):** during initial implementation
> the service surfaced "missing pivot" / "unknown log_id" as `Internal` errors,
> which mapped to HTTP 500. The wave-3 fix reshaped `service.context()` to
> return `ServiceError::InvalidInput` / `ServiceError::NotFound`, so the
> handler now inherits the documented 400 / 404 mapping via `respond()`.
> The snippets below already reflect that final shape — a replay should
> produce 400/404, not 500.

**Files:**
- Modify: `src/api.rs` (route registration + handler)
- Test: `src/api_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/api_tests.rs`:

```rust
#[tokio::test]
async fn context_returns_400_without_pivot() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(app, "/api/context", Some("secret")).await;
    // Service requires either log_id OR (hostname+timestamp); empty query
    // must surface as 400, not 200 with empty results.
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(value.get("error").is_some(), "missing error: {value}");
}

#[tokio::test]
async fn context_returns_404_for_unknown_log_id() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) =
        get_json(app, "/api/context?log_id=999999", Some("secret")).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib context_returns_400_without_pivot context_returns_404_for_unknown_log_id
```

Expected: both FAIL with `404 NOT_FOUND` (route not mounted).

- [ ] **Step 3: Add the route registration**

In `src/api.rs`, append after the `host-state` route added in Task 1:

```rust
        .route("/api/context", get(context))
```

- [ ] **Step 4: Add the import for `ContextRequest`**

In `src/api.rs`, extend the `use crate::app::{ ... }` block to include `ContextRequest` (alphabetically between `CompareRequest` and `CorrelateEventsRequest`):

```rust
    ClockSkewRequest, CompareRequest, ContextRequest, CorrelateEventsRequest,
    DbCheckpointRequest,
```

- [ ] **Step 5: Add the handler**

In `src/api.rs`, append after the `host_state` handler from Task 1:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextQuery {
    log_id: Option<i64>,
    hostname: Option<String>,
    timestamp: Option<String>,
    before: Option<u32>,
    after: Option<u32>,
}

async fn context(
    State(state): State<ApiState>,
    Query(query): Query<ContextQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .context(ContextRequest {
                log_id: query.log_id,
                hostname: query.hostname,
                timestamp: query.timestamp,
                before: query.before,
                after: query.after,
            })
            .await,
    )
}
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test --lib context_returns_400_without_pivot context_returns_404_for_unknown_log_id
```

Expected: both PASS. If the 400 test fails because the service surfaces "no pivot" as `Internal`, open `src/app/service.rs:1625` and confirm it returns `ServiceError::InvalidInput` for the empty case; that mapping is required for the contract to hold. If the service maps "missing pivot" to `Internal`, file a follow-up bead — do NOT paper over it in the handler.

- [ ] **Step 7: Commit**

```bash
rtk git add src/api.rs src/api_tests.rs
rtk git commit -m "feat(api): expose context via GET /api/context

Adds the pivot-window log-context primitive to the REST surface. Mirrors
the MCP tool_context dispatch — accepts log_id OR (hostname+timestamp),
delegates to service.context(), and inherits 400/404 from respond()."
```

---

## Task 3: Register `fleet_state` as an MCP action

**Files:**
- Modify: `src/mcp/actions.rs:74-324` (append a new `ActionSpec` row in the read-only section)
- Modify: `src/mcp/tools.rs:8` (extend imports), `~line 45` (dispatch arm), append a `tool_fleet_state` helper alongside `tool_host_state` at ~line 177
- Test: `src/mcp/tools.rs` already has its own integration coverage via `rmcp_server_tests.rs`; we'll add one dispatch smoke test in the existing `mcp::tools` tests module rather than spinning up an rmcp server.

- [ ] **Step 1: Confirm there is no existing fleet_state dispatch**

```bash
rtk grep -n "fleet_state" src/mcp/
```

Expected: zero matches. If matches appear, STOP and re-read the file before proceeding — the action may have already been registered in a parallel branch.

- [ ] **Step 2: Add the ActionSpec row**

In `src/mcp/actions.rs`, locate the existing `host_state` row (around line 106-111) and append a new row immediately after it:

```rust
    ActionSpec {
        name: "fleet_state",
        scope: Scope::Read,
        description: "Fleet-wide heartbeat snapshot with pressure flags",
        cost: Cost::Expensive,
    },
```

- [ ] **Step 3: Extend the tools.rs import**

In `src/mcp/tools.rs:8`, add `FleetStateRequest` to the existing `use crate::app::{ ... }` import list (alphabetically; it sorts between `FilterLogsRequest` and `GetErrorsRequest`).

- [ ] **Step 4: Add the dispatch arm**

In `src/mcp/tools.rs` around line 45 (where `"host_state" => tool_host_state(...)` lives), append:

```rust
        "fleet_state" => tool_fleet_state(state, args).await,
```

- [ ] **Step 5: Add the handler helper**

In `src/mcp/tools.rs`, append immediately after `tool_host_state` (around line 180):

```rust
async fn tool_fleet_state(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let req: FleetStateRequest = action_payload(args)?;
    Ok(serde_json::to_value(state.service.fleet_state(req).await?)?)
}
```

- [ ] **Step 6: Verify the project builds**

```bash
rtk cargo check
```

Expected: clean build, no warnings related to the new action. If `action_payload` is not in scope, copy the `use` line from `tool_host_state`'s surrounding context.

- [ ] **Step 7: Commit**

```bash
rtk git add src/mcp/actions.rs src/mcp/tools.rs
rtk git commit -m "feat(mcp): register fleet_state as a first-class MCP action

The fleet_state service method has existed since the heartbeat post-v1
work (commit afd77e4) but was never wired into ACTION_SPECS or the tool
dispatch table. Registering it now so the new /api/fleet-state route
(next commit) does not introduce surface drift."
```

---

## Task 4: `GET /api/fleet-state`

**Files:**
- Modify: `src/api.rs` (route registration + handler)
- Test: `src/api_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/api_tests.rs`:

```rust
#[tokio::test]
async fn fleet_state_returns_200_with_token_on_empty_db() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(app, "/api/fleet-state", Some("secret")).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(value.get("hosts").is_some(), "missing hosts: {value}");
    assert!(value.get("summary").is_some(), "missing summary: {value}");
}

#[tokio::test]
async fn fleet_state_accepts_include_ok_and_sort_params() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(
        app,
        "/api/fleet-state?include_ok=false&sort=freshness",
        Some("secret"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib fleet_state_returns_200_with_token_on_empty_db fleet_state_accepts_include_ok_and_sort_params
```

Expected: both FAIL with `404 NOT_FOUND`.

- [ ] **Step 3: Add the route registration**

In `src/api.rs`, append after the `/api/context` route added in Task 2:

```rust
        .route("/api/fleet-state", get(fleet_state))
```

- [ ] **Step 4: Add the import for `FleetStateRequest`**

In `src/api.rs`, extend the `use crate::app::{ ... }` block to include `FleetStateRequest` (alphabetically between `FilterLogsRequest` and `GetErrorsRequest`):

```rust
    DbIntegrityRequest, DbVacuumRequest, FilterLogsRequest, FleetStateRequest,
    GetErrorsRequest, GetLogRequest,
```

- [ ] **Step 5: Add the handler**

In `src/api.rs`, append after the `context` handler from Task 2:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FleetStateQuery {
    include_ok: Option<bool>,
    sort: Option<String>,
}

async fn fleet_state(
    State(state): State<ApiState>,
    Query(query): Query<FleetStateQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .fleet_state(FleetStateRequest {
                include_ok: query.include_ok,
                sort: query.sort,
            })
            .await,
    )
}
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test --lib fleet_state_returns_200_with_token_on_empty_db fleet_state_accepts_include_ok_and_sort_params
```

Expected: both PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add src/api.rs src/api_tests.rs
rtk git commit -m "feat(api): expose fleet_state via GET /api/fleet-state

Pairs with the new fleet_state MCP action. Returns the fleet-wide
heartbeat snapshot with pressure flags and summary counts so Aurora
operator surfaces can pull a single fleet view in one call."
```

---

## Task 5: `HttpClient` wrappers for `similar_incidents`, `ask_history`, `incident_context`

**Files:**
- Modify: `src/cli/http_client.rs` (append three methods inside the `impl HttpClient` block ending around line 633)

- [ ] **Step 1: Locate the insertion point**

Open `src/cli/http_client.rs` and find the last existing wrapper inside `impl HttpClient`. The audit found `list_apps` at line 623 returning `Result<ListAppsResponse>`. Insert the new methods immediately after it, before the helper block that starts at line 633.

- [ ] **Step 2: Confirm the imports already cover the request/response types**

```bash
rtk grep -n "SimilarIncidentsRequest\|SimilarIncidentsResponse\|AskHistoryRequest\|AskHistoryResponse\|IncidentContextRequest\|IncidentContextResponse" src/cli/http_client.rs
```

Expected: each request type appears in the existing `use` block; if any response type is missing, add it to the existing `use crate::app::{ ... }` import (the file imports request/response pairs together — pattern matches `SilentHostsRequest, SilentHostsResponse`, etc.).

- [ ] **Step 3: Add the three wrapper methods**

In `src/cli/http_client.rs`, append inside `impl HttpClient` (immediately after the `list_apps` method):

```rust
    pub async fn similar_incidents(
        &self,
        req: &SimilarIncidentsRequest,
    ) -> Result<SimilarIncidentsResponse> {
        self.get_json("/api/similar-incidents", Some(req)).await
    }

    pub async fn ask_history(
        &self,
        req: &AskHistoryRequest,
    ) -> Result<AskHistoryResponse> {
        self.get_json("/api/ai/ask-history", Some(req)).await
    }

    pub async fn incident_context(
        &self,
        req: &IncidentContextRequest,
    ) -> Result<IncidentContextResponse> {
        self.get_json("/api/incident-context", Some(req)).await
    }
```

`get_json` is the existing generic helper used by every other wrapper in this file (e.g. `silent_hosts` at line 607-609); it takes the path + an optional reference to a `Serialize` request and returns `Result<T>` decoded from JSON.

- [ ] **Step 4: Verify the file compiles**

```bash
rtk cargo check --lib
```

Expected: clean. If a response type is unknown, locate it in `src/app/models.rs` and add it to the import block at the top of `http_client.rs`.

- [ ] **Step 5: Commit**

```bash
rtk git add src/cli/http_client.rs
rtk git commit -m "feat(cli): add HttpClient wrappers for similar_incidents, ask_history, incident_context

The REST routes for these three actions have existed since the surface
parity gap closure (2026-05-22), but the CLI had no client method to
reach them — dispatch_ai.rs bailed on --http with 'currently runs
locally only'. Adding the wrappers here unblocks the guard removal in
the next commit."
```

---

## Task 6: Remove the stale `--http` guards in `dispatch_ai.rs`

**Files:**
- Modify: `src/cli/dispatch_ai.rs:389-423` (three functions: `run_ai_similar_incidents`, `run_ai_ask_history`, `run_ai_incident_context`)

- [ ] **Step 1: Inspect the existing pattern**

```bash
rtk read src/cli/dispatch_ai.rs --offset 388 --limit 36
```

Confirm the three target functions match the snippet pasted in Step 2 below. If a function has drifted (extra args, different name), STOP and reconcile before editing — do not assume the original form.

- [ ] **Step 2: Replace `run_ai_similar_incidents`**

In `src/cli/dispatch_ai.rs`, locate the function at line 389. Replace the body with:

```rust
pub(crate) async fn run_ai_similar_incidents(mode: &CliMode, args: AiSimilarArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Http(client) => client.similar_incidents(&req).await?,
        CliMode::Local(service) => service.similar_incidents(req).await?,
    };
    print_similar_incidents_response(&response, json)
}
```

- [ ] **Step 3: Replace `run_ai_ask_history`**

Replace the function at line 400 with:

```rust
pub(crate) async fn run_ai_ask_history(mode: &CliMode, args: AiAskHistoryArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Http(client) => client.ask_history(&req).await?,
        CliMode::Local(service) => service.ask_history(req).await?,
    };
    print_ask_history_response(&response, json)
}
```

- [ ] **Step 4: Replace `run_ai_incident_context`**

Replace the function at line 411 with:

```rust
pub(crate) async fn run_ai_incident_context(
    mode: &CliMode,
    args: AiIncidentContextArgs,
) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Http(client) => client.incident_context(&req).await?,
        CliMode::Local(service) => service.incident_context(req).await?,
    };
    print_incident_context_response(&response, json)
}
```

- [ ] **Step 5: Verify the build is clean and `bail!`/`anyhow` are still actually needed**

```bash
rtk cargo check --lib --bin syslog
```

Expected: no warnings about unused `bail` imports. If `anyhow::bail` is no longer used anywhere else in the file (other host-local guards at lines 312, 325, 336, 348, 362, 373, 478 still use it — confirm with grep), keep the import.

```bash
rtk grep -n "bail!" src/cli/dispatch_ai.rs
```

Expected: matches still present at the other guard sites (`ai add`, `ai watch`, `ai index`, `ai doctor`, `ai smoke-watch`, `ai watch-status`, `ai assess`).

- [ ] **Step 6: Run the full test suite**

```bash
just test
```

Expected: all tests pass. The `dispatch_tests.rs` file is large — if a test asserted the old bail message, it'll fail loudly and you'll need to delete or update it. Search for the old error strings:

```bash
rtk grep -n "currently runs locally only" src/cli/
```

Expected: zero matches after this task.

- [ ] **Step 7: Commit**

```bash
rtk git add src/cli/dispatch_ai.rs
rtk git commit -m "fix(cli): allow --http for similar_incidents, ask_history, incident_context

The bail!() guards were placeholders from when the REST routes did not
exist; they were left in place by oversight when the gap closure landed
on 2026-05-22. With the HttpClient wrappers from the previous commit,
these functions now have honest dual-mode support."
```

---

## Task 7: End-to-end smoke verification

**Files:** none modified.

- [ ] **Step 1: Run the full test suite**

```bash
just test
```

Expected: green. If anything related to scope gating or `action_names()` ordering fails because of the new `fleet_state` entry in `ACTION_SPECS`, locate the failing assertion and update it to include the new action — those tests are protecting against unintentional registry changes and need an intentional bump here.

- [ ] **Step 2: Run clippy**

```bash
just lint
```

Expected: clean. Common issue: a new `#[derive(Debug, Deserialize)]` struct without `Default` may trigger `clippy::derive_partial_eq_without_eq` if a downstream type needs it — none of the new query structs cross that line, but check the output.

- [ ] **Step 3: Build a release binary and check version output**

```bash
just build
./target/release/syslog --version
```

Expected: prints the current crate version. (This step exists because every prior parity-closure PR has hit a missing-feature flag or release-build divergence — running release once catches it before pushing.)

- [ ] **Step 4: Run the live smoke harness if a server is reachable**

```bash
just test-live
```

Expected: passes if `SYSLOG_API_TOKEN` and a running server are configured (per CLAUDE.md). Skip on machines without a configured local instance — the unit suite from Step 1 already covers route correctness.

- [ ] **Step 5: Commit any stray formatting changes from clippy/fmt**

```bash
rtk cargo fmt
rtk git diff --stat
```

If `git diff` shows changes, commit them with `chore: cargo fmt after api parity completion`. If clean, skip.

- [ ] **Step 6: Push**

```bash
rtk git pull --rebase
bd dolt push
rtk git push
rtk git status
```

Expected: `git status` reports the branch is up-to-date with origin. Per CLAUDE.md's Session Completion protocol, work is not complete until this step succeeds.

---

## Self-review notes

- **Spec coverage:** Every action from the audit's "Gaps" list has a task (`host_state` → Task 1, `context` → Task 2, `fleet_state` → Tasks 3+4, `similar_incidents`/`ask_history`/`incident_context` orphan-route wire-up → Tasks 5+6). The `get` CLI-verb gap and the `status` HTTP-shape gap are out of scope — they're CLI-only changes that don't add routes.
- **Type consistency:** `HostStateRequest`, `ContextRequest`, `FleetStateRequest`, `SimilarIncidentsRequest`, `AskHistoryRequest`, `IncidentContextRequest` and their `*Response` counterparts all exist in `src/app/models.rs` today — no struct definitions need to be added.
- **No placeholders:** every step has either runnable code, a runnable command with expected output, or both.
- **DRY:** each handler is a thin `Query` → `Request` → `service.method().await` → `respond()` shim that exactly mirrors the existing handlers in `src/api.rs` (`silent_hosts`, `clock_skew`, `anomalies`). The plan does not invent a new pattern.
- **TDD:** every route task starts with a failing test and ends with a green run; the MCP action registration (Task 3) is checked by `cargo check` because it has no behaviour worth a unit test on its own — the integration coverage from Task 4's HTTP smoke tests will exercise the new service path end-to-end through the REST surface.
