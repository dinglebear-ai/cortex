use super::*;
use crate::db::{LogBatchEntry, init_pool, insert_logs_batch};

fn fixture() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("scope.db"))).unwrap();
    (pool, dir)
}

fn row(time: &str, host: &str, project: &str, tool: &str, session: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: time.into(),
        hostname: host.into(),
        severity: "info".into(),
        message: format!("{project}/{tool}/{host}"),
        raw: "fixture".into(),
        source_ip: "fixture://scope".into(),
        ai_project: Some(project.into()),
        ai_tool: Some(tool.into()),
        ai_session_id: Some(session.into()),
        facility: None,
        app_name: None,
        process_id: None,
        docker_checkpoint: None,
        ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

fn scope() -> SessionGraphScope {
    SessionGraphScope {
        project: "chosen".into(),
        tool: "codex".into(),
        host: "host-a".into(),
        severity_in: vec!["info".into()],
    }
}

fn graph_entity(conn: &rusqlite::Connection, kind: &str, key: &str) -> i64 {
    conn.execute(
        "INSERT INTO graph_entities(entity_type,canonical_key,display_label,trust_level)
        VALUES(?1,?2,?2,'verified')",
        params![kind, key],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn fallback_reused_native_ids_preserve_selected_project_tool_host_and_bounds() {
    let (pool, _dir) = fixture();
    insert_logs_batch(
        &pool,
        &[
            row(
                "2026-09-21T08:00:00Z",
                "host-a",
                "chosen",
                "CODEX",
                "reused_%",
            ),
            row(
                "2026-09-21T09:00:00Z",
                "host-b",
                "chosen",
                "codex",
                "reused_%",
            ),
            row(
                "2026-09-21T10:00:00Z",
                "host-a",
                "other",
                "codex",
                "reused_%",
            ),
            row(
                "2026-09-21T11:00:00Z",
                "host-a",
                "chosen",
                "claude",
                "reused_%",
            ),
        ],
    )
    .unwrap();
    let result = correlate_session_graph_scoped(&pool, "reused_%", &scope(), 10).unwrap();
    assert!(!result.used_graph);
    assert_eq!(result.logs.len(), 1);
    assert_eq!(result.logs[0].message, "chosen/CODEX/host-a");
    assert_eq!(
        result.bounds,
        Some(("2026-09-21T08:00:00Z".into(), "2026-09-21T08:00:00Z".into()))
    );
}

#[test]
fn graph_uses_exact_seed_and_keeps_host_context_without_other_session_transcripts() {
    let (pool, _dir) = fixture();
    let mut context = row(
        "2026-09-21T08:01:00Z",
        "host-a",
        "unused",
        "unused",
        "unused",
    );
    context.ai_session_id = None;
    context.ai_project = None;
    context.ai_tool = None;
    context.message = "host context".into();
    insert_logs_batch(
        &pool,
        &[
            row(
                "2026-09-21T08:00:00Z",
                "host-a",
                "chosen",
                "codex",
                "reused_%",
            ),
            row(
                "2026-09-21T08:02:00Z",
                "host-a",
                "chosen",
                "codex",
                "reused_%",
            ),
            row(
                "2026-09-21T08:01:00Z",
                "host-a",
                "other",
                "codex",
                "reused_%",
            ),
            context,
        ],
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        let selected = graph_entity(&conn, "ai_session", "chosen:codex:reused_%");
        graph_entity(&conn, "ai_session", "other:codex:reused_%");
        let host = graph_entity(&conn, "host", "host-a");
        conn.execute("INSERT INTO graph_relationships(relationship_key,src_entity_id,dst_entity_id,
            relationship_type,reason_code,trust_level,confidence,last_seen_at)
            VALUES('scope-link',?1,?2,'runs_on','log_app_name','inferred',0.5,'2026-09-21T08:00:00Z')", params![selected,host]).unwrap();
    }
    let result = correlate_session_graph_scoped(&pool, "reused_%", &scope(), 10).unwrap();
    assert!(result.used_graph);
    assert_eq!(result.session_entity_keys, ["chosen:codex:reused_%"]);
    assert_eq!(result.logs.len(), 3);
    assert!(result.logs.iter().any(|row| row.message == "host context"));
    assert!(
        result
            .logs
            .iter()
            .all(|row| row.ai_project.as_deref() != Some("other"))
    );
}

#[test]
fn graph_key_shared_by_hosts_falls_back_without_claiming_other_host_evidence() {
    let (pool, _dir) = fixture();
    insert_logs_batch(
        &pool,
        &[
            row("2026-09-21T08:00:00Z", "host-a", "chosen", "codex", "same"),
            row("2026-09-21T08:01:00Z", "host-b", "chosen", "codex", "same"),
        ],
    )
    .unwrap();
    graph_entity(&pool.get().unwrap(), "ai_session", "chosen:codex:same");
    let result = correlate_session_graph_scoped(&pool, "same", &scope(), 10).unwrap();
    assert!(!result.used_graph);
    assert!(result.session_entity_keys.is_empty());
    assert!(result.discovered_hosts.is_empty());
    assert_eq!(result.logs.len(), 1);
    assert_eq!(result.logs[0].hostname, "host-a");
}

#[test]
fn severity_filter_precedes_the_correlation_row_limit() {
    let (pool, _dir) = fixture();
    let mut error = row("2026-09-21T08:00:00Z", "host-a", "chosen", "codex", "same");
    error.severity = "err".into();
    insert_logs_batch(
        &pool,
        &[
            error,
            row("2026-09-21T08:01:00Z", "host-a", "chosen", "codex", "same"),
            row("2026-09-21T08:02:00Z", "host-a", "chosen", "codex", "same"),
        ],
    )
    .unwrap();
    let mut scope = scope();
    scope.severity_in = vec!["err".into()];
    let result = correlate_session_graph_scoped(&pool, "same", &scope, 1).unwrap();
    assert_eq!(result.logs.len(), 1);
    assert_eq!(result.logs[0].severity, "err");
}

#[test]
fn oversized_source_fields_are_bounded_with_explicit_partial_evidence() {
    let (pool, _dir) = fixture();
    let mut source = row("2026-09-21T08:00:00Z", "host-a", "chosen", "codex", "large");
    source.message = "😀".repeat(10_000);
    source.metadata_json =
        Some(serde_json::json!({"source_kind":"fixture", "large":"x".repeat(10_000)}).to_string());
    insert_logs_batch(&pool, &[source]).unwrap();
    let result = correlate_session_graph_scoped(&pool, "large", &scope(), 10).unwrap();
    assert!(result.source_fields_truncated);
    assert_eq!(result.logs[0].message.chars().count(), 2048);
    assert!(result.logs[0].metadata_json.is_none());
}
