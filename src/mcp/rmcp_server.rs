use std::{
    borrow::Cow,
    net::Ipv6Addr,
    sync::{Arc, OnceLock},
};

use lab_auth::AuthContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        Annotations, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        EmbeddedResource, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
        Implementation, ListPromptsResult, ListResourcesResult, ListToolsResult, MetaObject,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, Role, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Map, Value};

use crate::app::ServiceError;
use crate::config::McpConfig;

use super::actions;
use super::prompts::{get_prompt, prompt_definitions};
use super::{AppState, AuthPolicy, schemas::tool_definitions, tools::execute_tool};

#[derive(Clone)]
pub struct CortexRmcpServer {
    state: AppState,
}

impl CortexRmcpServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

pub fn rmcp_server(state: AppState) -> CortexRmcpServer {
    CortexRmcpServer::new(state)
}

impl ServerHandler for CortexRmcpServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // tools/list requires AuthContext when policy is Mounted (but no scope).
        // LoopbackDev bypasses the check entirely.
        require_auth_context(&self.state, &context)?;

        let tools = rmcp_tool_definitions()?;
        tracing::info!(tool_count = tools.len(), "MCP tools listed");
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let tool_name = request.name.to_string();

        // Extract action for scope check before any DB work fires.
        // Clone into an owned String so request.arguments can be consumed below.
        let action: String = request
            .arguments
            .as_ref()
            .and_then(|m| m.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        // Fail-closed auth check: require AuthContext when Mounted, then scope.
        // LoopbackDev returns None — no scope enforcement applies.
        let auth = require_auth_context(&self.state, &context)?;
        if let (Some(auth), Some(required_scope)) = (auth, required_scope_for(&action)) {
            check_scope(auth, required_scope, &action)?;
        }

        let arguments = request
            .arguments
            .map(Value::Object)
            .unwrap_or_else(|| Value::Object(Map::new()));
        tracing::info!(tool = %tool_name, "MCP tool execution started");

        match execute_tool(&self.state, &tool_name, arguments, auth).await {
            Ok(result) => {
                let result_count = safe_result_count(&result);
                tracing::info!(
                    tool = %tool_name,
                    result_count,
                    "MCP tool execution completed"
                );
                let mut call_result = tool_result_from_json(result)?;
                if should_embed_widget(&action, widget_embed_enabled()) {
                    call_result.content.push(embedded_widget_content());
                }
                Ok(call_result.into())
            }
            // Mirror api.rs::respond(): every ServiceError variant maps to a
            // distinct client-visible class so MCP clients can tell retryable
            // contention from bad input from server faults (full-review AH1).
            Err(error) => match classify_tool_error(&error) {
                ToolErrorClass::InvalidParams => {
                    tracing::warn!(
                        tool = %tool_name,
                        error = %error,
                        error_class = "invalid_params",
                        "MCP tool execution rejected invalid params"
                    );
                    Ok(invalid_params_tool_result(&action, error.to_string()).into())
                }
                ToolErrorClass::Retryable => {
                    tracing::warn!(
                        tool = %tool_name,
                        error = %error,
                        error_class = "retryable",
                        "MCP tool execution is unavailable due to database contention or connection failure"
                    );
                    Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Action '{action}' is temporarily unavailable (database busy). \
                         Retry shortly; if the problem persists, check the server logs."
                    ))])
                    .into())
                }
                ToolErrorClass::NotFound => {
                    tracing::warn!(
                        tool = %tool_name,
                        error = %error,
                        error_class = "not_found",
                        "MCP tool execution target not found"
                    );
                    // NotFound messages are client-safe by construction
                    // (e.g. "log id 42 not found") — see ServiceError docs.
                    Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Not found: {error}"
                    ))])
                    .into())
                }
                ToolErrorClass::Conflict => {
                    tracing::warn!(
                        tool = %tool_name,
                        error = %error,
                        error_class = "conflict",
                        "MCP tool execution hit a constraint conflict"
                    );
                    Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Conflict: {error}"
                    ))])
                    .into())
                }
                ToolErrorClass::Internal => {
                    tracing::error!(
                        tool = %tool_name,
                        error = %error,
                        error_class = "tool_execution",
                        "MCP tool execution failed"
                    );
                    Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Tool execution failed for action '{action}'. \
                         Check server logs for details."
                    ))])
                    .into())
                }
            },
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        require_auth_context(&self.state, &context)?;
        Ok(ListResourcesResult {
            resources: vec![
                schema_resource(),
                prompt_output_schema_resource(),
                query_widget_resource(),
            ],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        require_auth_context(&self.state, &context)?;
        match request.uri.as_str() {
            SCHEMA_RESOURCE_URI => {
                let schema = tool_definitions();
                let text = serde_json::to_string_pretty(&schema).map_err(|error| {
                    ErrorData::internal_error(format!("serialization error: {error}"), None)
                })?;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, SCHEMA_RESOURCE_URI)
                        .with_mime_type("application/json"),
                ])
                .into())
            }
            PROMPT_OUTPUT_SCHEMA_RESOURCE_URI => {
                let text =
                    serde_json::to_string_pretty(&prompt_output_schema()).map_err(|error| {
                        ErrorData::internal_error(format!("serialization error: {error}"), None)
                    })?;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, PROMPT_OUTPUT_SCHEMA_RESOURCE_URI)
                        .with_mime_type("application/schema+json"),
                ])
                .into())
            }
            QUERY_WIDGET_RESOURCE_URI => {
                Ok(ReadResourceResult::new(vec![query_widget_contents()]).into())
            }
            _ => Err(ErrorData::invalid_params(
                format!("unknown resource: {}", request.uri),
                None,
            )),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        require_auth_context(&self.state, &context)?;
        let prompts = prompt_definitions();
        tracing::info!(prompt_count = prompts.len(), "MCP prompts listed");
        Ok(ListPromptsResult {
            prompts,
            ..Default::default()
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        require_auth_context(&self.state, &context)?;
        let Some((description, messages)) = get_prompt(&request.name, request.arguments.as_ref())
        else {
            return Err(ErrorData::invalid_params(
                format!("unknown prompt: {}", request.name),
                None,
            ));
        };
        tracing::info!(prompt = %request.name, "MCP prompt rendered");
        Ok(GetPromptResult::new(messages)
            .with_description(description)
            .into())
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new(
            self.state.config.server_name.clone(),
            env!("CARGO_PKG_VERSION"),
        ))
    }
}

