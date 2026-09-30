# MCP Apps Query Widget Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the server-side MCP Apps contract for a simple syslog log-query widget.

**Architecture:** Keep the existing single `syslog` action-dispatch tool. Add MCP Apps metadata to that tool, expose a `ui://syslog/query-widget` HTML resource through the existing rmcp resource handlers, and return structured JSON alongside existing text content from tool calls.

**Tech Stack:** Rust, `rmcp` 1.7, Axum test router, serde_json, SQLite-backed syslog service tests.

---

## File Structure

- Modify: `src/mcp/rmcp_server.rs`
  - Add constants for `QUERY_WIDGET_RESOURCE_URI` and `MCP_APP_HTML_MIME_TYPE`.
  - Add `query_widget_resource()` beside `schema_resource()`.
  - Add `query_widget_html_contents()` for `resources/read`.
  - Add `syslog_tool_meta()` and attach it in `rmcp_tool_from_json()`.
  - Change `tool_result_from_json()` to include `structured_content` while preserving existing pretty JSON text content.
- Create: `src/mcp/ui/query_widget.html`
  - Minimal valid placeholder HTML for this first contract slice.
- Modify: `src/mcp/rmcp_server_tests.rs`
  - Extend `tools/list` coverage to assert `_meta.ui.resourceUri`.
  - Extend `resources/list` and `resources/read` coverage for the query widget.
  - Extend tool-call coverage to assert `structuredContent`.
  - Keep mounted-policy auth failure tests passing.

---

### Task 1: Add Failing Tool Metadata and Structured Content Tests

**Files:**
- Modify: `src/mcp/rmcp_server_tests.rs`

- [ ] **Step 1: Keep `content_json()` on text JSON and add a structured helper**

Replace the current helper:

```rust
fn content_json(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}
```

with:

```rust
fn content_json(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

fn structured_json(response: &Value) -> &Value {
    &response["result"]["structuredContent"]
}
```

- [ ] **Step 2: Update `rmcp_tools_list_exposes_one_action_tool`**

Replace the assertions after `let names...` with:

```rust
    assert_eq!(names, vec!["syslog"]);
    assert_eq!(tools[0]["inputSchema"]["required"], json!(["action"]));
    assert_eq!(
        tools[0]["_meta"]["ui"]["resourceUri"],
        super::QUERY_WIDGET_RESOURCE_URI
    );
    assert_eq!(tools[0]["_meta"]["ui"]["visibility"], json!(["model", "app"]));
```

- [ ] **Step 3: Add a structuredContent assertion to the seeded search test**

In `rmcp_search_logs_works_against_seeded_data`, after:

```rust
    assert_eq!(result["logs"][0]["hostname"], "host-a");
```

add:

```rust
    let structured = structured_json(&response);
    assert_eq!(structured["count"], 1);
    assert_eq!(structured["logs"][0]["message"], "disk full");
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("\"message\": \"disk full\"")),
        "text content should remain readable JSON; response: {response}"
    );
```

- [ ] **Step 4: Run the focused tests and verify they fail**

Run:

```bash
cargo test rmcp_tools_list_exposes_one_action_tool
cargo test rmcp_search_logs_works_against_seeded_data
```

Expected: FAIL because `_meta.ui.resourceUri` and `structuredContent` are not implemented yet.

---

### Task 2: Implement Tool Metadata and Structured Tool Results

**Files:**
- Modify: `src/mcp/rmcp_server.rs`

- [ ] **Step 1: Import rmcp `Meta`**

Change the `rmcp::model` import list from:

```rust
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo, Tool,
```

to:

```rust
        Meta, ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo, Tool,
```

- [ ] **Step 2: Add the query-widget URI constant**

Below:

```rust
const SCHEMA_RESOURCE_URI: &str = "syslog://schema/mcp-tool";
```

add:

```rust
pub(super) const QUERY_WIDGET_RESOURCE_URI: &str = "ui://syslog/query-widget";
```

- [ ] **Step 3: Add `syslog_tool_meta()`**

Below `schema_resource()` add:

```rust
fn syslog_tool_meta() -> Meta {
    let mut meta = Map::new();
    meta.insert(
        "ui".to_string(),
        serde_json::json!({
            "resourceUri": QUERY_WIDGET_RESOURCE_URI,
            "visibility": ["model", "app"],
        }),
    );
    Meta(meta)
}
```

