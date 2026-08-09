use super::*;
use crate::app::models::{LogEntry, McpIncident, McpSignalCounts};

fn log(id: i64, message: &str) -> LogEntry {
    LogEntry {
        id,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hostname: "devhost".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        received_at: "2026-01-01T00:00:00Z".to_string(),
        source_ip: "127.0.0.1:0".to_string(),
        ai_tool: Some("codex".to_string()),
        ai_project: Some("/tmp/project".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        ai_transcript_path: None,
        metadata_json: None,
    }
}

fn incident(signals_present: Vec<&str>) -> McpIncident {
    McpIncident {
        incident_id: "mcp-inc-test".to_string(),
        mcp_server: "labby".to_string(),
        mcp_tool: Some("search".to_string()),
        tool: "codex".to_string(),
        project: "/tmp/project".to_string(),
        session_id: "sess-1".to_string(),
        hostname: "devhost".to_string(),
        first_seen: "2026-01-01T00:00:00Z".to_string(),
        last_seen: "2026-01-01T00:05:00Z".to_string(),
        duration_secs: 300,
        event_count: 3,
        error_count: 2,
        mcp_event_ids: vec![1],
        anchor_log_ids: vec![2],
        signal_counts: McpSignalCounts::default(),
        signals_present: signals_present.into_iter().map(String::from).collect(),
        priority_score: 22.0,
        priority_label: "medium".to_string(),
        window_minutes: 10,
    }
}

#[test]
fn detects_mcp_server_unavailable_category() {
    let inc = incident(vec!["unknown_tool_or_server"]);
    let anchors = vec![log(2, "mcp server error: server unavailable right now")];
    let findings = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MCP_SERVER_UNAVAILABLE),
        "expected mcp_server_unavailable category, got {:?}",
        findings.likely_failure_modes
    );
}

#[test]
fn detects_auth_or_permission_failure_category() {
    let inc = incident(vec!["auth_or_permission_failure"]);
    let anchors = vec![log(2, "permission denied calling the tool")];
    let findings = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MCP_AUTH_OR_PERMISSION_FAILURE)
    );
}

#[test]
fn detects_schema_mismatch_category() {
    let inc = incident(vec!["schema_or_validation_error"]);
    let anchors = vec![log(2, "schema validation failed for the request")];
    let findings = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MCP_SCHEMA_MISMATCH)
    );
}

#[test]
fn detects_timeout_or_rate_limit_category() {
    let inc = incident(vec!["timeout_or_rate_limit"]);
    let anchors = vec![log(2, "the call timed out after 30 seconds")];
    let findings = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MCP_TIMEOUT_OR_RATE_LIMIT)
    );
}

#[test]
fn every_non_unknown_failure_mode_cites_evidence() {
    let inc = incident(vec!["auth_or_permission_failure"]);
    let anchors = vec![log(2, "permission denied calling the tool")];
    let findings = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    for mode in &findings.likely_failure_modes {
        if mode.category != UNKNOWN {
            assert!(
                !mode.evidence_ids.is_empty(),
                "non-unknown category {} must cite evidence",
                mode.category
            );
        }
    }
}

#[test]
fn no_matching_evidence_yields_unknown_and_open_question() {
    let inc = incident(vec![]);
    let findings = derive_mcp_incident_findings(&inc, &[], &[], &[], &[], &[], &[]);
    assert_eq!(findings.likely_failure_modes.len(), 1);
    assert_eq!(findings.likely_failure_modes[0].category, UNKNOWN);
    assert!(!findings.open_questions.is_empty());
}

#[test]
fn repeated_errors_contributing_factor_present_when_threshold_met() {
    let inc = incident(vec![]);
    let findings = derive_mcp_incident_findings(&inc, &[], &[], &[], &[], &[], &[]);
    assert!(
        findings
            .contributing_factors
            .iter()
            .any(|f| f.factor.contains("Repeated tool calls"))
    );
    assert!(
        findings
            .contributing_factors
            .iter()
            .any(|f| f.factor.contains("Error burst"))
    );
}

#[test]
fn deterministic_output_for_identical_input() {
    let inc = incident(vec!["auth_or_permission_failure"]);
    let anchors = vec![log(2, "permission denied calling the tool")];
    let f1 = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    let f2 = derive_mcp_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert_eq!(f1, f2);
}
