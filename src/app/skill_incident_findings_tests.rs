use super::*;
use crate::app::models::{LogEntry, SkillIncident, SkillSignalCounts};

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

fn incident(signals_present: Vec<&str>) -> SkillIncident {
    SkillIncident {
        incident_id: "skill-inc-test".to_string(),
        skill_name: "lavra:lavra-plan".to_string(),
        skill_plugin: Some("lavra".to_string()),
        tool: "codex".to_string(),
        project: "/tmp/project".to_string(),
        session_id: "sess-1".to_string(),
        hostname: "devhost".to_string(),
        first_seen: "2026-01-01T00:00:00Z".to_string(),
        last_seen: "2026-01-01T00:05:00Z".to_string(),
        duration_secs: 300,
        skill_event_count: 1,
        skill_event_ids: vec![1],
        anchor_log_ids: vec![2],
        signal_counts: SkillSignalCounts::default(),
        signals_present: signals_present.into_iter().map(String::from).collect(),
        priority_score: 22.0,
        priority_label: "medium".to_string(),
        window_minutes: 10,
    }
}

#[test]
fn detects_wrong_source_of_truth_category() {
    let inc = incident(vec!["scope_or_source_confusion"]);
    let anchors = vec![log(
        2,
        "you're using the wrong source of truth here, check the live container",
    )];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == WRONG_SOURCE_OF_TRUTH),
        "expected wrong_source_of_truth category, got {:?}",
        findings.likely_failure_modes
    );
    let mode = findings
        .likely_failure_modes
        .iter()
        .find(|f| f.category == WRONG_SOURCE_OF_TRUTH)
        .unwrap();
    assert_eq!(mode.evidence_ids, vec![2]);
    assert!(
        findings
            .prevention_hints
            .iter()
            .any(|h| h.category == WRONG_SOURCE_OF_TRUTH
                && h.hint.to_ascii_lowercase().contains("source of truth"))
    );
}

#[test]
fn detects_missing_verification_step_category() {
    let inc = incident(vec!["ignored_skill_or_policy_instruction"]);
    let anchors = vec![log(
        2,
        "you claimed success without any verification of the running app",
    )];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == MISSING_VERIFICATION_STEP)
    );
    let hint = findings
        .prevention_hints
        .iter()
        .find(|h| h.category == MISSING_VERIFICATION_STEP)
        .unwrap();
    assert!(hint.hint.to_ascii_lowercase().contains("verification"));
}

#[test]
fn detects_overly_broad_research_loop_category() {
    let inc = incident(vec!["overlong_loop_after_skill"]);
    let counts = SkillSignalCounts {
        overlong_loop_after_skill: 1,
        ..Default::default()
    };
    let mut inc2 = inc;
    inc2.signal_counts = counts;
    let anchors = vec![log(
        2,
        "that's not what I asked, we wasted twenty minutes going in circles",
    )];
    let findings = derive_skill_incident_findings(&inc2, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == OVERLY_BROAD_RESEARCH_LOOP)
    );
}

#[test]
fn detects_ambiguous_skill_trigger_category() {
    let inc = incident(vec!["skill_scope_mismatch"]);
    let anchors = vec![log(
        2,
        "wrong skill triggered, this wasn't the right one for the task",
    )];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == AMBIGUOUS_SKILL_TRIGGER || f.category == SKILL_SCOPE_MISMATCH),
        "expected ambiguous_skill_trigger or skill_scope_mismatch, got {:?}",
        findings.likely_failure_modes
    );
}

#[test]
fn weak_evidence_falls_back_to_unknown_with_open_questions() {
    let inc = incident(vec![]);
    let findings = derive_skill_incident_findings(&inc, &[], &[], &[], &[], &[], &[]);
    assert!(
        findings
            .likely_failure_modes
            .iter()
            .any(|f| f.category == UNKNOWN)
    );
    assert!(!findings.open_questions.is_empty());
}

#[test]
fn every_finding_cites_evidence_ids_when_not_unknown() {
    let inc = incident(vec!["tool_failure_after_skill"]);
    let anchors = vec![log(2, "command exited with exit code 1")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
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

#[test]
fn prevention_hints_are_skill_doc_actionable() {
    let inc = incident(vec!["scope_or_source_confusion"]);
    let anchors = vec![log(2, "wrong repo, this is stale data not the live system")];
    let findings = derive_skill_incident_findings(&inc, &[], &anchors, &[], &[], &[], &[]);
    for hint in &findings.prevention_hints {
        assert!(
            hint.hint.len() > 20,
            "hint should be a concrete actionable sentence: {}",
            hint.hint
        );
    }
}
