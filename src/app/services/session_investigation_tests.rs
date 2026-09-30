use super::super::session_investigation_support::looks_like_linear_identifier;
use super::*;

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

    let (refs, _) = extract_session_external_references(&events, 100);

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

fn fixture_service() -> (tempfile::TempDir, CortexService) {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(dir.path().join("investigate.db"));
    let pool = std::sync::Arc::new(db::init_pool(&storage).unwrap());
    (dir, CortexService::new(pool, storage))
}

fn fixture_session() -> AiSessionEntry {
    db::AiSessionEntry {
        ai_project: "p".into(),
        ai_tool: "codex".into(),
        ai_session_id: "s".into(),
        ai_transcript_path: None,
        hostname: "h".into(),
        first_seen: "2026-09-21T00:00:00Z".into(),
        last_seen: "2026-09-21T00:10:00Z".into(),
        event_count: 1,
        title: None,
        title_provenance: None,
    }
    .into()
}

#[tokio::test]
async fn notification_identity_and_end_window_are_applied_before_limit() {
    let (_dir, service) = fixture_service();
    let conn = service.pool.get().unwrap();
    for (host, time) in [
        ("h", "2026-09-21T00:00:00Z"),
        ("h", "2026-09-21T00:05:00Z"),
        ("h", "2026-09-21T00:10:00Z"),
        ("h", "2026-09-21T01:00:00Z"),
    ] {
        conn.execute("INSERT INTO notification_firings(outbox_id,rule_id,severity,hostname,fired_at) VALUES(1,'r','err',?1,?2)", rusqlite::params![host,time]).unwrap();
    }
    for _ in 0..501 {
        conn.execute("INSERT INTO notification_firings(outbox_id,rule_id,severity,hostname,fired_at) VALUES(1,'r','err','other','2026-09-21T00:09:00Z')", []).unwrap();
    }
    drop(conn);
    let rows = super::super::session_investigation_sections::session_notifications(
        &service,
        &fixture_session(),
        2,
    )
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        3,
        "sentinel detects truncation among matching rows"
    );
    assert!(rows.iter().all(|row| row.hostname == "h"));
    assert!(
        rows.iter()
            .all(|row| row.fired_at.as_str() <= "2026-09-21T00:10:00Z")
    );
}

#[test]
fn artifact_union_is_deduplicated_bounded_and_signals_overflow() {
    let entry = |id| models::ArtifactEvidenceEntry {
        cortex_log_id: id,
        event: serde_json::from_value(serde_json::json!({
            "schemaVersion": "dinglebear.cortex-artifact-evidence/v1", "eventId": format!("e-{id}"),
            "eventKind": "installed", "sourceSystem": "test", "sourceIssuer": "fixture",
            "observedAt": "2026-09-21T00:05:00Z"
        }))
        .unwrap(),
    };
    let (events, truncated) = super::super::session_investigation_sections::merge_artifact_evidence(
        models::ListArtifactEvidenceResponse {
            events: vec![entry(1), entry(2)],
            truncated: false,
        },
        models::ListArtifactEvidenceResponse {
            events: vec![entry(2), entry(3)],
            truncated: false,
        },
        2,
    );
    assert_eq!(
        events
            .iter()
            .map(|item| item.cortex_log_id)
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert!(truncated);
}

#[test]
fn external_references_deduplicate_across_positions_and_cap_unique_values() {
    let events = vec![event(1, "#123 #456"), event(2, "#123 #789")];
    let (refs, truncated) = extract_session_external_references(&events, 2);
    assert!(truncated);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs.iter().filter(|item| item.value == "#123").count(), 1);
    assert!(
        refs.iter()
            .all(|item| !item.verified && item.trust_level == "claimed")
    );
}

#[tokio::test]
async fn related_sessions_signal_truncation() {
    let (_dir, service) = fixture_service();
    let conn = service.pool.get().unwrap();
    for id in 0..23 {
        conn.execute("INSERT INTO logs(timestamp,hostname,severity,message,raw,source_ip,ai_tool,ai_project,ai_session_id) VALUES('2026-09-21T00:05:00Z','h','info','message','','fixture','codex','p',?1)", [format!("session-{id}")]).unwrap();
    }
    drop(conn);
    let (rows, truncated) = related_sessions_for_investigation(&service, &fixture_session())
        .await
        .unwrap();
    assert_eq!(rows.len(), 20);
    assert!(truncated);
}
