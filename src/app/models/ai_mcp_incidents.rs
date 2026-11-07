use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiMcpIncidentRequest {
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
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
pub struct McpSignalCounts {
    pub repeated_call_failure: usize,
    pub timeout_or_rate_limit: usize,
    pub auth_or_permission_failure: usize,
    pub schema_or_validation_error: usize,
    pub unknown_tool_or_server: usize,
    pub user_correction_after_tool_call: usize,
}

impl From<db::McpSignalCounts> for McpSignalCounts {
    fn from(v: db::McpSignalCounts) -> Self {
        Self {
            repeated_call_failure: v.repeated_call_failure,
            timeout_or_rate_limit: v.timeout_or_rate_limit,
            auth_or_permission_failure: v.auth_or_permission_failure,
            schema_or_validation_error: v.schema_or_validation_error,
            unknown_tool_or_server: v.unknown_tool_or_server,
            user_correction_after_tool_call: v.user_correction_after_tool_call,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIncident {
    pub incident_id: String,
    pub mcp_server: String,
    pub mcp_tool: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub event_count: usize,
    pub error_count: usize,
    pub mcp_event_ids: Vec<i64>,
    pub anchor_log_ids: Vec<i64>,
    /// Signals are incomplete because anchor sampling reached a bound.
    #[serde(default)]
    pub anchor_window_truncated: bool,
    pub signal_counts: McpSignalCounts,
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    pub priority_label: String,
    pub window_minutes: u32,
}

impl From<db::McpIncident> for McpIncident {
    fn from(v: db::McpIncident) -> Self {
        Self {
            incident_id: v.incident_id,
            mcp_server: v.mcp_server,
            mcp_tool: v.mcp_tool,
            tool: v.tool,
            project: v.project,
            session_id: v.session_id,
            hostname: v.hostname,
            first_seen: v.first_seen,
            last_seen: v.last_seen,
            duration_secs: v.duration_secs,
            event_count: v.event_count,
            error_count: v.error_count,
            mcp_event_ids: v.mcp_event_ids,
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
pub struct AiMcpIncidentResponse {
    pub incidents: Vec<McpIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiMcpInvestigateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIncidentEvidence {
    pub incident: McpIncident,
    pub mcp_events: Vec<db::AiMcpEventEntry>,
    pub mcp_events_truncated: bool,
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    pub nearby_user_corrections: Vec<LogEntry>,
    pub nearby_user_corrections_truncated: bool,
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
    /// Deterministic, rule-based findings. Never an LLM summary — see
    /// [`crate::app::mcp_incident_findings`].
    pub findings: mcp_incident_findings::McpIncidentFindings,
}

impl From<db::McpIncidentEvidence> for McpIncidentEvidence {
    fn from(v: db::McpIncidentEvidence) -> Self {
        let incident: McpIncident = v.incident.into();
        let signal_anchors: Vec<LogEntry> = v.signal_anchors.into_iter().map(Into::into).collect();
        let transcript_before: Vec<LogEntry> =
            v.transcript_before.into_iter().map(Into::into).collect();
        let transcript_after: Vec<LogEntry> =
            v.transcript_after.into_iter().map(Into::into).collect();
        let nearby_user_corrections: Vec<LogEntry> = v
            .nearby_user_corrections
            .into_iter()
            .map(Into::into)
            .collect();
        let nearby_logs: Vec<LogEntry> = v.nearby_logs.into_iter().map(Into::into).collect();
        let nearby_errors: Vec<LogEntry> = v.nearby_errors.into_iter().map(Into::into).collect();

        let findings = mcp_incident_findings::derive_mcp_incident_findings(
            &incident,
            &v.mcp_events,
            &signal_anchors,
            &transcript_before,
            &transcript_after,
            &nearby_logs,
            &nearby_errors,
        );

        Self {
            incident,
            mcp_events: v.mcp_events,
            mcp_events_truncated: v.mcp_events_truncated,
            signal_anchors,
            signal_anchors_truncated: v.signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated: v.transcript_before_truncated,
            transcript_after,
            transcript_after_truncated: v.transcript_after_truncated,
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
pub struct McpIncidentSummary {
    pub incident_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiMcpInvestigateResponse {
    pub evidence: Vec<McpIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
    #[serde(default)]
    pub other_matching_incidents: Vec<McpIncidentSummary>,
    #[serde(default)]
    pub no_incident_low_severity_summary: bool,
    #[serde(default)]
    pub no_data: bool,
    #[serde(default)]
    pub suggested_filters: Vec<String>,
}
