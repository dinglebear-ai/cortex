use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSkillIncidentRequest {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    #[serde(default)]
    pub signals: Vec<String>,
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillSignalCounts {
    pub user_correction_after_skill: usize,
    pub tool_failure_after_skill: usize,
    pub scope_or_source_confusion: usize,
    pub ignored_skill_or_policy_instruction: usize,
    pub overlong_loop_after_skill: usize,
}

impl From<db::SkillSignalCounts> for SkillSignalCounts {
    fn from(v: db::SkillSignalCounts) -> Self {
        Self {
            user_correction_after_skill: v.user_correction_after_skill,
            tool_failure_after_skill: v.tool_failure_after_skill,
            scope_or_source_confusion: v.scope_or_source_confusion,
            ignored_skill_or_policy_instruction: v.ignored_skill_or_policy_instruction,
            overlong_loop_after_skill: v.overlong_loop_after_skill,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncident {
    pub incident_id: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub skill_event_count: usize,
    pub skill_event_ids: Vec<i64>,
    pub anchor_log_ids: Vec<i64>,
    /// Signals are incomplete because anchor sampling reached a bound.
    #[serde(default)]
    pub anchor_window_truncated: bool,
    pub signal_counts: SkillSignalCounts,
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    pub priority_label: String,
    pub window_minutes: u32,
}

impl From<db::SkillIncident> for SkillIncident {
    fn from(v: db::SkillIncident) -> Self {
        Self {
            incident_id: v.incident_id,
            skill_name: v.skill_name,
            skill_plugin: v.skill_plugin,
            tool: v.tool,
            project: v.project,
            session_id: v.session_id,
            hostname: v.hostname,
            first_seen: v.first_seen,
            last_seen: v.last_seen,
            duration_secs: v.duration_secs,
            skill_event_count: v.skill_event_count,
            skill_event_ids: v.skill_event_ids,
            anchor_log_ids: v.anchor_log_ids,
            anchor_window_truncated: v.anchor_window_truncated,
            signal_counts: v.signal_counts.into(),
            signals_present: v.signals_present,
            priority_score: v.priority_score,
            priority_label: v.priority_label,
            window_minutes: v.window_minutes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillIncidentResponse {
    pub incidents: Vec<SkillIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSkillInvestigateRequest {
    // `incident_id` is skipped when `None` for the same reason as
    // `AiInvestigateRequest::incident_id` in `ai_incidents.rs` — the REST
    // query surface uses `deny_unknown_fields` and serde_qs emits `None`
    // options as a bare key, which the server rejects as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentEvidence {
    pub incident: SkillIncident,
    pub skill_events: Vec<db::AiSkillEventEntry>,
    pub skill_events_truncated: bool,
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    pub nearby_tool_failures: Vec<LogEntry>,
    pub nearby_tool_failures_truncated: bool,
    pub nearby_user_corrections: Vec<LogEntry>,
    pub nearby_user_corrections_truncated: bool,
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
    /// Deterministic, rule-based findings. Never an LLM summary — see
    /// [`crate::app::skill_incident_findings`].
    pub findings: skill_incident_findings::SkillIncidentFindings,
}

impl From<db::SkillIncidentEvidence> for SkillIncidentEvidence {
    fn from(v: db::SkillIncidentEvidence) -> Self {
        let incident: SkillIncident = v.incident.into();
        let signal_anchors: Vec<LogEntry> = v.signal_anchors.into_iter().map(Into::into).collect();
        let transcript_before: Vec<LogEntry> =
            v.transcript_before.into_iter().map(Into::into).collect();
        let transcript_after: Vec<LogEntry> =
            v.transcript_after.into_iter().map(Into::into).collect();
        let nearby_tool_failures: Vec<LogEntry> =
            v.nearby_tool_failures.into_iter().map(Into::into).collect();
        let nearby_user_corrections: Vec<LogEntry> = v
            .nearby_user_corrections
            .into_iter()
            .map(Into::into)
            .collect();
        let nearby_logs: Vec<LogEntry> = v.nearby_logs.into_iter().map(Into::into).collect();
        let nearby_errors: Vec<LogEntry> = v.nearby_errors.into_iter().map(Into::into).collect();

        let findings = skill_incident_findings::derive_skill_incident_findings(
            &incident,
            &v.skill_events,
            &signal_anchors,
            &transcript_before,
            &transcript_after,
            &nearby_logs,
            &nearby_errors,
        );

        Self {
            incident,
            skill_events: v.skill_events,
            skill_events_truncated: v.skill_events_truncated,
            signal_anchors,
            signal_anchors_truncated: v.signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated: v.transcript_before_truncated,
            transcript_after,
            transcript_after_truncated: v.transcript_after_truncated,
            nearby_tool_failures,
            nearby_tool_failures_truncated: v.nearby_tool_failures_truncated,
            nearby_user_corrections,
            nearby_user_corrections_truncated: v.nearby_user_corrections_truncated,
            nearby_logs,
            nearby_logs_truncated: v.nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated: v.nearby_errors_truncated,
            findings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncidentSummary {
    pub incident_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillInvestigateResponse {
    pub evidence: Vec<SkillIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
    #[serde(default)]
    pub other_matching_incidents: Vec<SkillIncidentSummary>,
    #[serde(default)]
    pub no_incident_low_severity_summary: bool,
    #[serde(default)]
    pub no_data: bool,
    #[serde(default)]
    pub suggested_filters: Vec<String>,
}
