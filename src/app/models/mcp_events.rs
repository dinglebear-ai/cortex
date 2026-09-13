use super::*;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpBackfillRequest {
    pub since: Option<String>,
    pub limit: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpBackfillResult {
    pub scanned: u64,
    pub inserted: u64,
    pub skipped_duplicates: u64,
    pub parse_errors: u64,
    /// Original transcript path/line could not be recovered.
    pub source_unavailable: u64,
    pub truncated: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMcpEventsRequest {
    pub tool_name: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub is_error: Option<bool>,
    #[serde(alias = "since")]
    pub from: Option<String>,
    #[serde(alias = "until")]
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEventEntry {
    pub id: i64,
    pub call_log_id: Option<i64>,
    pub result_log_id: Option<i64>,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub turn_id: Option<String>,
    pub call_id: String,
    pub tool_name: String,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub event_kind: String,
    pub status: Option<String>,
    pub duration_ms: Option<i64>,
    pub is_error: Option<bool>,
    pub arguments_json: Option<String>,
    pub output_preview: Option<String>,
    pub error_text: Option<String>,
}

impl From<db::AiMcpEventEntry> for McpEventEntry {
    fn from(value: db::AiMcpEventEntry) -> Self {
        Self {
            id: value.id,
            call_log_id: value.call_log_id,
            result_log_id: value.result_log_id,
            ai_tool: value.ai_tool,
            ai_project: value.ai_project,
            ai_session_id: value.ai_session_id,
            hostname: value.hostname,
            timestamp: value.timestamp,
            turn_id: value.turn_id,
            call_id: value.call_id,
            tool_name: value.tool_name,
            mcp_server: value.mcp_server,
            mcp_tool: value.mcp_tool,
            event_kind: value.event_kind,
            status: value.status,
            duration_ms: value.duration_ms,
            is_error: value.is_error,
            arguments_json: value.arguments_json,
            output_preview: value.output_preview,
            error_text: value.error_text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMcpEventsResponse {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<McpEventEntry>,
}

impl From<db::ListMcpEventsResult> for ListMcpEventsResponse {
    fn from(value: db::ListMcpEventsResult) -> Self {
        Self {
            total: value.total,
            truncated: value.truncated,
            events: value.events.into_iter().map(Into::into).collect(),
        }
    }
}