pub fn streamable_http_config(config: &McpConfig) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts(config))
        .with_allowed_origins(allowed_origins(config))
}

pub fn streamable_http_service(
    state: AppState,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<CortexRmcpServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(CortexRmcpServer::new(state.clone())),
        Default::default(),
        config,
    )
}

const SCHEMA_RESOURCE_URI: &str = "cortex://schema/mcp-tool";
const PROMPT_OUTPUT_SCHEMA_RESOURCE_URI: &str = "cortex://schema/prompt-output";
pub(super) const QUERY_WIDGET_RESOURCE_URI: &str = "ui://cortex/query-widget";
pub(super) const MCP_APP_HTML_MIME_TYPE: &str = "text/html;profile=mcp-app";

fn schema_resource() -> Resource {
    Resource::new(SCHEMA_RESOURCE_URI, "cortex tool schema")
        .with_description("JSON schema for the cortex MCP tool and its action-based parameters")
        .with_mime_type("application/json")
}

fn prompt_output_schema_resource() -> Resource {
    Resource::new(
        PROMPT_OUTPUT_SCHEMA_RESOURCE_URI,
        "cortex prompt output schema",
    )
    .with_description("JSON schema for structured incident-style outputs from cortex MCP prompts")
    .with_mime_type("application/schema+json")
}

fn query_widget_resource() -> Resource {
    Resource::new(QUERY_WIDGET_RESOURCE_URI, "cortex query widget")
        .with_title("Syslog Query")
        .with_description("Interactive MCP Apps widget for querying cortex logs")
        .with_mime_type(MCP_APP_HTML_MIME_TYPE)
}

