use super::*;

/// Request for `cortex assess skill <skill>`. Either `skill` (a skill
/// name, e.g. `frustration-assessment`) or `plugin` (assess every
/// skill under a plugin) must be set — the service layer forwards both
/// straight into PR 3's `AiSkillInvestigateRequest { skill, plugin, .. }`,
/// which already knows how to resolve either shape. No synthetic string
/// encoding is needed (unlike the earlier FTS5-term-based draft).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillAssessRequest {
    /// Assess exactly this incident (as returned by the matching
    /// `*_incidents` listing). When set, the target fields may be empty.
    /// Skipped when `None` for the same serde_qs reason as
    /// `AiSkillInvestigateRequest::incident_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
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
/// MCP/REST caller — see `src/cli/commands/assess.rs` and
/// `src/app/services/skill_assessment.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAssessResult {
    pub incident_id: String,
    pub findings: crate::app::skill_incident_findings::SkillIncidentFindings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAssessResponse {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub results: Vec<SkillAssessResult>,
    pub total_incidents: usize,
    /// Additional matching incidents not included in `results` because
    /// `--all`/`--limit` was not passed — forwarded directly from PR 3's
    /// `AiSkillInvestigateResponse::other_matching_incidents`.
    pub other_matching_incidents: Vec<crate::app::models::SkillIncidentSummary>,
    /// Forwarded from PR 3 — true when a single low-signal incident was
    /// returned with no error (never an error condition on its own).
    pub no_incident_low_severity_summary: bool,
}
