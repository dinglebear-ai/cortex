use super::*;

/// Request for `cortex assess hooks [--hook NAME]`. When `hook_event`/
/// `hook_name` are both omitted, the service resolves the highest-priority
/// hook incident across all hooks in scope (mirrors `SkillAssessRequest`'s
/// "skill or plugin" contract, but hooks have no plugin-level grouping so an
/// unfiltered lookup is the default entry point instead).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookAssessRequest {
    /// Assess exactly this incident (as returned by the matching
    /// `*_incidents` listing). When set, the target fields may be empty.
    /// Skipped when `None` for the same serde_qs reason as
    /// `AiSkillInvestigateRequest::incident_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
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
pub struct HookAssessResult {
    pub incident_id: String,
    pub findings: crate::app::hook_incident_findings::HookIncidentFindings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookAssessResponse {
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub results: Vec<HookAssessResult>,
    pub total_incidents: usize,
    /// Additional matching incidents not included in `results` because
    /// `--all`/`--limit` was not passed.
    pub other_matching_incidents: Vec<crate::app::models::HookIncidentSummary>,
    /// True when a single low-signal incident was returned with no error
    /// (never an error condition on its own).
    pub no_incident_low_severity_summary: bool,
}