fn prompt_output_schema() -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": PROMPT_OUTPUT_SCHEMA_RESOURCE_URI,
        "title": "CortexPromptInvestigationResult",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "verdict",
            "confidence",
            "evidence",
            "likely_cause",
            "not_supported",
            "next_actions",
            "telemetry_gaps"
        ],
        "properties": {
            "verdict": {
                "type": "string",
                "description": "One sentence conclusion with the current confidence level."
            },
            "confidence": {
                "type": "string",
                "enum": ["low", "medium", "high"],
                "description": "Confidence based only on observed evidence."
            },
            "evidence": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "source",
                        "summary",
                        "timestamp",
                        "host",
                        "app",
                        "severity",
                        "log_id"
                    ],
                    "properties": {
                        "source": {
                            "type": "string",
                            "description": "Tool action, transcript, or log id used as evidence."
                        },
                        "summary": {
                            "type": "string",
                            "description": "Concise evidence summary without raw payload dumps."
                        },
                        "timestamp": {
                            "type": ["string", "null"],
                            "description": "RFC3339 timestamp when available."
                        },
                        "host": { "type": ["string", "null"] },
                        "app": { "type": ["string", "null"] },
                        "severity": { "type": ["string", "null"] },
                        "log_id": { "type": ["integer", "null"] }
                    }
                }
            },
            "likely_cause": { "type": "string" },
            "not_supported": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Hypotheses considered but not supported by current evidence."
            },
            "next_actions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "telemetry_gaps": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

fn query_widget_contents() -> ResourceContents {
    ResourceContents::text(
        include_str!("ui/query_widget.html"),
        QUERY_WIDGET_RESOURCE_URI,
    )
    .with_mime_type(MCP_APP_HTML_MIME_TYPE)
}

/// Actions whose successful results additionally carry the query widget as an
/// embedded resource content block.
///
/// MCP Apps hosts normally hydrate the widget by calling `resources/read` on
/// `_meta.ui.resourceUri`, but some connection paths (notably the claude.ai
/// connector proxy) expose tools only and report "does not support
/// resources", leaving a permanent "Rendering widget" placeholder. Embedding
/// the widget in the tool result gives such hosts an mcp-ui-style fallback
/// they can render without a resource round-trip.
const WIDGET_EMBED_ACTIONS: &[&str] = &["search", "filter", "tail", "errors"];

/// Whether `action`'s successful result should carry the embedded widget.
fn should_embed_widget(action: &str, enabled: bool) -> bool {
    enabled && WIDGET_EMBED_ACTIONS.contains(&action)
}

/// Parse the `CORTEX_WIDGET_EMBED` opt-in value. Defaults to `false`: the
/// widget HTML is ~16 KiB per result, and although the block is annotated
/// `audience: ["user"]`, hosts that ignore audience annotations would feed it
/// to the model on every query — so operators enable it deliberately.
fn parse_widget_embed(raw: Option<&str>) -> bool {
    raw.map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

/// Process-lifetime cache of the `CORTEX_WIDGET_EMBED` opt-in.
fn widget_embed_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| parse_widget_embed(crate::env::var("CORTEX_WIDGET_EMBED").ok().as_deref()))
}

/// The query widget as an embedded resource content block, annotated
/// `audience: ["user"]` so audience-aware hosts keep it out of model context.
fn embedded_widget_content() -> ContentBlock {
    ContentBlock::Resource(
        EmbeddedResource::new(
            ResourceContents::text(
                include_str!("ui/query_widget.html"),
                QUERY_WIDGET_RESOURCE_URI,
            )
            .with_mime_type(MCP_APP_HTML_MIME_TYPE),
        )
        .with_annotations(Annotations::default().with_audience(vec![Role::User])),
    )
}

fn cortex_tool_meta() -> MetaObject {
    let mut meta = Map::new();
    // Both formats the MCP Apps SDK tells hosts to check: the flat
    // `ui/resourceUri` key (ext-apps `RESOURCE_URI_META_KEY`) and the nested
    // `ui` object. Host generations differ in which one they read.
    meta.insert(
        "ui/resourceUri".to_string(),
        Value::String(QUERY_WIDGET_RESOURCE_URI.to_string()),
    );
    meta.insert(
        "ui".to_string(),
        serde_json::json!({
            "resourceUri": QUERY_WIDGET_RESOURCE_URI,
            "visibility": ["model", "app"],
        }),
    );
    MetaObject(meta)
}

