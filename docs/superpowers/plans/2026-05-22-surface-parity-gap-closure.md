# Surface Parity Gap Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the gap so every MCP action also has a matching REST API endpoint and CLI subcommand — full surface parity across all three caller-facing surfaces.

**Architecture:** The service layer (`SyslogService`) already exposes all 39 MCP actions as `pub async fn` methods. The gap is purely at the surface layer: REST handlers in `src/api.rs` and CLI parsers in `src/cli.rs` / `src/cli/commands/`. Each gap is closed by adding (a) a thin axum handler that calls the existing service method and (b) a CLI parser + dispatch handler + run wiring + HTTP client method. No service-layer changes required.

**Tech Stack:** Rust 2021, axum (REST), tokio, rusqlite via service layer, `Serialize`/`Deserialize` request/response structs (already defined in `src/app/models.rs`). CLI is a hand-rolled `FlagCursor` parser (per Q-C1 split pattern). Tests use the existing harness in `src/api_tests.rs` (`test_state`, `test_router`, `get_json`, `post_json`) and `src/cli_tests.rs` (`strings(...)` helper + `CliCommand::parse`).

---

## CRITICAL FACTS FROM CODEBASE RESEARCH

These were verified against the actual source on 2026-05-22 and override any earlier assumptions in this plan:

**Test helpers — exact API:**
- `test_state(token: Option<String>) -> (ApiState, Arc<DbPool>, TempDir)` at `src/api_tests.rs:28`
- `test_router(state: ApiState) -> axum::Router` at `src/api_tests.rs:19` (wraps router + MockConnectInfo)
- `get_json(app, uri, token) -> (StatusCode, Value)` at `src/api_tests.rs:102`
- `post_json(app, uri, body, token) -> (StatusCode, Value)` at `src/api_tests.rs:834`
- CLI tests use `strings(&["foo", "bar"])` (helper at `src/cli_tests.rs:1`) then `CliCommand::parse(strings(&[...]))`

**Service method names — they don't always match the MCP action name:**
- MCP action `abuse_incidents` → `service.list_ai_incidents(AiIncidentRequest { ... })`
- MCP action `abuse_investigate` → `service.investigate_ai_incidents(AiInvestigateRequest { ... })`
- MCP actions `compose_status` / `compose_doctor` do NOT go through SyslogService at all — they build a `ComposeService` and call `.status()` / `.doctor()` inside `spawn_blocking`. See `src/mcp/tools.rs:412-433` for the pattern (`compose_status()` helper).

**`terms: Vec<String>` requires `serde_qs::axum::QsQuery`, not `axum::extract::Query`:**
- `serde_urlencoded` (used by axum's default `Query`) cannot deserialize `Vec<String>` from repeated params.
- `AiIncidentRequest` and `AiInvestigateRequest` both have `#[serde(default)] pub terms: Vec<String>`.
- Existing `ai_abuse` handler at `src/api.rs:684` already uses `QsQuery` exactly for this reason — mirror it.

**Three CLI files (not two) must be updated per new subcommand:**
- `src/cli.rs::CliCommand::parse()` — parse dispatch (top-level "foo" => parse_foo)
- `src/cli/dispatch.rs` — `run_foo()` async handler + `into_request()` impl on Args
- `src/cli/run.rs` — `run()` match that routes `CliCommand::Foo(args) => dispatch::run_foo(&mode, args).await`. **Missing this means commands compile but hit catch-all error at runtime.**

**HTTP client methods** (for `--http` mode) live on a struct in `src/cli/http_client.rs`. Each new CLI command needs one new method there (or the `CliMode::Http` arm fails to compile).

**Imports cleanup:** Commit `8a2057d` removed `AiIncidentRequest`/`AiInvestigateRequest` from `src/api.rs` imports as "unused". These will need re-adding when the new handlers go in.

**Style: `pub mod` not `pub(crate) mod`** in `src/cli/commands/mod.rs` — match existing extractions (`pub mod notify; pub mod sig;`).

**`deny_unknown_fields` policy:** Original handlers use it (`SearchQuery`, `TailQuery`, etc.); surface-parity handlers added later (`SourceIpsQuery`, `TimelineQuery`, `PatternsQuery`, `IngestRateQuery`) **do not**. The plan applies `deny_unknown_fields` to new Query structs — this is a tightening beyond surface-parity precedent. POST bodies follow the `ack_error` pattern (no `deny_unknown_fields`).

---

## Gap Inventory (from session 2026-05-22 audit)

**REST API missing 12 endpoints:**
1. `abuse_incidents` → no `/api/ai/incidents`
2. `abuse_investigate` → no `/api/ai/investigate`
3. `anomalies` → no `/api/anomalies`
4. `apps` → no `/api/apps`
5. `ask_history` → no `/api/ai/ask-history`
6. `clock_skew` → no `/api/clock-skew`
7. `compare` → no `/api/compare`
8. `compose_doctor` → no `/api/compose/doctor`
9. `compose_status` → no `/api/compose/status`
10. `incident_context` → no `/api/incident-context`
11. `silent_hosts` → no `/api/silent-hosts`
12. `similar_incidents` → no `/api/similar-incidents`

**CLI missing 5 subcommands:** `anomalies`, `apps`, `clock-skew`, `compare`, `silent-hosts`.

---

## File Structure

**REST API — extending `src/api.rs`:**
- Add 12 handler functions following the existing thin-wrapper pattern (lines 306-633).
- Add 12 `.route(...)` calls to `router()` inside the `// --- surface parity routes ---` block (line ~229).
- Add `*Query` / `*Body` structs with `#[derive(Debug, Deserialize)]` and `#[serde(deny_unknown_fields)]` on Query structs (POST bodies omit it, matching `ack_error`).
- For handlers consuming structs with `Vec<String>` fields (AI incidents/investigate), use `serde_qs::axum::QsQuery` not `Query`.

**CLI — extending `src/cli/commands/`:**
- Create `src/cli/commands/silent_hosts.rs`, `clock_skew.rs`, `anomalies.rs`, `compare.rs`, `apps.rs`.
- Wire 5 new modules into `src/cli/commands/mod.rs` using `pub mod foo; pub use foo::parse_foo;`.
- Add 5 variants to `CliCommand` enum in `src/cli/args.rs` plus matching `*Args` structs.
- Add 5 dispatch arms in `src/cli.rs::CliCommand::parse()`.
- Add 5 `into_request()` impls + 5 `run_*` async handlers in `src/cli/dispatch.rs`.
- Add 5 routing arms in `src/cli/run.rs::run()` mapping `CliCommand::Foo` → `dispatch::run_foo`.
- Add 5 HTTP client methods in `src/cli/http_client.rs` (one per command).

**Mode allowlist:** Add 5 kebab-case strings to the `Mode::parse` match in `src/main.rs:341-363`.

**Tests:**
- Extend `src/api_tests.rs` — one test per new REST route using `test_state`/`test_router`/`get_json`/`post_json`.
- Extend `src/cli_tests.rs` — one parser test per new CLI subcommand using `CliCommand::parse(strings(&[...]))`.

**Docs:**
- Update `README.md` — REST API table and CLI commands table.
- Update `tests/test_live.sh` — add smoke checks for the 12 new REST routes.

---

## Task 1: REST `/api/silent-hosts` endpoint

**Files:**
- Modify: `src/api.rs` (add Query struct + handler near line ~640, add route in `router()` at line ~239)
- Modify: `src/api_tests.rs` (add test)

- [ ] **Step 1: Write the failing test**

Append to `src/api_tests.rs`:

```rust
#[tokio::test]
async fn silent_hosts_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(
        app,
        "/api/silent-hosts?silent_minutes=60",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("silent_minutes").is_some(), "missing silent_minutes: {value}");
    assert!(value.get("hosts").is_some(), "missing hosts: {value}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test silent_hosts_returns_200_with_token -- --nocapture`
Expected: FAIL — route returns 404.

- [ ] **Step 3: Add query struct and handler to src/api.rs**

After the `unack_error` handler (around line 610), add:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SilentHostsQuery {
    silent_minutes: Option<u32>,
}

async fn silent_hosts(
    State(state): State<ApiState>,
    Query(query): Query<SilentHostsQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .silent_hosts(SilentHostsRequest {
                silent_minutes: query.silent_minutes,
            })
            .await,
    )
}
```

Add `SilentHostsRequest` to the existing `use crate::app::{...}` block at the top of the file.

- [ ] **Step 4: Add route to router()**

Inside the `// --- surface parity routes ---` block (after line ~239 where `notifications_test` is registered), add:

