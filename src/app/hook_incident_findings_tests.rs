use super::*;
use crate::app::models::{HookEventEntry, HookIncident, HookSignalCounts, LogEntry};

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
        ai_tool: Some("claude".to_string()),
        ai_project: Some("/tmp/project".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        ai_transcript_path: None,
        metadata_json: None,
    }
}

fn hook_event_entry(id: i64) -> HookEventEntry {
    HookEventEntry {
        id,
        log_id: Some(id),
        ai_tool: "claude".to_string(),
        ai_project: Some("/tmp/project".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hook_event: "PostToolUse".to_string(),
        hook_name: Some("format-on-save".to_string()),
        hook_source: None,
        hook_command: None,
        status: "failed".to_string(),
        exit_code: Some(1),
        duration_ms: None,
        stdout_preview: None,
        stderr_preview: None,
        persisted_output_path: None,
        trusted_hash: None,
        evidence_kind: "runtime_transcript".to_string(),
        metadata_json: None,
    }
}

fn incident(signal_counts: HookSignalCounts, has_runtime_evidence: bool) -> HookIncident {
    let signals_present = {
        let mut s = Vec::new();
        if signal_counts.hook_failed > 0 {
            s.push("hook_failed".to_string());
        }
        if signal_counts.hook_timed_out > 0 {
            s.push("hook_timed_out".to_string());
        }
        if signal_counts.hook_output_parse_error > 0 {
            s.push("hook_output_parse_error".to_string());
        }
        if signal_counts.hook_invoked_too_often > 0 {
            s.push("hook_invoked_too_often".to_string());
        }
        if signal_counts.user_correction_after_hook > 0 {
            s.push("user_correction_after_hook".to_string());
        }
        s
    };
    HookIncident {
        incident_id: "hook-inc-test".to_string(),
        hook_event: "PostToolUse".to_string(),
        hook_name: Some("format-on-save".to_string()),
        hook_source: None,
        tool: "claude".to_string(),
        project: "/tmp/project".to_string(),
        session_id: "sess-1".to_string(),
        hostname: "devhost".to_string(),
        first_seen: "2026-01-01T00:00:00Z".to_string(),
        last_seen: "2026-01-01T00:05:00Z".to_string(),
        duration_secs: 300,
        hook_event_count: 1,
        hook_event_ids: vec![1],
        anchor_log_ids: vec![2],
        signal_counts,
        signals_present,
        has_runtime_evidence,
        priority_score: 22.0,
        priority_label: "medium".to_string(),
        window_minutes: 10,
    }
}

#[test]
fn detects_hook_failed_category_with_evidence_ids() {
    let counts = HookSignalCounts {
        hook_failed: 2,
        ..Default::default()
    };
    let inc = incident(counts, true);
    let hook_events = vec![hook_event_entry(1)];
    let findings = derive_hook_incident_findings(&inc, &hook_events, &[], &[], &[], &[], &[], &[]);
    let mode = findings
        .likely_failure_modes
        .iter()
        .find(|f| f.category == HOOK_FAILED)
        .expect("expected hook_failed finding");
    assert_eq!(mode.evidence_ids, vec![1]);
    assert!(
        findings
            .prevention_hints
            .iter()
            .any(|h| h.category == HOOK_FAILED)
    );
}

#[test]
fn detects_hook_timed_out_category() {
    let counts = HookSignalCounts {
        hook_timed_out: 1,
        ..Default::default()
    };
    let inc = incident(counts, true);
    let hook_events = vec![hook_event_entry(1)];
    let findings = derive_hook_incident_findings(&inc, &hook_events, &[], &[], &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == HOOK_TIMED_OUT)
    );
}

#[test]
fn detects_user_correction_as_blocked_agent_flow() {
    let counts = HookSignalCounts {
        user_correction_after_hook: 1,
        ..Default::default()
    };
    let inc = incident(counts, true);
    let anchors = vec![log(2, "that's not what I asked for")];
    let findings = derive_hook_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[], &[]);
    let mode = findings
        .likely_failure_modes
        .iter()
        .find(|f| f.category == HOOK_BLOCKED_AGENT_FLOW)
        .expect("expected hook_blocked_agent_flow finding");
    assert_eq!(mode.evidence_ids, vec![2]);
}

#[test]
fn runtime_evidence_basis_differs_from_config_only() {
    let inc_runtime = incident(HookSignalCounts::default(), true);
    let findings_runtime =
        derive_hook_incident_findings(&inc_runtime, &[], &[], &[], &[], &[], &[], &[]);
    assert!(
        findings_runtime
            .evidence_basis
            .contains("runtime_transcript")
    );
    assert!(
        findings_runtime
            .open_questions
            .iter()
            .all(|q| { !q.contains("configuration/trust-state inventory only") })
    );

    let inc_config = incident(HookSignalCounts::default(), false);
    let findings_config =
        derive_hook_incident_findings(&inc_config, &[], &[], &[], &[], &[], &[], &[]);
    assert!(
        findings_config
            .evidence_basis
            .contains("config_inventory/trusted_hash_state")
    );
    assert!(
        findings_config
            .open_questions
            .iter()
            .any(|q| q.contains("configuration/trust-state inventory only")),
        "expected an explicit config-only caveat in open_questions, got {:?}",
        findings_config.open_questions
    );
}

#[test]
fn no_signals_yields_unknown_and_open_question() {
    let inc = incident(HookSignalCounts::default(), true);
    let findings = derive_hook_incident_findings(&inc, &[], &[], &[], &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == UNKNOWN)
    );
    assert!(!findings.open_questions.is_empty());
}

#[test]
fn every_non_unknown_finding_cites_evidence() {
    let counts = HookSignalCounts {
        hook_failed: 1,
        hook_output_parse_error: 1,
        ..Default::default()
    };
    let inc = incident(counts, true);
    let hook_events = vec![hook_event_entry(1)];
    let findings = derive_hook_incident_findings(&inc, &hook_events, &[], &[], &[], &[], &[], &[]);
    for mode in &findings.likely_failure_modes {
        if mode.category != UNKNOWN {
            assert!(
                !mode.evidence_ids.is_empty(),
                "category {} has no evidence ids",
                mode.category
            );
        }
    }
}
