use super::*;
use crate::app::models::{AbuseIncident, LogEntry};

fn log(id: i64, message: &str) -> LogEntry {
    LogEntry {
        id,
        timestamp: "2026-05-25T00:00:00Z".into(),
        hostname: "nashost".into(),
        facility: None,
        severity: "err".into(),
        app_name: None,
        process_id: None,
        message: message.into(),
        received_at: "2026-05-25T00:00:00Z".into(),
        source_ip: "192.0.2.10:514".into(),
        ai_tool: Some("claude".into()),
        ai_project: Some("/home/jmagar/workspace/cortex".into()),
        ai_session_id: Some("sess-1".into()),
        ai_transcript_path: None,
        metadata_json: None,
    }
}

fn incident(abuse_count: usize) -> AbuseIncident {
    AbuseIncident {
        incident_id: "inc-1".into(),
        project: "/home/jmagar/workspace/cortex".into(),
        tool: "claude".into(),
        session_id: "sess-1".into(),
        hostname: "nashost".into(),
        first_seen: "2026-05-25T00:00:00Z".into(),
        last_seen: "2026-05-25T00:05:00Z".into(),
        duration_secs: 300,
        abuse_count,
        terms: vec!["dang".into()],
        anchor_ids: vec![1],
        priority_score: 1.0,
        priority_label: "medium".into(),
        window_minutes: 10,
    }
}

/// Convenience wrapper: most tests only vary the anchors/nearby logs.
fn derive(
    anchors: &[LogEntry],
    nearby_logs: &[LogEntry],
    nearby_errors: &[LogEntry],
) -> IncidentFindings {
    derive_incident_findings(&incident(1), anchors, &[], &[], nearby_logs, nearby_errors)
}

fn mode<'a>(f: &'a IncidentFindings, category: &str) -> Option<&'a FailureMode> {
    f.likely_failure_modes
        .iter()
        .find(|m| m.category == category)
}

#[test]
fn timeout_evidence_produces_tool_timeout_with_matching_ids() {
    let f = derive(&[log(10, "tool call timed out after 120s")], &[], &[]);
    let m = mode(&f, TOOL_TIMEOUT).expect("tool_timeout mode");
    assert_eq!(m.evidence_ids, vec![10]);
    assert_eq!(m.confidence, "low"); // single hit → conservative
    // Prevention hint is tied to the detected category.
    assert!(
        f.prevention_hints
            .iter()
            .any(|h| h.category == TOOL_TIMEOUT && !h.hint.is_empty())
    );
}

#[test]
fn auth_evidence_produces_auth_or_permission_failure() {
    let f = derive(
        &[log(11, "request returned 401 Unauthorized")],
        &[log(12, "permission denied opening /var/run/docker.sock")],
        &[],
    );
    let m = mode(&f, AUTH_OR_PERMISSION_FAILURE).expect("auth mode");
    assert_eq!(m.evidence_ids, vec![11, 12]);
    assert_eq!(m.confidence, "medium"); // two supporting rows
}

#[test]
fn version_drift_evidence_produces_stale_binary_mode() {
    let f = derive(
        &[log(20, "agent version mismatch: host 1.0 container 0.9")],
        &[],
        &[],
    );
    let m = mode(&f, STALE_BINARY_OR_VERSION_DRIFT).expect("version drift mode");
    assert_eq!(m.evidence_ids, vec![20]);
}

#[test]
fn failing_test_output_produces_test_failure() {
    let f = derive(
        &[log(30, "test result: FAILED. 1 passed; 2 failed")],
        &[log(31, "assertion failed: left == right")],
        &[],
    );
    let m = mode(&f, TEST_FAILURE).expect("test_failure mode");
    assert_eq!(m.evidence_ids, vec![30, 31]);
    assert_eq!(m.confidence, "medium");
}

#[test]
fn high_confidence_requires_three_or_more_supporting_rows() {
    let f = derive(
        &[
            log(40, "database is locked"),
            log(41, "database is locked"),
            log(42, "database is locked"),
        ],
        &[],
        &[],
    );
    let m = mode(&f, DB_BUSY_OR_PERFORMANCE_BOTTLENECK).expect("db mode");
    assert_eq!(m.evidence_ids, vec![40, 41, 42]);
    assert_eq!(m.confidence, "high");
}

#[test]
fn weak_noisy_evidence_produces_unknown_and_open_questions() {
    // Generic chatter with no category keyword must NOT trip a category.
    let f = derive(&[log(50, "thinking about the next step here")], &[], &[]);
    assert_eq!(f.likely_failure_modes.len(), 1);
    let m = &f.likely_failure_modes[0];
    assert_eq!(m.category, UNKNOWN);
    assert!(m.evidence_ids.is_empty());
    assert!(
        !f.open_questions.is_empty(),
        "expected open questions for weak evidence"
    );
    // No category prevention hints emitted when nothing matched.
    assert!(f.prevention_hints.is_empty());
}

#[test]
fn contributing_factors_cite_evidence_ids() {
    let anchors = vec![log(60, "ugh dang it"), log(61, "still broken")];
    let nearby_errors = vec![
        log(70, "connection refused"),
        log(71, "connection refused"),
        log(72, "connection refused"),
    ];
    let f = derive_incident_findings(&incident(4), &anchors, &[], &[], &[], &nearby_errors);
    // Abuse-count factor cites the anchor ids.
    let abuse_factor = f
        .contributing_factors
        .iter()
        .find(|c| c.factor.contains("frustration"))
        .expect("frustration factor");
    assert_eq!(abuse_factor.evidence_ids, vec![60, 61]);
    // Error-burst factor cites the error ids.
    let burst = f
        .contributing_factors
        .iter()
        .find(|c| c.factor.contains("Error burst"))
        .expect("error burst factor");
    assert_eq!(burst.evidence_ids, vec![70, 71, 72]);
}

#[test]
fn findings_are_deterministic_for_fixed_input() {
    let anchors = vec![log(80, "operation timed out"), log(81, "401 unauthorized")];
    let a = derive(&anchors, &[], &[]);
    let b = derive(&anchors, &[], &[]);
    assert_eq!(a, b);
}

#[test]
fn every_failure_mode_cites_evidence_unless_unknown() {
    let f = derive(&[log(90, "command not found: just")], &[], &[]);
    for m in &f.likely_failure_modes {
        if m.category == UNKNOWN {
            continue;
        }
        assert!(
            !m.evidence_ids.is_empty(),
            "non-unknown mode {} must cite evidence",
            m.category
        );
    }
}