```rust
.route("/api/silent-hosts", get(silent_hosts))
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test silent_hosts_returns_200_with_token -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/silent-hosts (surface parity)"
```

---

## Task 2: REST `/api/clock-skew` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn clock_skew_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(app, "/api/clock-skew", Some("secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("hosts").is_some(), "missing hosts: {value}");
}
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo test clock_skew_returns_200_with_token -- --nocapture`

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockSkewQuery {
    since: Option<String>,
}

async fn clock_skew(
    State(state): State<ApiState>,
    Query(query): Query<ClockSkewQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .clock_skew(ClockSkewRequest { since: query.since })
            .await,
    )
}
```

Add `ClockSkewRequest` to imports.

- [ ] **Step 4: Add route**

```rust
.route("/api/clock-skew", get(clock_skew))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/clock-skew (surface parity)"
```

---

## Task 3: REST `/api/anomalies` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn anomalies_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(
        app,
        "/api/anomalies?recent_minutes=15&baseline_minutes=360",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("hosts").is_some(), "missing hosts: {value}");
    assert!(value.get("recent_minutes").is_some(), "missing recent_minutes: {value}");
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnomaliesQuery {
    recent_minutes: Option<u32>,
    baseline_minutes: Option<u32>,
}

async fn anomalies(
    State(state): State<ApiState>,
    Query(query): Query<AnomaliesQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .anomalies(AnomaliesRequest {
                recent_minutes: query.recent_minutes,
                baseline_minutes: query.baseline_minutes,
            })
            .await,
    )
}
```

Add `AnomaliesRequest` to imports.

- [ ] **Step 4: Add route**

```rust
.route("/api/anomalies", get(anomalies))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/anomalies (surface parity)"
```

---

## Task 4: REST `/api/compare` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT:** `CompareRequest` has FOUR required `String` fields: `a_from`, `a_to`, `b_from`, `b_to` (`src/app/models.rs:1310-1315`). The Query struct mirrors this.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn compare_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(
        app,
        "/api/compare?a_from=2026-05-20T00:00:00Z&a_to=2026-05-20T23:59:59Z&b_from=2026-05-21T00:00:00Z&b_to=2026-05-21T23:59:59Z",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("a").is_some(), "missing a: {value}");
    assert!(value.get("b").is_some(), "missing b: {value}");
    assert!(value.get("delta_total_logs").is_some(), "missing delta_total_logs: {value}");
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareQuery {
    a_from: String,
    a_to: String,
    b_from: String,
    b_to: String,
}

async fn compare(
    State(state): State<ApiState>,
    Query(query): Query<CompareQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .compare(CompareRequest {
                a_from: query.a_from,
                a_to: query.a_to,
                b_from: query.b_from,
                b_to: query.b_to,
            })
            .await,
    )
}
```

Add `CompareRequest` to imports.

- [ ] **Step 4: Add route**

```rust
.route("/api/compare", get(compare))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/compare (surface parity)"
```

---

## Task 5: REST `/api/apps` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT (CORRECTED FROM EARLIER STUB):** `ListAppsRequest` at `src/app/models.rs:897-905` has these fields:
- `hostname: Option<String>`
- `from: Option<String>` (time-range filter, lower bound)
- `to: Option<String>` (time-range filter, upper bound)
- `limit: Option<u32>`
- `offset: Option<u32>` (pagination)