fn rmcp_tool_definitions() -> Result<Vec<Tool>, ErrorData> {
    tool_definitions()
        .into_iter()
        .map(rmcp_tool_from_json)
        .collect()
}

fn rmcp_tool_from_json(value: Value) -> Result<Tool, ErrorData> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ErrorData::internal_error("tool definition missing name", None))?;
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(|description| Cow::Owned(description.to_string()));
    let input_schema = value
        .get("inputSchema")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("tool definition missing inputSchema", None))?;

    let tool = Tool::new_with_raw(
        Cow::Owned(name.to_string()),
        description,
        Arc::new(input_schema),
    );

    Ok(if name == "cortex" {
        tool.with_meta(cortex_tool_meta())
    } else {
        tool
    })
}

fn tool_result_from_json(value: Value) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(&value).map_err(|error| {
        ErrorData::internal_error(format!("serialization error: {error}"), None)
    })?;
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(text)];
    Ok(result)
}

fn invalid_params_tool_result(action: &str, message: String) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "kind": "invalid_param",
        "message": message,
        "action": action,
        "retryable": false,
    }))
}

/// Client-facing classification of a tool-execution error.
///
/// Derived ONLY from the typed [`ServiceError`] in the anyhow chain — the
/// previous string-matching classifier (`" is required"`, `" must be <="`, …)
/// misclassified any future error message containing those substrings and
/// could not distinguish "retry me" from "your input was wrong" (full-review
/// AH1). All caller-input failures are now raised as
/// `ServiceError::InvalidInput` at the source (`tools.rs::invalid_input`,
/// service-layer validation), so no string heuristics are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolErrorClass {
    /// Caller-supplied arguments were invalid → structured tool-level error.
    InvalidParams,
    /// Transient resource contention (SQLite busy, pool timeout) → the
    /// caller should retry.
    Retryable,
    /// The referenced resource does not exist.
    NotFound,
    /// A uniqueness/constraint conflict.
    Conflict,
    /// Everything else — details stay server-side.
    Internal,
}

fn classify_tool_error(error: &anyhow::Error) -> ToolErrorClass {
    match error.downcast_ref::<ServiceError>() {
        Some(ServiceError::InvalidInput(_)) => ToolErrorClass::InvalidParams,
        Some(ServiceError::Busy(_)) | Some(ServiceError::DatabaseTimeout { .. }) => {
            ToolErrorClass::Retryable
        }
        Some(ServiceError::NotFound(_)) | Some(ServiceError::RowNotFound) => {
            ToolErrorClass::NotFound
        }
        Some(ServiceError::ConstraintViolation { .. }) => ToolErrorClass::Conflict,
        Some(ServiceError::Internal(_)) | None => ToolErrorClass::Internal,
    }
}

