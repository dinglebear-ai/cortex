use super::*;
use super::super::session_investigation_support::looks_like_linear_identifier;


fn event(position: i64, text: &str) -> models::RenderedSessionEvent {
    models::RenderedSessionEvent {
        position,
        timestamp: "2026-09-21T00:00:00Z".to_string(),
        kind: models::RenderedSessionEventKind::Assistant,
        text: text.to_string(),
        redacted: false,
        parse_warning: None,
    }
}

#[test]
fn external_references_are_classified_and_deduplicated() {
    let events = vec![
        event(
            1,
            "Worked on CLD-1149 and https://github.com/dinglebear-ai/labby/pull/717 with commit abc1234.",
        ),
        event(
            2,
            "Follow-up #731 https://github.com/dinglebear-ai/labby/issues/723 https://linear.app/lime-technology/issue/CLD-1149/foo",
        ),
    ];

    let refs = extract_session_external_references(&events);

    assert!(refs.iter().any(|item| {
        item.kind == models::SessionExternalReferenceKind::LinearIssue && item.value == "CLD-1149"
    }));
    assert!(refs.iter().any(|item| {
        item.kind == models::SessionExternalReferenceKind::GithubPullRequest
            && item.value.contains("/pull/717")
    }));
    assert!(refs.iter().any(|item| {
        item.kind == models::SessionExternalReferenceKind::GithubIssue
            && item.value.contains("/issues/723")
    }));
    assert!(refs.iter().any(|item| {
        item.kind == models::SessionExternalReferenceKind::GithubReference && item.value == "#731"
    }));
    assert!(refs.iter().any(|item| {
        item.kind == models::SessionExternalReferenceKind::CommitSha && item.value == "abc1234"
    }));
}

#[test]
fn linear_identifier_parser_rejects_ordinary_hyphenated_text() {
    assert!(looks_like_linear_identifier("U8-1122"));
    assert!(looks_like_linear_identifier("CLD-1149"));
    assert!(!looks_like_linear_identifier("session-investigate"));
    assert!(!looks_like_linear_identifier("abc-123"));
    assert!(!looks_like_linear_identifier("CLD-next"));
}

fn correlated_log(id: i64, source_kind: Option<&str>) -> CorrelatedLogRow {
    CorrelatedLogRow {
        entry: models::LogEntry {
            id,
            timestamp: "2026-09-21T00:00:00Z".to_string(),
            hostname: "macpoo".to_string(),
            facility: None,
            severity: "info".to_string(),
            app_name: Some("test".to_string()),
            process_id: None,
            message: format!("log-{id}"),
            received_at: "2026-09-21T00:00:00Z".to_string(),
            source_ip: "test://source".to_string(),
            ai_tool: None,
            ai_project: None,
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: None,
        },
        source_kind: source_kind.map(str::to_string),
        discovery: "test".to_string(),
    }
}

#[test]
fn source_evidence_groups_source_kinds_and_bounds_log_ids() {
    let correlation = GraphSessionCorrelation {
        session_id: "session-1".to_string(),
        session_start: "2026-09-21T00:00:00Z".to_string(),
        session_end: "2026-09-21T00:10:00Z".to_string(),
        used_graph: true,
        session_entity_keys: vec!["project:codex:session-1".to_string()],
        discovered_hosts: Vec::new(),
        discovered_entities: Vec::new(),
        logs: vec![
            correlated_log(1, Some("docker-stream")),
            correlated_log(2, Some("docker-stream")),
            correlated_log(3, Some("agent-command")),
            correlated_log(4, None),
        ],
        agent_command_count: 1,
        shell_history_count: 0,
        heartbeat_summaries: Vec::new(),
        truncated: false,
    };

    let summaries = summarize_session_source_evidence(Some(&correlation), 1);

    assert_eq!(summaries["docker-stream"].count, 2);
    assert_eq!(summaries["docker-stream"].log_ids, vec![1]);
    assert!(summaries["docker-stream"].truncated);
    assert_eq!(summaries["agent-command"].log_ids, vec![3]);
    assert_eq!(summaries["unknown"].log_ids, vec![4]);
}

#[test]
fn timestamp_window_is_inclusive_and_rejects_invalid_values() {
    let start = "2026-09-21T00:00:00Z";
    let end = "2026-09-21T00:10:00Z";

    assert!(timestamp_inclusive_between(start, start, end));
    assert!(timestamp_inclusive_between(end, start, end));
    assert!(timestamp_inclusive_between(
        "2026-09-21T00:05:00Z",
        start,
        end
    ));
    assert!(!timestamp_inclusive_between(
        "2026-09-21T00:11:00Z",
        start,
        end
    ));
    assert!(!timestamp_inclusive_between("not-a-time", start, end));
}
