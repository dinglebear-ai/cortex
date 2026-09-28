use super::super::session_investigation_support::{
    looks_like_linear_identifier, timestamp_inclusive_between,
};
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

    let (refs, truncated) = extract_session_external_references(&events, 100);
    assert!(!truncated);
    assert!(
        refs.iter()
            .all(|reference| !reference.verified && reference.trust_level == "claimed")
    );

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

#[test]
fn external_references_are_bounded_and_cannot_spoof_github_hosts() {
    let text = (0..1000)
        .map(|id| format!("U8-{id}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (refs, truncated) = extract_session_external_references(&[event(1, &text)], 3);
    assert_eq!(refs.len(), 3);
    assert!(truncated);
    let (refs, truncated) = extract_session_external_references(
        &[event(
            1,
            "https://evil.invalid/github.com/owner/repo/pull/123",
        )],
        3,
    );
    assert!(!truncated);
    assert_eq!(refs[0].kind, models::SessionExternalReferenceKind::Url);
}

fn service_fixture() -> (CortexService, std::sync::Arc<db::DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage =
        crate::config::StorageConfig::for_test(dir.path().join("session-investigation.db"));
    let pool = std::sync::Arc::new(db::init_pool(&storage).unwrap());
    (
        CortexService::new(std::sync::Arc::clone(&pool), storage),
        pool,
        dir,
    )
}

fn agent_command_log(ts: &str, host: &str, session: &str, cwd: &str) -> crate::db::LogBatchEntry {
    crate::db::LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("claude".to_string()),
        process_id: None,
        message: "cargo test".to_string(),
        raw: "cargo test".to_string(),
        source_ip: format!("agent-command://{host}/claude/{session}"),
        docker_checkpoint: None,
        ai_tool: Some("claude".to_string()),
        ai_project: Some(cwd.to_string()),
        ai_session_id: Some(session.to_string()),
        ai_transcript_path: None,
        metadata_json: Some(format!(
            r#"{{"source_kind":"agent-command","agent_command":{{"cwd":"{cwd}"}}}}"#
        )),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: Some("command".to_string()),
        parse_error: None,
    }
}

#[tokio::test]
async fn session_investigation_executes_against_sqlite_and_rejects_ambiguous_identity() {
    let (service, pool, _dir) = service_fixture();
    db::insert_logs_batch(
        &pool,
        &[
            agent_command_log("2026-09-21T00:00:00Z", "host", "target", "/project"),
            agent_command_log("2026-09-21T00:10:00Z", "host", "target", "/project"),
        ],
    )
    .unwrap();
    let result = service
        .session_investigate(models::SessionInvestigateRequest {
            session_id: "target".into(),
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.result.session.session_id, "target");
    assert_eq!(result.result.session.hostname, "host");
    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(bytes <= result.metadata.payload_limit_bytes as usize);
    assert_eq!(bytes, result.metadata.budget_used.payload_bytes as usize);
    assert!(matches!(
        service
            .session_investigate(models::SessionInvestigateRequest::default())
            .await,
        Err(ServiceError::InvalidInput(_))
    ));
    assert!(matches!(
        service
            .session_investigate(models::SessionInvestigateRequest {
                session_id: "missing".into(),
                ..Default::default()
            })
            .await,
        Err(ServiceError::NotFound(_))
    ));
    db::insert_logs_batch(
        &pool,
        &[agent_command_log(
            "2026-09-21T00:05:00Z",
            "other",
            "target",
            "/project",
        )],
    )
    .unwrap();
    assert!(matches!(
        service
            .session_investigate(models::SessionInvestigateRequest {
                session_id: "target".into(),
                ..Default::default()
            })
            .await,
        Err(ServiceError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn session_investigation_payload_budget_counts_utf8_and_preserves_identity() {
    let (service, pool, _dir) = service_fixture();
    db::insert_logs_batch(
        &pool,
        &[agent_command_log(
            "2026-09-21T00:00:00Z",
            "host",
            "target",
            "/project",
        )],
    )
    .unwrap();
    let mut result = service
        .session_investigate(models::SessionInvestigateRequest {
            session_id: "target".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    result.result.transcript = vec![event(1, &"😀\"".repeat(40_000))];
    super::super::session_investigation_bounds::bound_session_payload(&mut result).unwrap();
    assert!(result.metadata.partial);
    assert!(result.result.transcript_has_more);
    assert!(
        result
            .metadata
            .partial_reasons
            .iter()
            .any(|reason| reason == "payload_budget_exceeded")
    );
    assert_eq!(result.result.session.session_id, "target");
    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(bytes <= 65_536);
    assert_eq!(bytes, result.metadata.budget_used.payload_bytes as usize);
}

#[test]
fn artifact_union_is_bounded_after_deduplication() {
    let item = |id: i64| models::ArtifactEvidenceEntry {
        cortex_log_id: id,
        event: serde_json::from_value(serde_json::json!({
            "schemaVersion": "dinglebear.cortex-artifact-evidence/v1",
            "eventId": format!("event-{id}"), "eventKind": "runtime_call",
            "sourceSystem": "test", "sourceIssuer": "test",
            "observedAt": "2026-09-21T00:00:00Z"
        }))
        .unwrap(),
    };
    let page = |ids: &[i64]| models::ListArtifactEvidenceResponse {
        events: ids.iter().copied().map(item).collect(),
        truncated: false,
    };
    let (rows, truncated) = super::super::session_investigation_support::merge_session_artifacts(
        page(&[1, 2]),
        page(&[2, 3]),
        2,
    );
    assert_eq!(
        rows.iter().map(|row| row.cortex_log_id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(truncated);
    let (rows, truncated) = super::super::session_investigation_support::merge_session_artifacts(
        page(&[1, 2]),
        page(&[2, 1]),
        2,
    );
    assert_eq!(rows.len(), 2);
    assert!(!truncated);
}

#[test]
fn attributed_commit_reads_are_bounded_before_hydration() {
    let (_service, pool, _dir) = service_fixture();
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO repositories(repository_key,hostname,common_git_dir,primary_path,display_name,first_seen_at,last_seen_at) VALUES('repo','host','/git','/repo','repo','2026-09-21T00:00:00Z','2026-09-21T00:00:00Z')", []).unwrap();
    let repository_id = conn.last_insert_rowid();
    conn.execute("INSERT INTO agent_runs(run_key,native_session_id,tool,hostname,status,status_observed_at,started_at,last_activity_at) VALUES('run','session','codex','host','active','2026-09-21T00:00:00Z','2026-09-21T00:00:00Z','2026-09-21T00:00:00Z')", []).unwrap();
    let run_id = conn.last_insert_rowid();
    for id in 0..12 {
        conn.execute("INSERT INTO git_commits(repository_id,sha,first_observed_at,last_observed_at) VALUES(?1,?2,'2026-09-21T00:00:00Z','2026-09-21T00:00:00Z')", rusqlite::params![repository_id, format!("{id:040x}")]).unwrap();
        let commit_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO agent_run_commits(relation_key,run_id,commit_id,evidence_kind,evidence_source,trust_level,confidence,first_seen_at,last_seen_at) VALUES(?1,?2,?3,'command','test','verified',1.0,'2026-09-21T00:00:00Z','2026-09-21T00:00:00Z')", rusqlite::params![format!("relation-{id}"), run_id, commit_id]).unwrap();
    }
    drop(conn);
    let rows = db::agent_observatory::list_agent_run_attributed_commits(&pool, run_id, 1).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "one requested row plus a truncation sentinel"
    );
    assert!(rows.iter().all(|row| row.relation.run_id == run_id));
}