fn safe_result_count(value: &Value) -> Option<usize> {
    value
        .get("count")
        .and_then(Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .or_else(|| value.get("hosts").and_then(Value::as_array).map(Vec::len))
        .or_else(|| value.get("summary").and_then(Value::as_array).map(Vec::len))
}

// ── Auth helpers ─────────────────────────────────────────────────────────────

/// Extract and enforce the authentication context from the rmcp request.
///
/// - no-auth policies: always return `Ok(None)` — the listener bind or trusted
///   gateway is the trust boundary; no per-request credential needed.
/// - `AuthPolicy::Mounted(_)`: the middleware MUST have inserted an
///   [`AuthContext`] into the request extensions. If it is absent, this
///   returns a forbidden error immediately (fail-closed).
///
/// Returns `Ok(Some(&AuthContext))` for Mounted+present, `Ok(None)` for
/// no-auth policies. Callers can skip the scope check when the result is `None`.
fn require_auth_context<'a>(
    state: &AppState,
    ctx: &'a RequestContext<RoleServer>,
) -> Result<Option<&'a AuthContext>, ErrorData> {
    match &state.auth_policy {
        AuthPolicy::LoopbackDev | AuthPolicy::TrustedGatewayUnscoped => Ok(None),
        AuthPolicy::Mounted { .. } => {
            let parts = ctx
                .extensions
                .get::<axum::http::request::Parts>()
                .ok_or_else(|| {
                    // This indicates a framework-level invariant violation —
                    // rmcp changed how it propagates HTTP Parts, or the middleware
                    // ordering is broken at startup.
                    tracing::error!(
                        "rmcp HTTP Parts extension absent — middleware ordering may be broken; \
                         see docs/internal/rmcp-auth-spike.md"
                    );
                    ErrorData::invalid_request("forbidden: missing http context", None)
                })?;
            let auth = parts.extensions.get::<AuthContext>().ok_or_else(|| {
                // AuthLayer should always insert AuthContext on the happy path.
                // Absence here means AuthLayer is not mounted or failed to run.
                tracing::warn!(
                    "AuthContext absent from request extensions — \
                     AuthLayer may not be mounted or rejected the request without inserting context"
                );
                ErrorData::invalid_request("forbidden: missing auth context", None)
            })?;
            Ok(Some(auth))
        }
    }
}

/// Enforce that `auth` carries `required_scope`.
///
/// `cortex:admin` is treated as a superset of `cortex:read` — a caller with
/// admin access implicitly satisfies any read-level scope requirement.
///
/// Logs a warning with subject + action on denial (audit trail).
/// Only called when policy is Mounted (LoopbackDev short-circuits at the
/// caller via `require_auth_context` returning `None`).
fn check_scope(auth: &AuthContext, required_scope: &str, action: &str) -> Result<(), ErrorData> {
    let satisfied = auth
        .scopes
        .iter()
        .any(|s| s == required_scope || (required_scope == "cortex:read" && s == "cortex:admin"));
    if satisfied {
        return Ok(());
    }
    tracing::warn!(
        subject = %auth.sub,
        action = %action,
        required_scope = %required_scope,
        "MCP tool invocation denied: insufficient scope"
    );
    Err(ErrorData::invalid_request(
        format!("forbidden: requires scope: {required_scope}"),
        None,
    ))
}

/// Map a cortex tool action to the minimum required scope.
fn required_scope_for(action: &str) -> Option<&'static str> {
    actions::required_scope_for(action)
}