- [ ] **Step 4: Attach metadata in `rmcp_tool_from_json()`**

Replace:

```rust
    Ok(Tool::new_with_raw(
        Cow::Owned(name.to_string()),
        description,
        Arc::new(input_schema),
    ))
```

with:

```rust
    let tool = Tool::new_with_raw(
        Cow::Owned(name.to_string()),
        description,
        Arc::new(input_schema),
    );

    Ok(if name == "syslog" {
        tool.with_meta(syslog_tool_meta())
    } else {
        tool
    })
```

- [ ] **Step 5: Preserve text and add structuredContent in `tool_result_from_json()`**

Replace the final `Ok(...)` in `tool_result_from_json()`:

```rust
    Ok(CallToolResult::success(vec![Content::text(text)]))
```

with:

```rust
    Ok(CallToolResult {
        content: vec![Content::text(text)],
        structured_content: Some(value),
        is_error: Some(false),
        meta: None,
    })
```

- [ ] **Step 6: Run the focused tests and verify they pass**

Run:

```bash
cargo test rmcp_tools_list_exposes_one_action_tool
cargo test rmcp_search_logs_works_against_seeded_data
```

Expected: PASS.

---

### Task 3: Add Failing Resource List and Resource Read Tests

**Files:**
- Modify: `src/mcp/rmcp_server_tests.rs`

- [ ] **Step 1: Update `mounted_policy_with_auth_context_permits_schema_resources`**

Replace:

```rust
    let resources = response["result"]["resources"].as_array().unwrap();
    assert_eq!(
        resources[0]["uri"],
        super::SCHEMA_RESOURCE_URI,
        "resources/list should expose schema resource; response: {response}"
    );
```

with:

```rust
    let resources = response["result"]["resources"].as_array().unwrap();
    let uris: Vec<&str> = resources
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&super::SCHEMA_RESOURCE_URI),
        "resources/list should expose schema resource; response: {response}"
    );
    assert!(
        uris.contains(&super::QUERY_WIDGET_RESOURCE_URI),
        "resources/list should expose query widget resource; response: {response}"
    );
```

- [ ] **Step 2: Add a new read test for the query widget**

Below `mounted_policy_with_auth_context_permits_schema_resources`, add:

```rust
#[tokio::test]
async fn mounted_policy_with_auth_context_permits_query_widget_resource() {
    let (state, _pool, _dir) = mounted_state();
    let auth = auth_ctx_with_scopes(vec![]);
    let router = rmcp_router_with_auth(state, auth);

    let (status, response) = post_rmcp(
        router,
        jsonrpc_request(
            83,
            "resources/read",
            Some(json!({"uri": super::QUERY_WIDGET_RESOURCE_URI})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["result"]["contents"][0]["uri"],
        super::QUERY_WIDGET_RESOURCE_URI
    );
    assert_eq!(
        response["result"]["contents"][0]["mimeType"],
        super::MCP_APP_HTML_MIME_TYPE
    );
    assert!(
        response["result"]["contents"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("data-syslog-query-widget")),
        "resources/read should return query widget HTML; response: {response}"
    );
}
```

- [ ] **Step 3: Run the resource tests and verify they fail**

Run:

```bash
cargo test mounted_policy_with_auth_context_permits_schema_resources
cargo test mounted_policy_with_auth_context_permits_query_widget_resource
```

Expected: FAIL because `QUERY_WIDGET_RESOURCE_URI`, `MCP_APP_HTML_MIME_TYPE`, and query-widget resource handling are not implemented yet.

---

### Task 4: Implement the Query Widget Resource

**Files:**
- Modify: `src/mcp/rmcp_server.rs`
- Create: `src/mcp/ui/query_widget.html`

- [ ] **Step 1: Add HTML MIME constant**

Below:

```rust
pub(super) const QUERY_WIDGET_RESOURCE_URI: &str = "ui://syslog/query-widget";
```

add:

```rust
pub(super) const MCP_APP_HTML_MIME_TYPE: &str = "text/html;profile=mcp-app";
```

- [ ] **Step 2: Add `query_widget_resource()`**

Below `schema_resource()` add:

