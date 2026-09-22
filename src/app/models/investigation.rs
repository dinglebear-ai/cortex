use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub const INVESTIGATION_UI_VERSION: &str = "investigation-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvestigationVersionResponse {
    pub ui_version: &'static str,
    pub schema_version: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvestigationEnvelope<T> {
    pub metadata: InvestigationMetadata,
    pub result: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvestigationMetadata {
    pub server_version: String,
    pub schema_version: String,
    pub graph_projection_status: Option<String>,
    pub source_watermark: Option<String>,
    pub degraded_reasons: Vec<String>,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
    pub partial: bool,
    pub partial_reasons: Vec<String>,
    pub auth_state: String,
    pub budget: InvestigationBudget,
    pub budget_used: InvestigationBudgetUsed,
    pub payload_limit_bytes: u32,
    pub version_skew: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvestigationBudget {
    pub max_graph_calls: u32,
    pub max_log_rows: u32,
    pub max_evidence_rows: u32,
    pub max_candidate_explanations: u32,
    pub max_wall_time_ms: u32,
    pub max_payload_bytes: u32,
}

impl Default for InvestigationBudget {
    fn default() -> Self {
        Self {
            max_graph_calls: 3,
            max_log_rows: 25,
            max_evidence_rows: 12,
            max_candidate_explanations: 5,
            max_wall_time_ms: 2_000,
            max_payload_bytes: 65_536,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvestigationBudgetUsed {
    pub graph_calls: u32,
    pub log_rows: u32,
    pub evidence_rows: u32,
    pub candidate_explanations: u32,
    pub wall_time_ms: u32,
    pub payload_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationClaimType {
    Verified,
    SupportedCorrelation,
    WeakCorrelation,
    OpenQuestion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvestigationClaim {
    pub claim_type: InvestigationClaimType,
    pub title: String,
    pub summary: String,
    pub confidence: String,
    pub relationship_ids: Vec<i64>,
    pub evidence_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppEntitySummary {
    pub id: i64,
    pub entity_type: String,
    pub key: String,
    pub label: String,
    pub trust_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppRelationshipSummary {
    pub id: i64,
    pub source_entity_id: i64,
    pub target_entity_id: i64,
    pub relationship_type: String,
    pub reason_code: String,
    pub trust_level: String,
    pub confidence: f64,
    pub evidence_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppEvidenceSummary {
    pub id: i64,
    pub relationship_id: i64,
    pub source_kind: String,
    pub source_log_id: Option<i64>,
    pub observed_at: String,
    pub reason_code: String,
    pub reason_text: Option<String>,
    pub confidence_delta: f64,
    pub trust_level: String,
    pub excerpt: Option<String>,
    pub missing_source_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppLogSummary {
    pub id: i64,
    pub timestamp: String,
    pub received_at: String,
    pub hostname: String,
    pub severity: String,
    pub app_name: Option<String>,
    pub message: String,
    pub message_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppGraphResponse {
    pub focus: Option<AppEntitySummary>,
    pub entities: Vec<AppEntitySummary>,
    pub relationships: Vec<AppRelationshipSummary>,
    pub evidence: Vec<AppEvidenceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppGraphEntityResponse {
    pub resolved_entity: Option<AppEntitySummary>,
    pub candidates: Vec<AppEntitySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppGraphEvidenceResponse {
    pub evidence: AppEvidenceSummary,
    pub relationship: AppRelationshipSummary,
    pub source_log_summary: Option<AppLogSummary>,
    pub missing_source_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AskInvestigationRequest {
    pub prompt: String,
    pub host: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AskInvestigationResponse {
    pub prompt: String,
    pub resolved_entity: Option<AppEntitySummary>,
    pub candidates: Vec<AppEntitySummary>,
    pub claims: Vec<InvestigationClaim>,
    pub open_questions: Vec<String>,
    pub next_queries: Vec<String>,
    pub graph: AppGraphResponse,
    pub logs: Vec<AppLogSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInvestigateRequest {
    pub session_id: String,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub host: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub severity_min: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SessionExternalReferenceKind {
    GithubPullRequest,
    GithubIssue,
    GithubReference,
    LinearIssue,
    Url,
    CommitSha,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionExternalReference {
    pub kind: SessionExternalReferenceKind,
    pub value: String,
    pub source_position: Option<i64>,
    pub evidence_kind: String,
    pub trust_level: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRetentionLineageEntry {
    pub log_id: i64,
    pub hostname: String,
    pub app_name: Option<String>,
    pub severity: String,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_session_id: Option<String>,
    pub retained_until_epoch: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSourceEvidenceSummary {
    pub count: usize,
    pub log_ids: Vec<i64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionObservatoryEvidence {
    pub runs: Vec<db::agent_observatory::ObservatoryRunRow>,
    pub ambiguous_run: bool,
    pub related_runs: Vec<db::agent_observatory::ObservatoryRunRow>,
    pub related_runs_truncated: bool,
    pub repository: Option<db::agent_observatory::ObservatoryRepositoryRow>,
    pub worktree: Option<db::agent_observatory::ObservatoryWorktreeRow>,
    pub commits: Vec<db::agent_observatory::AgentRunAttributedCommit>,
    pub events: Vec<db::agent_observatory::ObservatoryEventRow>,
    pub spans: Vec<db::agent_observatory::ObservatorySpanRow>,
    pub metrics: Vec<db::agent_observatory::ObservatoryMetricRow>,
    pub events_truncated: bool,
    pub spans_truncated: bool,
    pub metrics_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInvestigateResponse {
    pub session: AiSessionEntry,
    pub transcript: Vec<RenderedSessionEvent>,
    pub transcript_has_more: bool,
    pub correlation: Option<GraphSessionCorrelation>,
    pub graph_neighborhood: Option<AppGraphResponse>,
    pub skill_events: Vec<SkillEventEntry>,
    pub skill_events_truncated: bool,
    pub mcp_events: Vec<McpEventEntry>,
    pub mcp_events_truncated: bool,
    pub hook_events: Vec<HookEventEntry>,
    pub hook_events_truncated: bool,
    pub artifact_evidence: Vec<ArtifactEvidenceEntry>,
    pub artifact_evidence_truncated: bool,
    pub observatory: SessionObservatoryEvidence,
    pub incident_context: IncidentContextResponse,
    pub notifications: Vec<db::notifications::FiringRow>,
    pub notifications_truncated: bool,
    pub retention_lineage: Vec<SessionRetentionLineageEntry>,
    pub retention_lineage_truncated: bool,
    pub related_sessions: Vec<AiSessionEntry>,
    pub external_references: Vec<SessionExternalReference>,
    pub source_counts: BTreeMap<String, usize>,
    pub source_evidence: BTreeMap<String, SessionSourceEvidenceSummary>,
    pub partial_reasons: Vec<String>,
}

pub fn app_entity_summary(entity: &GraphEntity) -> AppEntitySummary {
    AppEntitySummary {
        id: entity.id,
        entity_type: entity.entity_type.clone(),
        key: entity.canonical_key.clone(),
        label: safe_passive_text(&entity.display_label, 160),
        trust_level: entity.trust_level.clone(),
    }
}

pub fn app_relationship_summary(relationship: &GraphRelationship) -> AppRelationshipSummary {
    AppRelationshipSummary {
        id: relationship.id,
        source_entity_id: relationship.src_entity_id,
        target_entity_id: relationship.dst_entity_id,
        relationship_type: relationship.relationship_type.clone(),
        reason_code: relationship.reason_code.clone(),
        trust_level: relationship.trust_level.clone(),
        confidence: relationship.confidence,
        evidence_count: relationship.evidence_count,
    }
}

pub fn app_evidence_summary(evidence: &GraphEvidence) -> AppEvidenceSummary {
    AppEvidenceSummary {
        id: evidence.id,
        relationship_id: evidence.relationship_id,
        source_kind: evidence.source_kind.clone(),
        source_log_id: evidence.source_log_id,
        observed_at: evidence.observed_at.clone(),
        reason_code: evidence.reason_code.clone(),
        reason_text: evidence
            .reason_text
            .as_deref()
            .map(|text| safe_passive_text(text, 320)),
        confidence_delta: evidence.confidence_delta,
        trust_level: evidence.trust_level.clone(),
        excerpt: evidence
            .safe_excerpt
            .as_deref()
            .map(|text| safe_passive_text(text, 500)),
        missing_source_reason: evidence
            .source_log_id
            .is_none()
            .then(|| "source_log_missing_or_retained_out".to_string()),
    }
}

pub fn app_log_summary(log: &LogEntry, max_chars: usize) -> AppLogSummary {
    AppLogSummary {
        id: log.id,
        timestamp: log.timestamp.clone(),
        received_at: log.received_at.clone(),
        hostname: safe_passive_text(&log.hostname, 120),
        severity: log.severity.clone(),
        app_name: log
            .app_name
            .as_deref()
            .map(|app| safe_passive_text(app, 120)),
        message: safe_passive_text(&log.message, max_chars),
        message_truncated: log.message.chars().count() > max_chars,
    }
}

pub fn app_graph_from_explain_response(explain: &GraphExplainResponse) -> AppGraphResponse {
    let mut entities = BTreeMap::<i64, AppEntitySummary>::new();
    let mut relationships = BTreeMap::<i64, AppRelationshipSummary>::new();
    let mut evidence_ids = BTreeSet::<i64>::new();

    if let Some(entity) = explain.resolved_entity.as_ref() {
        entities.insert(entity.id, app_entity_summary(entity));
    }
    for chain in &explain.chains {
        for entity in &chain.entities {
            entities.insert(entity.id, app_entity_summary(entity));
        }
        for relationship in &chain.relationships {
            relationships.insert(relationship.id, app_relationship_summary(relationship));
        }
        evidence_ids.extend(chain.evidence_ids.iter().copied());
    }

    let evidence = explain
        .evidence
        .iter()
        .filter(|evidence| evidence_ids.is_empty() || evidence_ids.contains(&evidence.id))
        .map(app_evidence_summary)
        .collect();

    AppGraphResponse {
        focus: explain.resolved_entity.as_ref().map(app_entity_summary),
        entities: entities.into_values().collect(),
        relationships: relationships.into_values().collect(),
        evidence,
    }
}

pub fn app_graph_from_around_response(around: &GraphAroundResponse) -> AppGraphResponse {
    AppGraphResponse {
        focus: around.resolved_entity.as_ref().map(app_entity_summary),
        entities: around.entities.iter().map(app_entity_summary).collect(),
        relationships: around
            .relationships
            .iter()
            .map(app_relationship_summary)
            .collect(),
        evidence: around.evidence.iter().map(app_evidence_summary).collect(),
    }
}

pub fn safe_passive_text(input: &str, max_chars: usize) -> String {
    let mut out = input
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect::<String>();
    for marker in [
        "sk-proj-",
        "Bearer ",
        "password=",
        "token=",
        "CORTEX_API_TOKEN=",
    ] {
        out = out.replace(marker, "[redacted]");
    }
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>();
        out.push_str("...");
    }
    out
}