There is NO `source_ip` field. Mirror exactly.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn apps_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, value) = get_json(app, "/api/apps?limit=50", Some("secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("apps").is_some(), "missing apps: {value}");
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppsQuery {
    hostname: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn apps(
    State(state): State<ApiState>,
    Query(query): Query<AppsQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .list_apps(ListAppsRequest {
                hostname: query.hostname,
                from: query.from,
                to: query.to,
                limit: query.limit,
                offset: query.offset,
            })
            .await,
    )
}
```

Add `ListAppsRequest` to imports.

- [ ] **Step 4: Add route**

```rust
.route("/api/apps", get(apps))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/apps (surface parity)"
```

---

## Task 6: REST `/api/similar-incidents` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT (CORRECTED FROM EARLIER STUB):** `SimilarIncidentsRequest` at `src/app/models.rs:1503-1514`:
- `query: String` (REQUIRED — search text)
- `hostname: Option<String>`
- `app_name: Option<String>`
- `severity_min: Option<String>`
- `from: Option<String>`
- `to: Option<String>`
- `window_minutes: Option<u32>`
- `limit: Option<u32>`

There is NO `reference_time` field — the URI must use `?query=` not `?reference_time=`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn similar_incidents_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(
        app,
        "/api/similar-incidents?query=disk%20full&window_minutes=30",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimilarIncidentsQuery {
    query: String,
    hostname: Option<String>,
    app_name: Option<String>,
    severity_min: Option<String>,
    from: Option<String>,
    to: Option<String>,
    window_minutes: Option<u32>,
    limit: Option<u32>,
}

async fn similar_incidents(
    State(state): State<ApiState>,
    Query(q): Query<SimilarIncidentsQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .similar_incidents(SimilarIncidentsRequest {
                query: q.query,
                hostname: q.hostname,
                app_name: q.app_name,
                severity_min: q.severity_min,
                from: q.from,
                to: q.to,
                window_minutes: q.window_minutes,
                limit: q.limit,
            })
            .await,
    )
}
```

- [ ] **Step 4: Add route**

```rust
.route("/api/similar-incidents", get(similar_incidents))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/similar-incidents (surface parity)"
```

---

## Task 7: REST `/api/incident-context` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT:** `IncidentContextRequest` at `src/app/models.rs:1620-1629`:
- `from: String` (REQUIRED)
- `to: String` (REQUIRED)
- `hostname: Option<String>`
- `app_name: Option<String>`
- `query: Option<String>`
- `severity_min: Option<String>`
- `limit: Option<u32>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn incident_context_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(
        app,
        "/api/incident-context?from=2026-05-21T11:00:00Z&to=2026-05-21T13:00:00Z",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentContextQuery {
    from: String,
    to: String,
    hostname: Option<String>,
    app_name: Option<String>,
    query: Option<String>,
    severity_min: Option<String>,
    limit: Option<u32>,
}

async fn incident_context(
    State(state): State<ApiState>,
    Query(q): Query<IncidentContextQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .incident_context(IncidentContextRequest {
                from: q.from,
                to: q.to,
                hostname: q.hostname,
                app_name: q.app_name,
                query: q.query,
                severity_min: q.severity_min,
                limit: q.limit,
            })
            .await,
    )
}
```

- [ ] **Step 4: Add route**

```rust
.route("/api/incident-context", get(incident_context))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/incident-context (surface parity)"
```

---

## Task 8: REST `/api/ai/ask-history` endpoint

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT:** `AskHistoryRequest` at `src/app/models.rs:1587-1595`:
- `query: String` (REQUIRED)
- `hostname: Option<String>`
- `app_name: Option<String>`
- `from: Option<String>`
- `to: Option<String>`
- `limit: Option<u32>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn ai_ask_history_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(
        app,
        "/api/ai/ask-history?query=ssh%20key%20rotation",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
```

- [ ] **Step 2: Run test — expect FAIL**

- [ ] **Step 3: Add handler**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AskHistoryQuery {
    query: String,
    hostname: Option<String>,
    app_name: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
}

async fn ai_ask_history(
    State(state): State<ApiState>,
    Query(q): Query<AskHistoryQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .ask_history(AskHistoryRequest {
                query: q.query,
                hostname: q.hostname,
                app_name: q.app_name,
                from: q.from,
                to: q.to,
                limit: q.limit,
            })
            .await,
    )
}
```

- [ ] **Step 4: Add route**

```rust
.route("/api/ai/ask-history", get(ai_ask_history))
```

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add GET /api/ai/ask-history (surface parity)"
```

---

## Task 9: REST `/api/ai/incidents` and `/api/ai/investigate` endpoints

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT (CRITICAL CORRECTION):** Both structs use `terms: Vec<String>` which **cannot** be deserialized from a URL query string via `axum::extract::Query` (it uses `serde_urlencoded`). Use `serde_qs::axum::QsQuery` — exactly like `ai_abuse` at `src/api.rs:684`.

Actual field sets:
- `AiIncidentRequest` (`src/app/models.rs:368-377`): `project`, `tool`, `from`, `to`, `limit`, `window_minutes`, `terms: Vec<String>`
- `AiInvestigateRequest` (`src/app/models.rs:429-439`): `project`, `tool`, `from`, `to`, `limit`, `window_minutes`, `correlation_window_minutes`, `terms: Vec<String>`

Service methods (note the different names):
- MCP `abuse_incidents` → `service.list_ai_incidents(req)`
- MCP `abuse_investigate` → `service.investigate_ai_incidents(req)`

Both endpoints are **GET** (they are search/filter calls, not single-incident triggers — there is no `incident_id`/`dry_run` field).

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn ai_incidents_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(app, "/api/ai/incidents?limit=10", Some("secret")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ai_investigate_returns_200_with_token() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(
        app,
        "/api/ai/investigate?window_minutes=60&correlation_window_minutes=30",
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
```

- [ ] **Step 2: Run tests — expect FAIL (404)**

- [ ] **Step 3: Add handlers**

```rust
#[derive(Debug, Deserialize)]
struct AiIncidentsQuery {
    project: Option<String>,
    tool: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
    window_minutes: Option<u32>,
    #[serde(default)]
    terms: Vec<String>,
}

async fn ai_incidents(
    State(state): State<ApiState>,
    QsQuery(q): QsQuery<AiIncidentsQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .list_ai_incidents(AiIncidentRequest {
                project: q.project,
                tool: q.tool,
                from: q.from,
                to: q.to,
                limit: q.limit,
                window_minutes: q.window_minutes,
                terms: q.terms,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
struct AiInvestigateQuery {
    project: Option<String>,
    tool: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
    window_minutes: Option<u32>,
    correlation_window_minutes: Option<u32>,
    #[serde(default)]
    terms: Vec<String>,
}

async fn ai_investigate(
    State(state): State<ApiState>,
    QsQuery(q): QsQuery<AiInvestigateQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .investigate_ai_incidents(AiInvestigateRequest {
                project: q.project,
                tool: q.tool,
                from: q.from,
                to: q.to,
                limit: q.limit,
                window_minutes: q.window_minutes,
                correlation_window_minutes: q.correlation_window_minutes,
                terms: q.terms,
            })
            .await,
    )
}
```

Add to imports at top of `src/api.rs`:
- Re-add `AiIncidentRequest, AiInvestigateRequest` to the `use crate::app::{...}` block (removed by commit `8a2057d`).
- Confirm `serde_qs::axum::QsQuery` is already imported (it must be — `ai_abuse` uses it).

**Note:** No `deny_unknown_fields` on `Vec<String>` query structs — `serde_qs` is more permissive and `deny_unknown_fields` interacts poorly with array-bracketed params.

- [ ] **Step 4: Add routes**

```rust
.route("/api/ai/incidents", get(ai_incidents))
.route("/api/ai/investigate", get(ai_investigate))
```

Note: both are **GET** since they're query/filter endpoints — not POST.

- [ ] **Step 5: Run tests — expect PASS**

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add ai_incidents and ai_investigate endpoints (surface parity)"
```

---

## Task 10: REST `/api/compose/status` and `/api/compose/doctor` endpoints

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`

**FACT:** These do NOT go through `SyslogService`. The pattern at `src/mcp/tools.rs:412-433`:

```rust
let service = crate::compose::ComposeService::new(
    crate::compose::CliDockerInspect,
    crate::compose::ProcessRunner,
    crate::compose::ComposeDefaults::default(),
);
let status = tokio::task::spawn_blocking(move || {
    let _permit = permit;
    service.status(&crate::compose::ComposeTarget::default())
})
.await??;
```

The semaphore (`permit`) limits concurrent Docker invocations. For REST, either:
- (a) Mirror the helper from `tools.rs` directly (preferred — DRY).
- (b) Re-implement the pattern inline.

**Recommended:** Read `tool_compose_status` body in `src/mcp/tools.rs:412-433` and mirror it verbatim in the new REST handler. The semaphore acquisition pattern (`state.compose_permit`) needs to be replicated — verify `ApiState` already carries a compose permit; if not, plumb one through.

- [ ] **Step 1: Read the existing compose handler in tools.rs**

Run: `sed -n '395,440p' src/mcp/tools.rs`

Identify:
- Whether `ComposeService::new` takes the same three args
- How the semaphore is acquired (from `AppState`? `ApiState`?)
- The exact `.await??` (double-question-mark) pattern for spawn_blocking + Result-returning service

- [ ] **Step 2: Write the failing tests**

```rust
#[tokio::test]
async fn compose_status_route_exists() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(app, "/api/compose/status", Some("secret")).await;
    // In test env, Docker is absent — accept 200 OR 500/503 (degraded).
    // The point is to reject 404 (route missing).
    assert_ne!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn compose_doctor_route_exists() {
    let (state, _pool, _dir) = test_state(Some("secret".into()));
    let app = test_router(state);
    let (status, _value) = get_json(app, "/api/compose/doctor", Some("secret")).await;
    assert_ne!(status, StatusCode::NOT_FOUND);
}
```

- [ ] **Step 3: Run tests — expect FAIL (404)**

- [ ] **Step 4: Add handlers**

Following the pattern from `tool_compose_status` and `tool_compose_doctor`:

```rust
async fn compose_status(State(state): State<ApiState>) -> impl IntoResponse {
    let permit_result = state.compose_permit.clone().acquire_owned().await;
    let permit = match permit_result {
        Ok(p) => p,
        Err(e) => {
            return respond::<()>(Err(ServiceError::Internal(anyhow::anyhow!(
                "compose permit closed: {e}"
            ))));
        }
    };

    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let service = crate::compose::ComposeService::new(
            crate::compose::CliDockerInspect,
            crate::compose::ProcessRunner,
            crate::compose::ComposeDefaults::default(),
        );
        service.status(&crate::compose::ComposeTarget::default())
    })
    .await;

    match result {
        Ok(Ok(value)) => respond::<crate::compose::ComposeStatus>(Ok(value)),
        Ok(Err(e)) => respond::<crate::compose::ComposeStatus>(Err(ServiceError::Internal(
            anyhow::anyhow!("compose status: {e}"),
        ))),
        Err(e) => respond::<crate::compose::ComposeStatus>(Err(ServiceError::Internal(
            anyhow::anyhow!("compose status join: {e}"),
        ))),
    }
}

async fn compose_doctor(State(state): State<ApiState>) -> impl IntoResponse {
    // Same shape as compose_status but call service.doctor(...) instead.
    // Mirror tool_compose_doctor body from src/mcp/tools.rs verbatim.
    // ...
}
```

**IMPORTANT:** If `ApiState` does NOT already carry a `compose_permit: Arc<Semaphore>`, either:
- Add the field to `ApiState` (mirror `AppState`'s field for consistency), wire it through `RuntimeCore` construction; OR
- Call the `compose_status()` helper from `src/mcp/tools.rs` directly if it can be made `pub(crate)`.

Decide based on whether `state.compose_permit` exists. Read `src/api.rs:1-200` (ApiState struct + constructor) to confirm.

- [ ] **Step 5: Add routes**

```rust
.route("/api/compose/status", get(compose_status))
.route("/api/compose/doctor", get(compose_doctor))
```

- [ ] **Step 6: Run tests — expect non-404 status**

- [ ] **Step 7: Commit**

```bash
git add src/api.rs src/api_tests.rs
git commit -m "feat(api): add compose status and doctor endpoints (surface parity)"
```

---

## Task 11: CLI `syslog silent-hosts` subcommand

**Files (all five must be touched):**
- Create: `src/cli/commands/silent_hosts.rs`
- Modify: `src/cli/commands/mod.rs`
- Modify: `src/cli/args.rs`
- Modify: `src/cli.rs` (`CliCommand::parse` dispatch)
- Modify: `src/cli/dispatch.rs` (`run_silent_hosts` + `into_request()` impl)
- Modify: `src/cli/run.rs` (`run()` match — **DO NOT FORGET**)
- Modify: `src/cli/http_client.rs` (add `silent_hosts(&req)` method)
- Modify: `src/cli_tests.rs` (parser test)

- [ ] **Step 1: Add CliCommand variant and Args struct**

In `src/cli/args.rs`, add to the `CliCommand` enum:

```rust
SilentHosts(SilentHostsArgs),
```

Add the args struct:

```rust
#[derive(Debug, Default)]
pub struct SilentHostsArgs {
    pub silent_minutes: Option<u32>,
    pub json: bool,
}
```

Add an `into_request()` impl in `src/cli/dispatch.rs` near other surface-parity request conversions:

```rust
impl SilentHostsArgs {
    pub(super) fn into_request(self) -> SilentHostsRequest {
        SilentHostsRequest { silent_minutes: self.silent_minutes }
    }
}
```

- [ ] **Step 2: Write the failing parser test**

In `src/cli_tests.rs`:

```rust
#[test]
fn parse_silent_hosts_with_minutes_and_json() {
    let cmd = CliCommand::parse(strings(&["silent-hosts", "--silent-minutes", "120", "--json"]))
        .expect("parse silent-hosts");
    match cmd {
        CliCommand::SilentHosts(args) => {
            assert_eq!(args.silent_minutes, Some(120));
            assert!(args.json);
        }
        other => panic!("expected SilentHosts, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

Run: `cargo test parse_silent_hosts_with_minutes_and_json -- --nocapture`

- [ ] **Step 4: Create parser module**

Create `src/cli/commands/silent_hosts.rs`:

```rust
//! Parse function for `syslog silent-hosts`.

use anyhow::{bail, Result};

use super::super::args::{CliCommand, SilentHostsArgs};
use super::super::{parse_u32_flag, FlagCursor};

pub(crate) fn parse_silent_hosts(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SilentHostsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--silent-minutes")? {
            parsed.silent_minutes = Some(parse_u32_flag("--silent-minutes", v)?);
        } else {
            bail!("unknown silent-hosts option: {arg}");
        }
    }
    Ok(CliCommand::SilentHosts(parsed))
}
```

- [ ] **Step 5: Wire module into mod.rs**

In `src/cli/commands/mod.rs`, add (matching existing `pub mod` style):

```rust
pub mod silent_hosts;
pub use silent_hosts::parse_silent_hosts;
```

- [ ] **Step 6: Add parse dispatch in src/cli.rs**

In `CliCommand::parse()` match, add:

```rust
"silent-hosts" => parse_silent_hosts(rest),
```

Import `parse_silent_hosts` at the top of `src/cli.rs` from `crate::cli::commands` (or wherever the other extracted parsers are imported — match the `parse_sig` / `parse_notify` import pattern).

- [ ] **Step 7: Add run_silent_hosts in dispatch.rs**

In `src/cli/dispatch.rs`, following the `run_source_ips` / `run_timeline` pattern:

```rust
pub(super) async fn run_silent_hosts(mode: &CliMode, args: SilentHostsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.silent_hosts(req).await?,
        CliMode::Http(client) => http_or_cancel(client.silent_hosts(&req)).await?,
    };
    print_silent_hosts_response(&response, json)
}
```

Add a `print_silent_hosts_response` formatter in the same file or in `src/cli.rs` near other `print_*_response` helpers. Table format: `hostname | last_seen | silent_for_secs | log_count`. If `json` is true, call `print_json(&response)`.

- [ ] **Step 8: Add CliCommand routing arm in run.rs**

In `src/cli/run.rs::run()` match (lines 46-120), add:

```rust
CliCommand::SilentHosts(args) => dispatch::run_silent_hosts(&mode, args).await,
```

**This step is what makes the command actually execute. Without it, the parser succeeds but the runner falls into the catch-all error.**

- [ ] **Step 9: Add HTTP client method in http_client.rs**

In `src/cli/http_client.rs`, add a method following the existing surface-parity pattern (`source_ips`, `timeline`, etc.):

```rust
pub async fn silent_hosts(&self, req: &SilentHostsRequest) -> Result<SilentHostsResponse> {
    let mut url = self.endpoint("/api/silent-hosts")?;
    if let Some(m) = req.silent_minutes {
        url.query_pairs_mut().append_pair("silent_minutes", &m.to_string());
    }
    self.get_json(url).await
}
```

(Adjust the method signature and import based on the actual `HttpClient` shape — read one of the existing methods first.)

- [ ] **Step 10: Run parser test — expect PASS**

Run: `cargo test parse_silent_hosts_with_minutes_and_json -- --nocapture`

- [ ] **Step 11: Verify full build**

Run: `cargo build && cargo clippy -- -D warnings`

- [ ] **Step 12: Commit**

```bash
git add src/cli/commands/silent_hosts.rs src/cli/commands/mod.rs src/cli/args.rs src/cli.rs src/cli/dispatch.rs src/cli/run.rs src/cli/http_client.rs src/cli_tests.rs
git commit -m "feat(cli): add 'syslog silent-hosts' command (surface parity)"
```

---

## Task 12: CLI `syslog clock-skew` subcommand

**Files:** mirror Task 11's eight-file pattern. Module: `src/cli/commands/clock_skew.rs`.

- [ ] **Step 1: Add CliCommand variant + Args + into_request()**

In `src/cli/args.rs`:

```rust
ClockSkew(ClockSkewArgs),
```

```rust
#[derive(Debug, Default)]
pub struct ClockSkewArgs {
    pub since: Option<String>,
    pub json: bool,
}
```

In `src/cli/dispatch.rs`:

```rust
impl ClockSkewArgs {
    pub(super) fn into_request(self) -> ClockSkewRequest {
        ClockSkewRequest { since: self.since }
    }
}
```

- [ ] **Step 2: Write the failing parser test**

```rust
#[test]
fn parse_clock_skew_with_since() {
    let cmd = CliCommand::parse(strings(&["clock-skew", "--since", "2026-05-20T00:00:00Z"]))
        .expect("parse clock-skew");
    match cmd {
        CliCommand::ClockSkew(args) => {
            assert_eq!(args.since.as_deref(), Some("2026-05-20T00:00:00Z"));
            assert!(!args.json);
        }
        other => panic!("expected ClockSkew, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

- [ ] **Step 4: Create parser**

`src/cli/commands/clock_skew.rs`:

```rust
//! Parse function for `syslog clock-skew`.

use anyhow::{bail, Result};

use super::super::args::{ClockSkewArgs, CliCommand};
use super::super::FlagCursor;

pub(crate) fn parse_clock_skew(args: &[String]) -> Result<CliCommand> {
    let mut parsed = ClockSkewArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(v.to_owned());
        } else {
            bail!("unknown clock-skew option: {arg}");
        }
    }
    Ok(CliCommand::ClockSkew(parsed))
}
```

- [ ] **Step 5: Wire into mod.rs**

```rust
pub mod clock_skew;
pub use clock_skew::parse_clock_skew;
```

- [ ] **Step 6: Add parse dispatch in src/cli.rs**

```rust
"clock-skew" => parse_clock_skew(rest),
```

- [ ] **Step 7: Add run_clock_skew in dispatch.rs**

Mirror Task 11. Format: `hostname | samples | avg_skew_secs | max_skew_secs`.

- [ ] **Step 8: Add routing arm in run.rs**

```rust
CliCommand::ClockSkew(args) => dispatch::run_clock_skew(&mode, args).await,
```

- [ ] **Step 9: Add HTTP client method**

```rust
pub async fn clock_skew(&self, req: &ClockSkewRequest) -> Result<ClockSkewResponse> {
    let mut url = self.endpoint("/api/clock-skew")?;
    if let Some(ref s) = req.since {
        url.query_pairs_mut().append_pair("since", s);
    }
    self.get_json(url).await
}
```

- [ ] **Step 10: Run test — expect PASS, verify build**

Run: `cargo test parse_clock_skew_with_since -- --nocapture && cargo build && cargo clippy -- -D warnings`

- [ ] **Step 11: Commit**

```bash
git add src/cli/commands/clock_skew.rs src/cli/commands/mod.rs src/cli/args.rs src/cli.rs src/cli/dispatch.rs src/cli/run.rs src/cli/http_client.rs src/cli_tests.rs
git commit -m "feat(cli): add 'syslog clock-skew' command (surface parity)"
```

---

## Task 13: CLI `syslog anomalies` subcommand

**Files:** mirror Task 11.

- [ ] **Step 1: Add CliCommand variant + Args + into_request()**

```rust
Anomalies(AnomaliesArgs),
```

```rust
#[derive(Debug, Default)]
pub struct AnomaliesArgs {
    pub recent_minutes: Option<u32>,
    pub baseline_minutes: Option<u32>,
    pub json: bool,
}
```

```rust
impl AnomaliesArgs {
    pub(super) fn into_request(self) -> AnomaliesRequest {
        AnomaliesRequest {
            recent_minutes: self.recent_minutes,
            baseline_minutes: self.baseline_minutes,
        }
    }
}
```

- [ ] **Step 2: Write the failing parser test**

```rust
#[test]
fn parse_anomalies_with_windows() {
    let cmd = CliCommand::parse(strings(&[
        "anomalies", "--recent-minutes", "30", "--baseline-minutes", "720",
    ]))
    .expect("parse anomalies");
    match cmd {
        CliCommand::Anomalies(args) => {
            assert_eq!(args.recent_minutes, Some(30));
            assert_eq!(args.baseline_minutes, Some(720));
        }
        other => panic!("expected Anomalies, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

- [ ] **Step 4: Create parser**

`src/cli/commands/anomalies.rs`:

```rust
//! Parse function for `syslog anomalies`.

use anyhow::{bail, Result};

use super::super::args::{AnomaliesArgs, CliCommand};
use super::super::{parse_u32_flag, FlagCursor};

pub(crate) fn parse_anomalies(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AnomaliesArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--recent-minutes")? {
            parsed.recent_minutes = Some(parse_u32_flag("--recent-minutes", v)?);
        } else if let Some(v) = flags.match_value(&arg, "--baseline-minutes")? {
            parsed.baseline_minutes = Some(parse_u32_flag("--baseline-minutes", v)?);
        } else {
            bail!("unknown anomalies option: {arg}");
        }
    }
    Ok(CliCommand::Anomalies(parsed))
}
```

- [ ] **Step 5: Wire mod.rs + cli.rs parse dispatch**

```rust
pub mod anomalies;
pub use anomalies::parse_anomalies;
```

```rust
"anomalies" => parse_anomalies(rest),
```

- [ ] **Step 6: dispatch.rs run handler**

Table format: `hostname | recent_per_min | baseline_per_min | ratio | z_score`.

- [ ] **Step 7: run.rs routing arm**

```rust
CliCommand::Anomalies(args) => dispatch::run_anomalies(&mode, args).await,
```

- [ ] **Step 8: HTTP client method**

```rust
pub async fn anomalies(&self, req: &AnomaliesRequest) -> Result<AnomaliesResponse> {
    let mut url = self.endpoint("/api/anomalies")?;
    if let Some(m) = req.recent_minutes {
        url.query_pairs_mut().append_pair("recent_minutes", &m.to_string());
    }
    if let Some(m) = req.baseline_minutes {
        url.query_pairs_mut().append_pair("baseline_minutes", &m.to_string());
    }
    self.get_json(url).await
}
```

- [ ] **Step 9: Run test, build, commit**

```bash
cargo test parse_anomalies_with_windows -- --nocapture && cargo build && cargo clippy -- -D warnings
git add src/cli/commands/anomalies.rs src/cli/commands/mod.rs src/cli/args.rs src/cli.rs src/cli/dispatch.rs src/cli/run.rs src/cli/http_client.rs src/cli_tests.rs
git commit -m "feat(cli): add 'syslog anomalies' command (surface parity)"
```

---

## Task 14: CLI `syslog compare` subcommand

**Files:** mirror Task 11.

**FACT:** `CompareRequest` has FOUR REQUIRED fields (`a_from`, `a_to`, `b_from`, `b_to`), all `String`. The CLI must validate that all four flags were provided and return a clear error message if any is missing — not silently submit empty strings.

- [ ] **Step 1: Add CliCommand variant + Args + into_request()**

```rust
Compare(CompareArgs),
```

```rust
#[derive(Debug, Default)]
pub struct CompareArgs {
    pub a_from: Option<String>,
    pub a_to: Option<String>,
    pub b_from: Option<String>,
    pub b_to: Option<String>,
    pub json: bool,
}
```

The `into_request` here is fallible — return `Result<CompareRequest>` and use `ok_or_else` to surface missing flags:

```rust
impl CompareArgs {
    pub(super) fn into_request(self) -> Result<CompareRequest> {
        Ok(CompareRequest {
            a_from: self.a_from.ok_or_else(|| anyhow::anyhow!("--a-from is required"))?,
            a_to: self.a_to.ok_or_else(|| anyhow::anyhow!("--a-to is required"))?,
            b_from: self.b_from.ok_or_else(|| anyhow::anyhow!("--b-from is required"))?,
            b_to: self.b_to.ok_or_else(|| anyhow::anyhow!("--b-to is required"))?,
        })
    }
}
```

- [ ] **Step 2: Write the failing parser test**

```rust
#[test]
fn parse_compare_with_ranges() {
    let cmd = CliCommand::parse(strings(&[
        "compare",
        "--a-from", "2026-05-20T00:00:00Z",
        "--a-to",   "2026-05-20T23:59:59Z",
        "--b-from", "2026-05-21T00:00:00Z",
        "--b-to",   "2026-05-21T23:59:59Z",
    ]))
    .expect("parse compare");
    match cmd {
        CliCommand::Compare(args) => {
            assert_eq!(args.a_from.as_deref(), Some("2026-05-20T00:00:00Z"));
            assert_eq!(args.b_to.as_deref(), Some("2026-05-21T23:59:59Z"));
        }
        other => panic!("expected Compare, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

- [ ] **Step 4: Create parser**

`src/cli/commands/compare.rs`:

```rust
//! Parse function for `syslog compare`.

use anyhow::{bail, Result};

use super::super::args::{CliCommand, CompareArgs};
use super::super::FlagCursor;

pub(crate) fn parse_compare(args: &[String]) -> Result<CliCommand> {
    let mut parsed = CompareArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--a-from")? {
            parsed.a_from = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--a-to")? {
            parsed.a_to = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--b-from")? {
            parsed.b_from = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--b-to")? {
            parsed.b_to = Some(v.to_owned());
        } else {
            bail!("unknown compare option: {arg}");
        }
    }
    Ok(CliCommand::Compare(parsed))
}
```

- [ ] **Step 5: Wire mod.rs + cli.rs parse dispatch**

```rust
pub mod compare;
pub use compare::parse_compare;
```

```rust
"compare" => parse_compare(rest),
```

- [ ] **Step 6: dispatch.rs run handler**

```rust
pub(super) async fn run_compare(mode: &CliMode, args: CompareArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request()?;
    let response = match mode {
        CliMode::Local(service) => service.compare(req).await?,
        CliMode::Http(client) => http_or_cancel(client.compare(&req)).await?,
    };
    print_compare_response(&response, json)
}
```

Print summary: `A summary`, `B summary`, `delta` line.

- [ ] **Step 7: run.rs routing arm**

```rust
CliCommand::Compare(args) => dispatch::run_compare(&mode, args).await,
```

- [ ] **Step 8: HTTP client method**

```rust
pub async fn compare(&self, req: &CompareRequest) -> Result<CompareResponse> {
    let mut url = self.endpoint("/api/compare")?;
    url.query_pairs_mut()
        .append_pair("a_from", &req.a_from)
        .append_pair("a_to", &req.a_to)
        .append_pair("b_from", &req.b_from)
        .append_pair("b_to", &req.b_to);
    self.get_json(url).await
}
```

- [ ] **Step 9: Run test, build, commit**

```bash
cargo test parse_compare_with_ranges -- --nocapture && cargo build && cargo clippy -- -D warnings
git add src/cli/commands/compare.rs src/cli/commands/mod.rs src/cli/args.rs src/cli.rs src/cli/dispatch.rs src/cli/run.rs src/cli/http_client.rs src/cli_tests.rs
git commit -m "feat(cli): add 'syslog compare' command (surface parity)"
```

---

## Task 15: CLI `syslog apps` subcommand

**Files:** mirror Task 11.

**FACT:** `ListAppsRequest` fields — `hostname`, `from`, `to`, `limit`, `offset`. NO `source_ip`. NO `app_name`.

- [ ] **Step 1: Add CliCommand variant + Args + into_request()**

```rust
Apps(AppsArgs),
```

```rust
#[derive(Debug, Default)]
pub struct AppsArgs {
    pub hostname: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub json: bool,
}
```

```rust
impl AppsArgs {
    pub(super) fn into_request(self) -> ListAppsRequest {
        ListAppsRequest {
            hostname: self.hostname,
            from: self.from,
            to: self.to,
            limit: self.limit,
            offset: self.offset,
        }
    }
}
```

- [ ] **Step 2: Write the failing parser test**

```rust
#[test]
fn parse_apps_with_hostname_limit() {
    let cmd = CliCommand::parse(strings(&[
        "apps", "--host", "devhost", "--limit", "50",
    ]))
    .expect("parse apps");
    match cmd {
        CliCommand::Apps(args) => {
            assert_eq!(args.hostname.as_deref(), Some("devhost"));
            assert_eq!(args.limit, Some(50));
        }
        other => panic!("expected Apps, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

- [ ] **Step 4: Create parser**

`src/cli/commands/apps.rs`:

```rust
//! Parse function for `syslog apps`.

use anyhow::{bail, Result};

use super::super::args::{AppsArgs, CliCommand};
use super::super::{parse_u32_flag, FlagCursor};

pub(crate) fn parse_apps(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AppsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--host")? {
            parsed.hostname = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.from = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--until")? {
            parsed.to = Some(v.to_owned());
        } else if let Some(v) = flags.match_value(&arg, "--limit")? {
            parsed.limit = Some(parse_u32_flag("--limit", v)?);
        } else if let Some(v) = flags.match_value(&arg, "--offset")? {
            parsed.offset = Some(parse_u32_flag("--offset", v)?);
        } else {
            bail!("unknown apps option: {arg}");
        }
    }
    Ok(CliCommand::Apps(parsed))
}
```

- [ ] **Step 5: Wire mod.rs + cli.rs parse dispatch**

```rust
pub mod apps;
pub use apps::parse_apps;
```

```rust
"apps" => parse_apps(rest),
```

- [ ] **Step 6: dispatch.rs run handler**

Table format: `app_name | log_count | first_seen | last_seen` (or whatever fields `ListAppsResponse` exposes — read `src/app/models.rs` for the response type).

- [ ] **Step 7: run.rs routing arm**

```rust
CliCommand::Apps(args) => dispatch::run_apps(&mode, args).await,
```

- [ ] **Step 8: HTTP client method**

```rust
pub async fn list_apps(&self, req: &ListAppsRequest) -> Result<ListAppsResponse> {
    let mut url = self.endpoint("/api/apps")?;
    if let Some(ref h) = req.hostname {
        url.query_pairs_mut().append_pair("hostname", h);
    }
    if let Some(ref f) = req.from {
        url.query_pairs_mut().append_pair("from", f);
    }
    if let Some(ref t) = req.to {
        url.query_pairs_mut().append_pair("to", t);
    }
    if let Some(l) = req.limit {
        url.query_pairs_mut().append_pair("limit", &l.to_string());
    }
    if let Some(o) = req.offset {
        url.query_pairs_mut().append_pair("offset", &o.to_string());
    }
    self.get_json(url).await
}
```

- [ ] **Step 9: Run test, build, commit**

```bash
cargo test parse_apps_with_hostname_limit -- --nocapture && cargo build && cargo clippy -- -D warnings
git add src/cli/commands/apps.rs src/cli/commands/mod.rs src/cli/args.rs src/cli.rs src/cli/dispatch.rs src/cli/run.rs src/cli/http_client.rs src/cli_tests.rs
git commit -m "feat(cli): add 'syslog apps' command (surface parity)"
```

---

## Task 16: Mode allowlist update for new CLI subcommands

**Files:**
- Modify: `src/main.rs` (the `Mode::parse` match at lines 341-363)
- Modify: `src/main_tests.rs`

- [ ] **Step 1: Confirm the allowlist location**

Run: `sed -n '335,370p' src/main.rs`

You should see a `match command.as_str()` block with kebab-case subcommand strings (`"search"`, `"tail"`, `"errors"`, ... `"sig"`, `"notify"`).

- [ ] **Step 2: Write failing test**

In `src/main_tests.rs`:

```rust
#[test]
fn mode_parse_accepts_new_surface_parity_subcommands() {
    for cmd in &["silent-hosts", "clock-skew", "anomalies", "compare", "apps"] {
        let result = Mode::parse(&[cmd.to_string()]);
        assert!(result.is_ok(), "Mode::parse rejected '{cmd}': {result:?}");
    }
}
```

- [ ] **Step 3: Run test — expect FAIL**

Run: `cargo test mode_parse_accepts_new_surface_parity_subcommands -- --nocapture`

- [ ] **Step 4: Add subcommands to allowlist**

Add the five new kebab-case strings (`"silent-hosts"`, `"clock-skew"`, `"anomalies"`, `"compare"`, `"apps"`) to the appropriate `|` chain in the `Mode::parse` match arm.

- [ ] **Step 5: Run test — expect PASS**

- [ ] **Step 6: Verify full build**

Run: `cargo build && cargo clippy -- -D warnings && cargo test`

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/main_tests.rs
git commit -m "fix(cli): add surface-parity commands to Mode::parse allowlist"
```

---

## Task 17: Update README and live smoke test

**Files:**
- Modify: `README.md`
- Modify: `tests/test_live.sh`

- [ ] **Step 1: Find REST API table in README**

Run: `grep -n "^## REST API\|^### REST\|/api/" README.md | head -20`

- [ ] **Step 2: Add 12 new REST routes to README table**

Add entries for:

```
GET  /api/silent-hosts
GET  /api/clock-skew
GET  /api/anomalies
GET  /api/compare
GET  /api/apps
GET  /api/similar-incidents
GET  /api/incident-context
GET  /api/ai/ask-history
GET  /api/ai/incidents
GET  /api/ai/investigate
GET  /api/compose/status
GET  /api/compose/doctor
```

Follow the existing row format. Include one-line descriptions.

- [ ] **Step 3: Find CLI commands section in README**

Run: `grep -n "^## CLI\|^## Commands\|syslog search" README.md | head -10`

- [ ] **Step 4: Add 5 new CLI subcommands to README**

```
syslog silent-hosts [--silent-minutes N] [--json]
syslog clock-skew   [--since RFC3339] [--json]
syslog anomalies    [--recent-minutes N] [--baseline-minutes N] [--json]
syslog compare      --a-from RFC3339 --a-to RFC3339 --b-from RFC3339 --b-to RFC3339 [--json]
syslog apps         [--host H] [--since RFC3339] [--until RFC3339] [--limit N] [--offset N] [--json]
```

- [ ] **Step 5: Add smoke-test entries to tests/test_live.sh**

Read the file first: `cat tests/test_live.sh | head -50`

Follow the existing pattern (likely a `curl` call per endpoint with assertions on status code or jq-extracted fields). Add a section like:

```bash
echo "--- surface-parity smoke (new routes) ---"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/silent-hosts?silent_minutes=60" >/dev/null && echo "  silent-hosts: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/clock-skew" >/dev/null && echo "  clock-skew: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/anomalies" >/dev/null && echo "  anomalies: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/apps?limit=10" >/dev/null && echo "  apps: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/similar-incidents?query=test&limit=1" >/dev/null && echo "  similar-incidents: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/incident-context?from=2026-01-01T00:00:00Z&to=2026-12-31T23:59:59Z" >/dev/null && echo "  incident-context: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/ai/ask-history?query=test" >/dev/null && echo "  ai/ask-history: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/ai/incidents?limit=5" >/dev/null && echo "  ai/incidents: ok"
curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/api/ai/investigate?limit=5" >/dev/null && echo "  ai/investigate: ok"
# compose endpoints may 5xx in test env if Docker is unreachable — accept non-404
curl -sS -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $TOKEN" "$BASE/api/compose/status" | grep -v "^404$" >/dev/null && echo "  compose/status: route exists"
curl -sS -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $TOKEN" "$BASE/api/compose/doctor" | grep -v "^404$" >/dev/null && echo "  compose/doctor: route exists"
# `/api/compare` requires 4 time bounds; just check route exists by sending a 400
curl -sS -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $TOKEN" "$BASE/api/compare?a_from=x&a_to=y&b_from=z&b_to=w" | grep -v "^404$" >/dev/null && echo "  compare: route exists"
```

- [ ] **Step 6: Commit**

```bash
git add README.md tests/test_live.sh
git commit -m "docs: list new surface-parity REST + CLI surfaces in README and smoke test"
```

---

## Task 18: Final integration verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass, no regressions.

- [ ] **Step 2: Run clippy with strict lints**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3: Run formatter check**

Run: `cargo fmt -- --check`
Expected: clean.

- [ ] **Step 4: Verify cargo-deny still passes**

Run: `cargo deny check` (if `cargo-deny` is installed locally; otherwise rely on CI).

- [ ] **Step 5: Sanity-check parity claim with a quick grep**

Run:

```bash
echo "MCP actions:"; grep -oP 'name: "[a-z_]+"' src/mcp/actions.rs | grep -oP '"[a-z_]+"' | sort > /tmp/mcp.txt
echo "REST routes:"; grep -oP '\.route\("/api[^"]+' src/api.rs | sed 's|\.route("/api/||' | sort > /tmp/rest.txt
diff /tmp/mcp.txt /tmp/rest.txt
```

Verify every MCP action has a corresponding REST route (with the known name mappings: `list_ai_*` → `/api/ai/*`, `abuse_incidents` → `/api/ai/incidents`, etc.).

- [ ] **Step 6: Commit anything left**

```bash
git status
git add . && git commit -m "chore: final surface-parity cleanups" || echo "nothing to commit"
```

---

## Self-Review Checklist

- [ ] All 12 REST routes added with matching tests using the correct test helpers (`test_state`, `test_router`, `get_json`, `post_json`)
- [ ] All 5 CLI subcommands added with parser tests using `CliCommand::parse(strings(&[...]))`
- [ ] **Each CLI command touches 8 files**: `commands/foo.rs` (new), `commands/mod.rs`, `args.rs`, `cli.rs`, `dispatch.rs`, `run.rs`, `http_client.rs`, `cli_tests.rs` — verify all 8 are in each commit
- [ ] `Mode::parse` allowlist updated (Task 16) — without this, the new CLI commands are silently rejected
- [ ] README updated with both new REST routes and new CLI commands
- [ ] `tests/test_live.sh` updated with smoke checks for the 12 new REST routes
- [ ] AI endpoints (`/api/ai/incidents`, `/api/ai/investigate`) use `QsQuery` not `Query` (required for `Vec<String>` deserialization)
- [ ] Service method names match reality: `list_ai_incidents`, `investigate_ai_incidents` (NOT `abuse_incidents`/`abuse_investigate`)
- [ ] Compose REST handlers mirror `tool_compose_status` from `src/mcp/tools.rs:412-433` — semaphore + spawn_blocking + ComposeService::new
- [ ] `cargo build`, `cargo clippy -- -D warnings`, `cargo test`, `cargo fmt -- --check` all pass