pub(super) fn allowed_hosts(config: &McpConfig) -> Vec<String> {
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    for host in &config.allowed_hosts {
        push_host_variants(&mut hosts, host, config.port);
    }
    push_host_variants(&mut hosts, &config.host, config.port);
    push_host_variants(&mut hosts, "localhost", config.port);
    push_host_variants(&mut hosts, "127.0.0.1", config.port);
    push_host_variants(&mut hosts, "::1", config.port);
    // When CORTEX_PUBLIC_URL is set (required for OAuth mode), extract the
    // host and add it to the allowlist so callbacks from the public hostname are
    // accepted by rmcp's DNS-rebinding guard.
    if let Some(public_url) = config.auth.public_url.as_deref() {
        push_public_url_hosts(&mut hosts, public_url, config.port);
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

fn push_host_variants(hosts: &mut Vec<String>, host: &str, port: u16) {
    let host = host.trim();
    if host.is_empty() {
        return;
    }
    hosts.push(host.to_string());
    if host.starts_with('[') && host.contains("]:") {
        return;
    }
    if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        if !inner.is_empty() {
            hosts.push(format!("[{inner}]:{port}"));
        }
    } else if host.parse::<Ipv6Addr>().is_ok() {
        hosts.push(format!("[{host}]"));
        hosts.push(format!("[{host}]:{port}"));
    } else if !has_port(host) {
        hosts.push(format!("{host}:{port}"));
    }
}

/// Extract the host (and explicit port, if any) from a URL string and push
/// variants into the hosts allowlist.
///
/// Browsers send the Host header with port only when it differs from the
/// default for the scheme (80 for http, 443 for https). We emit both the
/// bare host and `host:port` so that both direct (non-standard-port) and
/// reverse-proxied (standard-port) deployments are covered.
fn push_public_url_hosts(hosts: &mut Vec<String>, url: &str, listen_port: u16) {
    let Ok(parsed) = url::Url::parse(url) else {
        tracing::warn!(
            public_url = url,
            "CORTEX_PUBLIC_URL is not a valid URL; skipping host allowlist extension"
        );
        return;
    };
    let Some(host) = parsed.host_str() else {
        return;
    };
    // Never add wildcards — rmcp does exact host matching and a wildcard in
    // the allowlist would silently permit any Host header value.
    if host.contains('*') {
        tracing::warn!(host, "CORTEX_PUBLIC_URL host contains wildcard; skipping");
        return;
    }
    // `url::Url::port()` returns None for default ports (443 for https, 80 for
    // http). Browsers omit the port from the Host header when it matches the
    // scheme default, so we must allowlist the bare host in that case.
    //
    // For non-standard ports, browsers include the port in Host, so we emit
    // both `host` and `host:port`.
    //
    // For standard ports we additionally emit `host:default_port` so that
    // rmcp's port-aware comparison passes even when the port is explicit.
    let explicit_port = parsed.port();
    let scheme_default_port = match parsed.scheme() {
        "https" => Some(443u16),
        "http" => Some(80u16),
        _ => None,
    };

    if let Some(p) = explicit_port {
        // Non-standard port: push `host`, `host:p` (both forms browsers use).
        push_host_variants(hosts, host, p);
        let with_port = format!("{host}:{p}");
        if !hosts.contains(&with_port) {
            hosts.push(with_port);
        }
    } else if let Some(default_port) = scheme_default_port {
        // Standard port (implicit): push bare `host` and `host:default_port`.
        // The bare host is the form browsers put in the Host header; the
        // `host:port` form satisfies rmcp's port-aware allowlist comparison.
        let bare = host.to_string();
        if !hosts.contains(&bare) {
            hosts.push(bare);
        }
        let with_default = format!("{host}:{default_port}");
        if !hosts.contains(&with_default) {
            hosts.push(with_default);
        }
    } else {
        // Unknown scheme: fall back to listen_port as before.
        push_host_variants(hosts, host, listen_port);
    }
}

fn has_port(host: &str) -> bool {
    host.rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .is_some()
}

pub(super) fn allowed_origins(config: &McpConfig) -> Vec<String> {
    let mut origins = vec![
        format!("http://localhost:{}", config.port),
        format!("http://127.0.0.1:{}", config.port),
    ];
    origins.extend(config.allowed_origins.iter().cloned());
    // When CORTEX_PUBLIC_URL is set, add its origin (scheme + host + port
    // if non-default) so browser preflight from the configured public URL is
    // accepted by the CORS layer.
    if let Some(public_url) = config.auth.public_url.as_deref()
        && let Some(origin) = extract_origin(public_url)
    {
        origins.push(origin);
    }
    origins.sort();
    origins.dedup();
    origins
}

/// Build the Origin string from a URL (`scheme://host` or `scheme://host:port`).
///
/// Browsers omit the port from the Origin header when it matches the scheme
/// default (80 for http, 443 for https). We follow the same rule so that the
/// string we store matches what browsers actually send.
fn extract_origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url)
        .map_err(|e| {
            tracing::warn!(public_url = url, error = %e, "CORTEX_PUBLIC_URL is not a valid URL; skipping origin allowlist extension");
        })
        .ok()?;
    let scheme = parsed.scheme();
    let host = parsed.host_str()?;
    // Never add wildcards.
    if host.contains('*') {
        tracing::warn!(
            host,
            "CORTEX_PUBLIC_URL host contains wildcard; skipping origin"
        );
        return None;
    }
    let default_port = match scheme {
        "http" => Some(80u16),
        "https" => Some(443u16),
        _ => None,
    };
    let origin = match parsed.port() {
        Some(port) if default_port != Some(port) => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    };
    Some(origin)
}

#[cfg(test)]
#[path = "rmcp_server_tests.rs"]
mod tests;