```rust
fn query_widget_resource() -> Resource {
    Resource::new(
        RawResource::new(QUERY_WIDGET_RESOURCE_URI, "syslog query widget")
            .with_title("Syslog Query")
            .with_description("Interactive MCP Apps widget for querying syslog-mcp logs")
            .with_mime_type(MCP_APP_HTML_MIME_TYPE),
        None,
    )
}
```

- [ ] **Step 3: Return both resources from `list_resources()`**

Replace:

```rust
            resources: vec![schema_resource()],
```

with:

```rust
            resources: vec![schema_resource(), query_widget_resource()],
```

- [ ] **Step 4: Add the resource HTML file**

Create `src/mcp/ui/query_widget.html` with:

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Syslog Query</title>
</head>
<body data-syslog-query-widget>
  <form aria-label="Syslog query">
    <label>
      Query
      <input name="query" type="search" autocomplete="off" placeholder="error AND sshd">
    </label>
    <input name="action" type="hidden" value="search">
    <button type="submit">Search</button>
  </form>
  <output aria-live="polite">Ready to query syslog-mcp logs.</output>
</body>
</html>
```

- [ ] **Step 5: Add `query_widget_contents()`**

Below `query_widget_resource()` add:

```rust
fn query_widget_contents() -> ResourceContents {
    ResourceContents::text(
        include_str!("ui/query_widget.html"),
        QUERY_WIDGET_RESOURCE_URI,
    )
    .with_mime_type(MCP_APP_HTML_MIME_TYPE)
}
```

- [ ] **Step 6: Route `read_resource()` by URI**

Replace the current unknown-resource check and schema-only return:

```rust
        if request.uri != SCHEMA_RESOURCE_URI {
            return Err(ErrorData::invalid_params(
                format!("unknown resource: {}", request.uri),
                None,
            ));
        }
        let schema = tool_definitions();
        let text = serde_json::to_string_pretty(&schema).map_err(|error| {
            ErrorData::internal_error(format!("serialization error: {error}"), None)
        })?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            text,
            SCHEMA_RESOURCE_URI,
        )
        .with_mime_type("application/json")]))
```

with:

```rust
        match request.uri.as_str() {
            SCHEMA_RESOURCE_URI => {
                let schema = tool_definitions();
                let text = serde_json::to_string_pretty(&schema).map_err(|error| {
                    ErrorData::internal_error(format!("serialization error: {error}"), None)
                })?;
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    SCHEMA_RESOURCE_URI,
                )
                .with_mime_type("application/json")]))
            }
            QUERY_WIDGET_RESOURCE_URI => Ok(ReadResourceResult::new(vec![query_widget_contents()])),
            _ => Err(ErrorData::invalid_params(
                format!("unknown resource: {}", request.uri),
                None,
            )),
        }
```

- [ ] **Step 7: Run the resource tests and verify they pass**

Run:

```bash
cargo test mounted_policy_with_auth_context_permits_schema_resources
cargo test mounted_policy_with_auth_context_permits_query_widget_resource
```

Expected: PASS.

---

### Task 5: Final Verification and Commit

**Files:**
- Modify: `src/mcp/rmcp_server.rs`
- Modify: `src/mcp/rmcp_server_tests.rs`
- Modify: `CHANGELOG.md`
- Create: `src/mcp/ui/query_widget.html`

- [ ] **Step 1: Run all rmcp server tests**

Run:

```bash
cargo test mcp::rmcp_server::tests
```

Expected: PASS.

- [ ] **Step 2: Run formatting**

Run:

```bash
cargo fmt
```

Expected: no command error.

- [ ] **Step 3: Run the required focused clippy gate**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Update the bead**

Run:

```bash
bd close syslog-mcp-yi66.1 --reason "Implemented MCP Apps query widget server contract: tool metadata, UI resource, structuredContent, and rmcp tests."
```

Expected: bead closes, and `bd ready` shows `syslog-mcp-yi66.2` as newly ready.

- [ ] **Step 5: Commit**

Run:

```bash
git status --short
git add CHANGELOG.md docs/superpowers/plans/2026-05-23-mcp-apps-query-widget-contract.md src/mcp/rmcp_server.rs src/mcp/rmcp_server_tests.rs src/mcp/ui/query_widget.html
git commit -m "feat: add MCP Apps query widget contract"
```

Expected: commit succeeds.
