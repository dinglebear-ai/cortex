# Surface Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring CLI, REST API, and MCP to parity so every operator-facing feature is reachable from all three surfaces — no more raw SQLite queries to do routine work.

**Architecture:** The service layer already implements every feature; this plan is pure plumbing. For each gap, we wire the existing service method through: (1) a REST API route in `src/api.rs`, (2) a CLI arg struct + dispatch function in `src/cli.rs` + `src/cli/dispatch.rs`, and the MCP dispatch already exists. New CLI commands follow the established `CliMode::Local(service) / CliMode::Http(client)` branch pattern. New REST routes follow the `Query<Struct> → service.method() → respond()` pattern.

**Tech Stack:** Rust, Axum (REST), `serde`/`serde_qs` (query param deserialization), existing `SyslogService` trait, `HttpClient` (HTTP CLI mode)

---

## File Map

| File | Changes |
|------|---------|
| `src/api.rs` | Add 10 new routes: `/api/source-ips`, `/api/timeline`, `/api/patterns`, `/api/ingest-rate`, `/api/get`, `/api/errors/unaddressed`, `POST /api/errors/ack`, `POST /api/errors/unack`, `/api/notifications/recent`, `POST /api/notifications/test` |
| `src/cli.rs` | Add `SourceIps`, `Timeline`, `Patterns`, `IngestRate`, `ErrorSig` subcommands to `CliCommand`; add corresponding arg structs; add print formatters |
| `src/cli/dispatch.rs` | Add `run_source_ips`, `run_timeline`, `run_patterns`, `run_ingest_rate`, `run_error_sig_list`, `run_error_sig_ack`, `run_error_sig_unack`, `run_notifications_recent`, `run_notifications_test` |
| `src/cli/dispatch_tests.rs` | Snapshot tests for each new `into_request()` conversion |
| `src/cli/http_client.rs` | Add HTTP client methods for the new REST routes |

---

## Task 1: REST API — query gaps (`source_ips`, `timeline`, `patterns`, `ingest_rate`, `get`)

These five are read-only GET routes backed by service methods that already exist. Pattern: add a `Query<Struct>`, call `service.method()`, `respond()`.

**Files:**
- Modify: `src/api.rs`

- [ ] **Step 1: Add route registrations**

In `src/api.rs`, inside `pub fn router(...)`, add after the existing `.route("/api/stats", get(stats))` line:

```rust
.route("/api/source-ips", get(source_ips))
.route("/api/timeline", get(timeline))
.route("/api/patterns", get(patterns))
.route("/api/ingest-rate", get(ingest_rate))
.route("/api/get", get(get_log))
```

- [ ] **Step 2: Add handler structs and functions**

