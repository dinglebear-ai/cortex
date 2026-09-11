use super::*;

/// Request for `cortex assess mcp <tool-or-server>`. Either `mcp_server`,
/// `mcp_tool`, or `tool_name` must be set — the service layer forwards all
/// three straight into `AiMcpInvestigateRequest { mcp_server, mcp_tool,
/// tool_name, .. }`, which resolves them via `search_ai_mcp_incidents`'s
/// filter surface. `positional` is set by the CLI when the user passes a
/// single bare argument (`cortex assess mcp cortex`) whose shape (looks
/// like an `mcp__server__tool` name vs. a bare server name) is resolved by
/// the service layer, not the CLI parser.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpAssessRequest {
    /// Assess exactly this incident (as returned by the matching
    /// `*_incidents` listing). When set, the target fields may be empty.
    /// Skipped when `None` for the same serde_qs reason as
    /// `AiSkillInvestigateRequest::incident_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    pub limit: Option<u32>,
    /// When true, assess every matching incident (bounded by `limit`)
    /// instead of only the highest-priority one.
    #[serde(default)]
    pub all: bool,
}

/// One assessed incident's result (LLM assessment is `None` when the
/// caller requested deterministic-findings-only, e.g. `--no-llm` or an
/// MCP/REST caller).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAssessResult {
    pub incident_id: String,
    pub findings: crate::app::mcp_incident_findings::McpIncidentFindings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAssessResponse {
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
    pub results: Vec<McpAssessResult>,
    pub total_incidents: usize,
    pub other_matching_incidents: Vec<crate::app::models::McpIncidentSummary>,
    pub no_incident_low_severity_summary: bool,
}
