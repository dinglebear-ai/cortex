use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiHookIncidentRequest {
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    /// Restrict candidate hook events to a single `evidence_kind` (e.g.
    /// `"runtime_transcript"` to only consider proven-executed hooks).
    pub evidence_kind: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    #[serde(default)]
    pub signals: Vec<String>,
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookSignalCounts {
    pub hook_failed: usize,
    pub hook_timed_out: usize,
    pub hook_output_parse_error: usize,
    pub hook_invoked_too_often: usize,
    pub user_correction_after_hook: usize,
}

impl From<db::HookSignalCounts> for HookSignalCounts {
    fn from(v: db::HookSignalCounts) -> Self {
        Self {
            hook_failed: v.hook_failed,
            hook_timed_out: v.hook_timed_out,
            hook_output_parse_error: v.hook_output_parse_error,
            hook_invoked_too_often: v.hook_invoked_too_often,
            user_correction_after_hook: v.user_correction_after_hook,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIncident {
    pub incident_id: String,
    pub hook_event: String,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub hook_event_count: usize,
    pub hook_event_ids: Vec<i64>,
    pub anchor_log_ids: Vec<i64>,
    /// Signals are incomplete because anchor sampling reached a bound.
    #[serde(default)]
    pub anchor_window_truncated: bool,
    pub signal_counts: HookSignalCounts,
    pub signals_present: Vec<String>,
    pub has_runtime_evidence: bool,
    pub priority_score: f64,
    pub priority_label: String,
    pub window_minutes: u32,
}

impl From<db::HookIncident> for HookIncident {
    fn from(v: db::HookIncident) -> Self {
        Self {
            incident_id: v.incident_id,
            hook_event: v.hook_event,
            hook_name: v.hook_name,
            hook_source: v.hook_source,
            tool: v.tool,
            project: v.project,
            session_id: v.session_id,
            hostname: v.hostname,
            first_seen: v.first_seen,
            last_seen: v.last_seen,
            duration_secs: v.duration_secs,
            hook_event_count: v.hook_event_count,
            hook_event_ids: v.hook_event_ids,
            anchor_log_ids: v.anchor_log_ids,
            anchor_window_truncated: v.anchor_window_truncated,
            signal_counts: v.signal_counts.into(),
            signals_present: v.signals_present,
            has_runtime_evidence: v.has_runtime_evidence,
            priority_score: v.priority_score,
            priority_label: v.priority_label,
            window_minutes: v.window_minutes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiHookIncidentResponse {
    pub incidents: Vec<HookIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiHookInvestigateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIncidentEvidence {
    pub incident: HookIncident,
    pub hook_events: Vec<HookEventEntry>,
    pub hook_events_truncated: bool,
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    pub nearby_tool_calls: Vec<LogEntry>,
    pub nearby_tool_calls_truncated: bool,
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
    /// Deterministic, rule-based findings. Never an LLM summary — see
    /// [`crate::app::hook_incident_findings`].
    pub findings: hook_incident_findings::HookIncidentFindings,
}

impl From<db::HookIncidentEvidence> for HookIncidentEvidence {
    fn from(v: db::HookIncidentEvidence) -> Self {
        let incident: HookIncident = v.incident.into();
        let signal_anchors: Vec<LogEntry> = v.signal_anchors.into_iter().map(Into::into).collect();
        let transcript_before: Vec<LogEntry> =
            v.transcript_before.into_iter().map(Into::into).collect();
        let transcript_after: Vec<LogEntry> =
            v.transcript_after.into_iter().map(Into::into).collect();
        let nearby_tool_calls: Vec<LogEntry> =
            v.nearby_tool_calls.into_iter().map(Into::into).collect();
        let nearby_logs: Vec<LogEntry> = v.nearby_logs.into_iter().map(Into::into).collect();
        let nearby_errors: Vec<LogEntry> = v.nearby_errors.into_iter().map(Into::into).collect();
        let hook_events: Vec<HookEventEntry> = v.hook_events.into_iter().map(Into::into).collect();

        let findings = hook_incident_findings::derive_hook_incident_findings(
            &incident,
            &hook_events,
            &signal_anchors,
            &transcript_before,
            &transcript_after,
            &nearby_tool_calls,
            &nearby_logs,
            &nearby_errors,
        );

        Self {
            incident,
            hook_events,
            hook_events_truncated: v.hook_events_truncated,
            signal_anchors,
            signal_anchors_truncated: v.signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated: v.transcript_before_truncated,
            transcript_after,
            transcript_after_truncated: v.transcript_after_truncated,
            nearby_tool_calls,
            nearby_tool_calls_truncated: v.nearby_tool_calls_truncated,
            nearby_logs,
            nearby_logs_truncated: v.nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated: v.nearby_errors_truncated,
            findings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIncidentSummary {
    pub incident_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
    pub has_runtime_evidence: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiHookInvestigateResponse {
    pub evidence: Vec<HookIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
    #[serde(default)]
    pub other_matching_incidents: Vec<HookIncidentSummary>,
    #[serde(default)]
    pub no_incident_low_severity_summary: bool,
    #[serde(default)]
    pub no_data: bool,
    #[serde(default)]
    pub suggested_filters: Vec<String>,
}