Add after the `stats` handler in `src/api.rs`:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIpsQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn source_ips(
    State(state): State<ApiState>,
    Query(query): Query<SourceIpsQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::ListSourceIpsRequest;
    respond(
        state
            .service
            .list_source_ips(ListSourceIpsRequest {
                limit: query.limit,
                offset: query.offset,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineQuery {
    bucket: Option<String>,
    group_by: Option<String>,
    from: Option<String>,
    to: Option<String>,
    hostname: Option<String>,
    app_name: Option<String>,
    severity_min: Option<String>,
}

async fn timeline(
    State(state): State<ApiState>,
    Query(query): Query<TimelineQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::TimelineRequest;
    respond(
        state
            .service
            .timeline(TimelineRequest {
                bucket: query.bucket,
                group_by: query.group_by,
                from: query.from,
                to: query.to,
                hostname: query.hostname,
                app_name: query.app_name,
                severity_min: query.severity_min,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternsQuery {
    from: Option<String>,
    to: Option<String>,
    hostname: Option<String>,
    app_name: Option<String>,
    severity_min: Option<String>,
    scan_limit: Option<u32>,
    top_n: Option<u32>,
}

async fn patterns(
    State(state): State<ApiState>,
    Query(query): Query<PatternsQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::PatternsRequest;
    respond(
        state
            .service
            .patterns(PatternsRequest {
                from: query.from,
                to: query.to,
                hostname: query.hostname,
                app_name: query.app_name,
                severity_min: query.severity_min,
                scan_limit: query.scan_limit,
                top_n: query.top_n,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestRateQuery {
    by_host: Option<bool>,
}

async fn ingest_rate(
    State(state): State<ApiState>,
    Query(query): Query<IngestRateQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::IngestRateRequest;
    respond(
        state
            .service
            .ingest_rate(IngestRateRequest {
                by_host: query.by_host,
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetLogQuery {
    id: i64,
}

async fn get_log(
    State(state): State<ApiState>,
    Query(query): Query<GetLogQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::GetLogRequest;
    respond(state.service.get_log(GetLogRequest { id: query.id }).await)
}
```

- [ ] **Step 3: Build to verify**

```bash
just build
```

Expected: compiles with no errors.

- [ ] **Step 4: Smoke-test the new routes**

```bash
TOKEN=$(grep ^SYSLOG_API_TOKEN ~/.syslog-mcp/.env | cut -d= -f2)
curl -s -H "Authorization: Bearer $TOKEN" "http://localhost:3100/api/source-ips?limit=5" | python3 -m json.tool | head -10
curl -s -H "Authorization: Bearer $TOKEN" "http://localhost:3100/api/ingest-rate?by_host=true" | python3 -m json.tool | head -10
curl -s -H "Authorization: Bearer $TOKEN" "http://localhost:3100/api/timeline?bucket=1h" | python3 -m json.tool | head -10
```

Expected: JSON responses with data, no `{"error":"not_found"}`.

- [ ] **Step 5: Commit**

```bash
git add src/api.rs
git commit -m "feat(api): add source-ips, timeline, patterns, ingest-rate, get routes"
```

---

## Task 2: REST API — error signature surface

Three routes: one read (GET unaddressed errors), two writes (POST ack, POST unack). Write routes take JSON body and require auth identity extraction.

**Files:**
- Modify: `src/api.rs`

- [ ] **Step 1: Add route registrations**

In `pub fn router(...)`, add after the db routes:

```rust
// --- error signatures ---
.route("/api/errors/unaddressed", get(unaddressed_errors))
.route("/api/errors/ack", post(ack_error))
.route("/api/errors/unack", post(unack_error))
```

- [ ] **Step 2: Add handler structs and functions**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnaddressedErrorsQuery {
    limit: Option<u32>,
    include_acknowledged: Option<bool>,
}

async fn unaddressed_errors(
    State(state): State<ApiState>,
    Query(query): Query<UnaddressedErrorsQuery>,
) -> impl IntoResponse {
    use syslog_mcp::app::UnaddressedErrorsRequest;
    respond(
        state
            .service
            .unaddressed_errors(UnaddressedErrorsRequest {
                limit: query.limit,
                include_acknowledged: query.include_acknowledged.unwrap_or(false),
            })
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AckErrorBody {
    signature_hash: String,
    notes: Option<String>,
}

async fn ack_error(
    State(state): State<ApiState>,
    Json(body): Json<AckErrorBody>,
) -> impl IntoResponse {
    use syslog_mcp::app::AckErrorRequest;
    // REST API uses the API token identity — no per-user JWT in bearer mode.
    let actor = "api".to_string();
    respond(
        state
            .service
            .ack_error(
                AckErrorRequest {
                    signature_hash: body.signature_hash,
                    notes: body.notes,
                },
                &actor,
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnackErrorBody {
    signature_hash: String,
    reason: Option<String>,
}

async fn unack_error(
    State(state): State<ApiState>,
    Json(body): Json<UnackErrorBody>,
) -> impl IntoResponse {
    use syslog_mcp::app::UnackErrorRequest;
    let actor = "api".to_string();
    respond(
        state
            .service
            .unack_error(
                UnackErrorRequest {
                    signature_hash: body.signature_hash,
                    reason: body.reason,
                },
                &actor,
            )
            .await,
    )
}
```

- [ ] **Step 3: Build**

```bash
just build
```

- [ ] **Step 4: Smoke-test**

```bash
TOKEN=$(grep ^SYSLOG_API_TOKEN ~/.syslog-mcp/.env | cut -d= -f2)
curl -s -H "Authorization: Bearer $TOKEN" \
  "http://localhost:3100/api/errors/unaddressed?limit=5" | python3 -m json.tool | head -20
```

Expected: `{"signatures": [...]}` (may be empty if scanner hasn't run yet — that's fine).

- [ ] **Step 5: Commit**

```bash
git add src/api.rs
git commit -m "feat(api): add error signature routes (unaddressed, ack, unack)"
```

---

## Task 3: REST API — notifications surface

Two routes: GET recent firings, POST test notification.

**Files:**
- Modify: `src/api.rs`

- [ ] **Step 1: Add route registrations**

```rust
// --- notifications ---
.route("/api/notifications/recent", get(notifications_recent))
.route("/api/notifications/test", post(notifications_test))
```

- [ ] **Step 2: Add handlers**

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationsRecentQuery {
    limit: Option<i64>,
    rule_id: Option<String>,
    since: Option<String>,
}

async fn notifications_recent(
    State(state): State<ApiState>,
    Query(query): Query<NotificationsRecentQuery>,
) -> impl IntoResponse {
    respond(
        state
            .service
            .notifications_recent(
                query.limit.unwrap_or(50).clamp(1, 500),
                query.rule_id,
                query.since,
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationsTestBody {
    body: Option<String>,
}

async fn notifications_test(
    State(state): State<ApiState>,
    Json(body): Json<NotificationsTestBody>,
) -> impl IntoResponse {
    let actor = "api".to_string();
    let apprise_url = state.notifications_config.apprise_url.clone();
    let apprise_urls = state.notifications_config.apprise_urls.clone();
    respond(
        state
            .service
            .notifications_test(
                body.body
                    .unwrap_or_else(|| "Test notification from syslog-mcp".to_string()),
                actor,
                apprise_url,
                apprise_urls,
            )
            .await
            .map(|r| serde_json::json!({ "result": r })),
    )
}
```

Note: `notifications_config` must be added to `ApiState`. Check if it already exists; if not, add it following the same pattern as `AppState` in `src/mcp/routes.rs`.

- [ ] **Step 3: Build**

```bash
just build
```

Fix any `ApiState` field missing errors by adding `notifications_config: NotificationsConfig` to the `ApiState` struct and threading it through `router()`.

- [ ] **Step 4: Smoke-test**

```bash
TOKEN=$(grep ^SYSLOG_API_TOKEN ~/.syslog-mcp/.env | cut -d= -f2)
curl -s -H "Authorization: Bearer $TOKEN" \
  "http://localhost:3100/api/notifications/recent?limit=5" | python3 -m json.tool
```

- [ ] **Step 5: Commit**

```bash
git add src/api.rs
git commit -m "feat(api): add notifications/recent and notifications/test routes"
```

---

## Task 4: HTTP client — wire new routes for `--http` CLI mode

`src/cli/http_client.rs` has one method per API route. Add methods for every route added in Tasks 1–3 so the CLI's `CliMode::Http` arm can call them.

**Files:**
- Modify: `src/cli/http_client.rs`

- [ ] **Step 1: Identify the existing pattern**

Open `src/cli/http_client.rs` and find an existing GET method (e.g. `hosts()`). It will look like:

```rust
pub async fn hosts(&self) -> anyhow::Result<ListHostsResponse> {
    self.get("/api/hosts", &()).await
}
```

And an existing POST method (e.g. `db_checkpoint()`):

```rust
pub async fn db_checkpoint(&self, req: &DbCheckpointRequest) -> anyhow::Result<DbCheckpointResponse> {
    self.post("/api/db/checkpoint", req).await
}
```

- [ ] **Step 2: Add GET methods**

```rust
pub async fn source_ips(
    &self,
    req: &syslog_mcp::app::ListSourceIpsRequest,
) -> anyhow::Result<syslog_mcp::app::ListSourceIpsResponse> {
    self.get("/api/source-ips", req).await
}

pub async fn timeline(
    &self,
    req: &syslog_mcp::app::TimelineRequest,
) -> anyhow::Result<syslog_mcp::app::TimelineResponse> {
    self.get("/api/timeline", req).await
}

pub async fn patterns(
    &self,
    req: &syslog_mcp::app::PatternsRequest,
) -> anyhow::Result<syslog_mcp::app::PatternsResponse> {
    self.get("/api/patterns", req).await
}

pub async fn ingest_rate(
    &self,
    req: &syslog_mcp::app::IngestRateRequest,
) -> anyhow::Result<syslog_mcp::app::IngestRateResponse> {
    self.get("/api/ingest-rate", req).await
}

pub async fn get_log(
    &self,
    req: &syslog_mcp::app::GetLogRequest,
) -> anyhow::Result<syslog_mcp::app::GetLogResponse> {
    self.get("/api/get", req).await
}

pub async fn unaddressed_errors(
    &self,
    req: &syslog_mcp::app::UnaddressedErrorsRequest,
) -> anyhow::Result<syslog_mcp::app::UnaddressedErrorsResponse> {
    self.get("/api/errors/unaddressed", req).await
}

pub async fn notifications_recent(
    &self,
    limit: i64,
    rule_id: Option<String>,
    since: Option<String>,
) -> anyhow::Result<serde_json::Value> {
    #[derive(serde::Serialize)]
    struct Params {
        limit: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        rule_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    }
    self.get("/api/notifications/recent", &Params { limit, rule_id, since }).await
}
```

- [ ] **Step 3: Add POST methods**

```rust
pub async fn ack_error(
    &self,
    req: &syslog_mcp::app::AckErrorRequest,
) -> anyhow::Result<syslog_mcp::app::AckErrorResponse> {
    self.post("/api/errors/ack", req).await
}

pub async fn unack_error(
    &self,
    req: &syslog_mcp::app::UnackErrorRequest,
) -> anyhow::Result<syslog_mcp::app::UnackErrorResponse> {
    self.post("/api/errors/unack", req).await
}

pub async fn notifications_test(
    &self,
    body: Option<String>,
) -> anyhow::Result<serde_json::Value> {
    #[derive(serde::Serialize)]
    struct Payload { body: Option<String> }
    self.post("/api/notifications/test", &Payload { body }).await
}
```

- [ ] **Step 4: Build**

```bash
just build
```

Fix any type mismatches — response types may need to be checked against `src/app/models.rs`.

- [ ] **Step 5: Commit**

```bash
git add src/cli/http_client.rs
git commit -m "feat(cli/http): add HTTP client methods for new API routes"
```

---

## Task 5: CLI — `syslog source-ips`, `syslog timeline`, `syslog patterns`, `syslog ingest-rate`

Add four new top-level query commands following the established arg struct + dispatch pattern.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/cli/dispatch.rs`

- [ ] **Step 1: Write failing tests in `src/cli/dispatch_tests.rs`**

```rust
#[test]
fn source_ips_args_into_request_default() {
    let args = super::super::SourceIpsArgs { limit: None, offset: None, json: false };
    let req = args.into_request();
    assert_eq!(format!("{req:?}"), "ListSourceIpsRequest { limit: None, offset: None }");
}

#[test]
fn timeline_args_into_request() {
    let args = super::super::TimelineArgs {
        bucket: Some("1h".to_string()),
        group_by: None,
        from: None,
        to: None,
        hostname: None,
        app_name: None,
        severity_min: None,
        json: false,
    };
    let req = args.into_request();
    assert_eq!(format!("{req:?}"), "TimelineRequest { bucket: Some(\"1h\"), group_by: None, from: None, to: None, hostname: None, app_name: None, severity_min: None }");
}

#[test]
fn ingest_rate_args_into_request_by_host() {
    let args = super::super::IngestRateArgs { by_host: true, json: false };
    let req = args.into_request();
    assert_eq!(format!("{req:?}"), "IngestRateRequest { by_host: Some(true) }");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
just test 2>&1 | grep -E "FAIL|error\[" | head -10
```

Expected: compile errors — `SourceIpsArgs`, `TimelineArgs`, `IngestRateArgs` not defined.

- [ ] **Step 3: Add arg structs to `src/cli.rs`**

Add to the `CliCommand` enum:
```rust
SourceIps(SourceIpsArgs),
Timeline(TimelineArgs),
Patterns(PatternsArgs),
IngestRate(IngestRateArgs),
```

Add struct definitions:
```rust
pub(crate) struct SourceIpsArgs {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub json: bool,
}

pub(crate) struct TimelineArgs {
    pub bucket: Option<String>,
    pub group_by: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub severity_min: Option<String>,
    pub json: bool,
}

pub(crate) struct PatternsArgs {
    pub from: Option<String>,
    pub to: Option<String>,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub severity_min: Option<String>,
    pub scan_limit: Option<u32>,
    pub top_n: Option<u32>,
    pub json: bool,
}

pub(crate) struct IngestRateArgs {
    pub by_host: bool,
    pub json: bool,
}
```

Add CLI parsing in the `parse_args` function (follow the pattern for `SearchArgs`). Add help strings to `print_usage()`:
```
  syslog source-ips [--limit N] [--offset N] [--json]
  syslog timeline [--bucket 1m|5m|1h|1d] [--group-by hostname|severity|app] [--host HOST] [--app APP] [--severity-min LEVEL] [--since TIME] [--until TIME] [--json]
  syslog patterns [--host HOST] [--app APP] [--severity-min LEVEL] [--since TIME] [--until TIME] [--scan-limit N] [--top-n N] [--json]
  syslog ingest-rate [--by-host] [--json]
```

- [ ] **Step 4: Add `into_request()` and print functions**

In `src/cli/dispatch.rs`, add:
```rust
impl SourceIpsArgs {
    pub(super) fn into_request(self) -> ListSourceIpsRequest {
        ListSourceIpsRequest { limit: self.limit, offset: self.offset }
    }
}

impl TimelineArgs {
    pub(super) fn into_request(self) -> TimelineRequest {
        TimelineRequest {
            bucket: self.bucket,
            group_by: self.group_by,
            from: self.from,
            to: self.to,
            hostname: self.hostname,
            app_name: self.app_name,
            severity_min: self.severity_min,
        }
    }
}

impl PatternsArgs {
    pub(super) fn into_request(self) -> PatternsRequest {
        PatternsRequest {
            from: self.from,
            to: self.to,
            hostname: self.hostname,
            app_name: self.app_name,
            severity_min: self.severity_min,
            scan_limit: self.scan_limit,
            top_n: self.top_n,
        }
    }
}

impl IngestRateArgs {
    pub(super) fn into_request(self) -> IngestRateRequest {
        IngestRateRequest { by_host: if self.by_host { Some(true) } else { None } }
    }
}

pub(super) async fn run_source_ips(mode: &CliMode, args: SourceIpsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.list_source_ips(req).await?,
        CliMode::Http(client) => http_or_cancel(client.source_ips(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    for ip in &response.source_ips {
        println!("{:<20} {}", ip.source_ip, ip.log_count);
    }
    Ok(())
}

pub(super) async fn run_timeline(mode: &CliMode, args: TimelineArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.timeline(req).await?,
        CliMode::Http(client) => http_or_cancel(client.timeline(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    for pt in &response.points {
        println!("{} {:>8}", pt.bucket, pt.count);
    }
    Ok(())
}

pub(super) async fn run_patterns(mode: &CliMode, args: PatternsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.patterns(req).await?,
        CliMode::Http(client) => http_or_cancel(client.patterns(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    println!("{} pattern(s) (scanned {} logs{})", response.patterns.len(), response.scanned,
        if response.truncated { ", truncated" } else { "" });
    for p in &response.patterns {
        println!("  {:>6}  {}", p.count, p.template);
    }
    Ok(())
}

pub(super) async fn run_ingest_rate(mode: &CliMode, args: IngestRateArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.ingest_rate(req).await?,
        CliMode::Http(client) => http_or_cancel(client.ingest_rate(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    println!("rate: {:.1} logs/sec", response.logs_per_sec);
    for h in &response.by_host {
        println!("  {:<20} {:.1} logs/sec", h.hostname, h.logs_per_sec);
    }
    Ok(())
}
```

Add arm dispatch in `src/cli.rs` `run()` match:
```rust
CliCommand::SourceIps(args) => dispatch::run_source_ips(&mode, args).await,
CliCommand::Timeline(args) => dispatch::run_timeline(&mode, args).await,
CliCommand::Patterns(args) => dispatch::run_patterns(&mode, args).await,
CliCommand::IngestRate(args) => dispatch::run_ingest_rate(&mode, args).await,
```

- [ ] **Step 5: Run tests**

```bash
just test 2>&1 | grep -E "test_.*source_ips|test_.*timeline|test_.*ingest_rate|PASSED|FAILED"
```

Expected: all three new tests pass.

- [ ] **Step 6: Smoke-test CLI**

```bash
syslog source-ips --limit 5
syslog ingest-rate --by-host
syslog timeline --bucket 1h
```

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/cli/dispatch.rs src/cli/dispatch_tests.rs
git commit -m "feat(cli): add source-ips, timeline, patterns, ingest-rate commands"
```

---

## Task 6: CLI — `syslog sig list|ack|unack` and `syslog notify recent|test`

The most critical gap: error signature management and notification testing from the shell.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/cli/dispatch.rs`
- Modify: `src/cli/dispatch_tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn sig_list_args_default() {
    let args = super::super::SigListArgs { limit: None, include_acknowledged: false, json: false };
    let req = args.into_request();
    assert_eq!(format!("{req:?}"),
        "UnaddressedErrorsRequest { limit: None, include_acknowledged: false }");
}

#[test]
fn sig_ack_args_required() {
    let args = super::super::SigAckArgs {
        signature_hash: "abc123".to_string(),
        notes: Some("arcane auto-heal bug".to_string()),
        json: false,
    };
    let req = args.into_request();
    assert_eq!(format!("{req:?}"),
        "AckErrorRequest { signature_hash: \"abc123\", notes: Some(\"arcane auto-heal bug\") }");
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
just test 2>&1 | grep -E "SigListArgs|SigAckArgs|error\[" | head -5
```

- [ ] **Step 3: Add subcommand enum and arg structs to `src/cli.rs`**

```rust
// In CliCommand enum:
Sig(SigCommand),
Notify(NotifyCommand),

// Subcommand enums:
pub(crate) enum SigCommand {
    List(SigListArgs),
    Ack(SigAckArgs),
    Unack(SigUnackArgs),
}

pub(crate) enum NotifyCommand {
    Recent(NotifyRecentArgs),
    Test(NotifyTestArgs),
}

// Arg structs:
pub(crate) struct SigListArgs {
    pub limit: Option<u32>,
    pub include_acknowledged: bool,
    pub json: bool,
}

pub(crate) struct SigAckArgs {
    pub signature_hash: String,
    pub notes: Option<String>,
    pub json: bool,
}

pub(crate) struct SigUnackArgs {
    pub signature_hash: String,
    pub reason: Option<String>,
    pub json: bool,
}

pub(crate) struct NotifyRecentArgs {
    pub limit: Option<i64>,
    pub rule_id: Option<String>,
    pub since: Option<String>,
    pub json: bool,
}

pub(crate) struct NotifyTestArgs {
    pub body: Option<String>,
    pub json: bool,
}
```

Add to `print_usage()`:
```
  syslog sig list [--include-acknowledged] [--limit N] [--json]
  syslog sig ack HASH [--notes TEXT] [--json]
  syslog sig unack HASH [--reason TEXT] [--json]
  syslog notify recent [--rule-id ID] [--since TIME] [--limit N] [--json]
  syslog notify test [--body TEXT] [--json]
```

- [ ] **Step 4: Add `into_request()` and dispatch functions**

```rust
impl SigListArgs {
    pub(super) fn into_request(self) -> UnaddressedErrorsRequest {
        UnaddressedErrorsRequest {
            limit: self.limit,
            include_acknowledged: self.include_acknowledged,
        }
    }
}

impl SigAckArgs {
    pub(super) fn into_request(self) -> AckErrorRequest {
        AckErrorRequest {
            signature_hash: self.signature_hash,
            notes: self.notes,
        }
    }
}

impl SigUnackArgs {
    pub(super) fn into_request(self) -> UnackErrorRequest {
        UnackErrorRequest {
            signature_hash: self.signature_hash,
            reason: self.reason,
        }
    }
}

pub(super) async fn run_sig_list(mode: &CliMode, args: SigListArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.unaddressed_errors(req).await?,
        CliMode::Http(client) => http_or_cancel(client.unaddressed_errors(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    if response.signatures.is_empty() {
        println!("No unaddressed error signatures.");
        return Ok(());
    }
    println!("{} signature(s):", response.signatures.len());
    for sig in &response.signatures {
        let acked = if sig.acknowledged_at.is_some() { " [acked]" } else { "" };
        println!("  {:>6}x  {}  {}{}", sig.total_count, &sig.signature_hash[..16], sig.template, acked);
        println!("         app={} host={}", sig.sample_app_name.as_deref().unwrap_or("-"), sig.sample_hostname);
    }
    Ok(())
}

pub(super) async fn run_sig_ack(mode: &CliMode, args: SigAckArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.ack_error(req, "cli").await?,
        CliMode::Http(client) => http_or_cancel(client.ack_error(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    println!("acknowledged {}", response.signature_hash);
    Ok(())
}

pub(super) async fn run_sig_unack(mode: &CliMode, args: SigUnackArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.unack_error(req, "cli").await?,
        CliMode::Http(client) => http_or_cancel(client.unack_error(&req)).await?,
    };
    if json {
        return super::print_json(&response);
    }
    println!("unacknowledged {}", response.signature_hash);
    Ok(())
}

pub(super) async fn run_notify_recent(mode: &CliMode, args: NotifyRecentArgs) -> Result<()> {
    let json = args.json;
    let limit = args.limit.unwrap_or(50).clamp(1, 500);
    let firings = match mode {
        CliMode::Local(service) => service.notifications_recent(limit, args.rule_id, args.since).await?,
        CliMode::Http(client) => http_or_cancel(client.notifications_recent(limit, args.rule_id, args.since)).await?,
    };
    if json {
        return super::print_json(&firings);
    }
    if firings.is_empty() {
        println!("No recent notification firings.");
        return Ok(());
    }
    for f in &firings {
        println!("{} rule={} host={} status={}", f.fired_at, f.rule_id, f.hostname,
            f.status_code.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string()));
    }
    Ok(())
}

pub(super) async fn run_notify_test(mode: &CliMode, args: NotifyTestArgs) -> Result<()> {
    let json = args.json;
    match mode {
        CliMode::Http(client) => {
            let result = http_or_cancel(client.notifications_test(args.body)).await?;
            if json { return super::print_json(&result); }
            println!("{result}");
        }
        CliMode::Local(service) => {
            // Local mode calls the service directly; apprise URLs come from config
            // which the service already has — no need to pass them here.
            bail!("notify test requires --http (apprise config lives in the server process)");
        }
    }
    Ok(())
}
```

Add dispatch arms in `run()`:
```rust
CliCommand::Sig(cmd) => match cmd {
    SigCommand::List(args) => dispatch::run_sig_list(&mode, args).await,
    SigCommand::Ack(args) => dispatch::run_sig_ack(&mode, args).await,
    SigCommand::Unack(args) => dispatch::run_sig_unack(&mode, args).await,
},
CliCommand::Notify(cmd) => match cmd {
    NotifyCommand::Recent(args) => dispatch::run_notify_recent(&mode, args).await,
    NotifyCommand::Test(args) => dispatch::run_notify_test(&mode, args).await,
},
```

- [ ] **Step 5: Run tests**

```bash
just test 2>&1 | grep -E "sig_list|sig_ack|PASSED|FAILED"
```

Expected: both new tests pass.

- [ ] **Step 6: End-to-end test the full workflow**

Once the error scanner has run and signatures exist:

```bash
# List unaddressed errors
syslog sig list

# Acknowledge the arcane auto-heal signature (replace HASH with actual value from list)
syslog sig ack HASH --notes "arcane v1.19.4 bug: docker-client-refresh races auto-heal context"

# Verify it no longer appears
syslog sig list

# Check recent notification firings
syslog notify recent --limit 10
```

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/cli/dispatch.rs src/cli/dispatch_tests.rs
git commit -m "feat(cli): add sig list/ack/unack and notify recent/test commands"
```

---

## Task 7: Update docs/contracts and CLAUDE.md

**Files:**
- Modify: `docs/contracts/cli-surface.md`
- Modify: `docs/contracts/mcp-actions-current.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update CLI surface contract**

In `docs/contracts/cli-surface.md`, add the new commands to the command table following existing format.

- [ ] **Step 2: Update MCP actions doc**

In `docs/contracts/mcp-actions.md`, verify `unaddressed_errors`, `ack_error`, `unack_error`, `notifications_recent`, `notifications_test` are documented. Add any missing entries.

- [ ] **Step 3: Update CLAUDE.md CLI command table**

In `CLAUDE.md`, update the CLI Commands table to include:
```
| `syslog source-ips` | List unique source IPs with log counts |
| `syslog timeline` | Log volume over time (bucketed) |
| `syslog patterns` | Recurring message patterns |
| `syslog ingest-rate` | Current ingest rate (logs/sec) |
| `syslog sig list` | List unaddressed error signatures |
| `syslog sig ack HASH` | Acknowledge/suppress an error signature |
| `syslog sig unack HASH` | Revoke an acknowledgement |
| `syslog notify recent` | Recent notification firings |
| `syslog notify test` | Send a test notification via Apprise |
```

- [ ] **Step 4: Commit**

```bash
git add docs/contracts/ CLAUDE.md
git commit -m "docs: update CLI surface and MCP action contracts for surface parity"
```

---

## Task 8: Integration test coverage

**Files:**
- Modify: `tests/test_live.sh` or add `tests/test_surface_parity.sh`

- [ ] **Step 1: Add API route smoke tests**

Add to the live smoke test:
```bash
# source-ips
assert_http_ok "GET /api/source-ips" \
  "$(curl -sf -H "Authorization: Bearer $TOKEN" "$BASE/api/source-ips?limit=3")"

# timeline
assert_http_ok "GET /api/timeline" \
  "$(curl -sf -H "Authorization: Bearer $TOKEN" "$BASE/api/timeline?bucket=1h")"

# unaddressed errors (may be empty — just check 200)
assert_http_ok "GET /api/errors/unaddressed" \
  "$(curl -sf -H "Authorization: Bearer $TOKEN" "$BASE/api/errors/unaddressed")"

# notifications recent
assert_http_ok "GET /api/notifications/recent" \
  "$(curl -sf -H "Authorization: Bearer $TOKEN" "$BASE/api/notifications/recent?limit=5")"
```

- [ ] **Step 2: Add CLI smoke tests**

```bash
syslog source-ips --limit 3 --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'source_ips' in d"
syslog ingest-rate --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'logs_per_sec' in d"
syslog sig list --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'signatures' in d"
syslog notify recent --limit 1 --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d, list)"
```

- [ ] **Step 3: Run the live tests**

```bash
just test-live
```

Expected: all assertions pass.

- [ ] **Step 4: Final commit and push**

```bash
git add tests/
git commit -m "test: integration coverage for surface parity routes and CLI commands"
git pull --rebase && git push
```

---

## Self-Review

**Spec coverage:**
- ✅ `source_ips` → REST API (Task 1) + CLI (Task 5)
- ✅ `timeline` → REST API (Task 1) + CLI (Task 5)
- ✅ `patterns` → REST API (Task 1) + CLI (Task 5)
- ✅ `ingest_rate` → REST API (Task 1) + CLI (Task 5)
- ✅ `get` (single log) → REST API (Task 1)
- ✅ `unaddressed_errors` → REST API (Task 2) + CLI (Task 6)
- ✅ `ack_error` → REST API (Task 2) + CLI (Task 6)
- ✅ `unack_error` → REST API (Task 2) + CLI (Task 6)
- ✅ `notifications_recent` → REST API (Task 3) + CLI (Task 6)
- ✅ `notifications_test` → REST API (Task 3) + CLI (Task 6)
- ✅ HTTP client methods for `--http` mode (Task 4)
- ✅ Docs updated (Task 7)
- ✅ Integration tests (Task 8)

**Out of scope (CLI-only things that are intentionally local-only):** `incident`, `ai watch/index/add`, `compose`, `setup`, `doctor`, `db backup` — these shell out to local processes or local SQLite; an HTTP analogue makes no sense and would be a security risk.

**Type consistency check:** All `into_request()` methods map to the exact struct names confirmed in `src/app/models.rs`. Response types used in `run_*` functions match the service layer's return types. The `notifications_recent` return type is `Vec<_>` (check `src/app/service.rs` to confirm field names like `fired_at`, `rule_id`, `hostname`, `status_code` before Task 6 Step 4).
