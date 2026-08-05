use super::*;

#[test]
fn investigation_human_output_includes_findings_context_and_related_incidents() {
    let response: AiHookInvestigateResponse = serde_json::from_value(serde_json::json!({
        "evidence": [{
            "incident": {
                "incident_id": "hook-main",
                "hook_event": "PostToolUse",
                "hook_name": "format-on-save",
                "hook_source": "settings",
                "tool": "codex",
                "project": "cortex",
                "session_id": "session-1",
                "hostname": "devhost",
                "first_seen": "2026-07-16T12:00:00Z",
                "last_seen": "2026-07-16T12:01:00Z",
                "duration_secs": 60,
                "hook_event_count": 1,
                "hook_event_ids": [7],
                "anchor_log_ids": [9],
                "signal_counts": {
                    "hook_failed": 1,
                    "hook_timed_out": 0,
                    "hook_output_parse_error": 0,
                    "hook_invoked_too_often": 0,
                    "user_correction_after_hook": 0
                },
                "signals_present": ["hook_failed"],
                "has_runtime_evidence": true,
                "priority_score": 50.0,
                "priority_label": "high",
                "window_minutes": 5
            },
            "hook_events": [],
            "hook_events_truncated": true,
            "signal_anchors": [],
            "signal_anchors_truncated": false,
            "transcript_before": [],
            "transcript_before_truncated": true,
            "transcript_after": [],
            "transcript_after_truncated": false,
            "nearby_tool_calls": [],
            "nearby_tool_calls_truncated": false,
            "nearby_logs": [],
            "nearby_logs_truncated": false,
            "nearby_errors": [],
            "nearby_errors_truncated": false,
            "findings": {
                "likely_failure_modes": [{
                    "category": "hook_failed",
                    "confidence": "high",
                    "evidence_ids": [7]
                }],
                "contributing_factors": [{
                    "factor": "formatter exited non-zero",
                    "evidence_ids": [7]
                }],
                "prevention_hints": [{
                    "category": "hook_failed",
                    "hint": "Make the formatter failure non-blocking."
                }],
                "open_questions": ["Was the formatter installed?"],
                "evidence_basis": "Runtime hook execution evidence is present."
            }
        }],
        "total_incidents": 2,
        "truncated": true,
        "other_matching_incidents": [{
            "incident_id": "hook-related",
            "first_seen": "2026-07-15T12:00:00Z",
            "last_seen": "2026-07-15T12:01:00Z",
            "priority_score": 25.0,
            "priority_label": "medium",
            "has_runtime_evidence": false
        }],
        "no_incident_low_severity_summary": false,
        "no_data": false,
        "suggested_filters": []
    }))
    .unwrap();

    let output = render_ai_hook_investigate_response(&response);

    assert!(output.contains("hook_failed"));
    assert!(output.contains("Make the formatter failure non-blocking."));
    assert!(output.contains("formatter exited non-zero"));
    assert!(output.contains("Was the formatter installed?"));
    assert!(output.contains("hook_events=0 (truncated)"));
    assert!(output.contains("transcript_before=0 (truncated)"));
    assert!(output.contains("Runtime hook execution evidence is present."));
    assert!(output.contains("related incident(s)"));
    assert!(output.contains("hook-related"));
}
