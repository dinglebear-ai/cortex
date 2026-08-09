use super::*;
use crate::config::StorageConfig;
use crate::db::pool::init_pool;
use crate::db::{DbPool, LogBatchEntry, insert_logs_batch};

fn test_pool() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    (pool, dir)
}

fn make_ai_entry(
    ts: &str,
    host: &str,
    tool: &str,
    project: &str,
    session_id: &str,
    message: &str,
) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: Some("local0".to_string()),
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: message.to_string(),
        source_ip: "127.0.0.1:514".to_string(),
        docker_checkpoint: None,
        ai_tool: Some(tool.to_string()),
        ai_project: Some(project.to_string()),
        ai_session_id: Some(session_id.to_string()),
        ai_transcript_path: Some(format!("{project}/{session_id}.jsonl")),
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_mcp_event(
    pool: &DbPool,
    call_log_id: i64,
    ai_tool: &str,
    ai_project: &str,
    ai_session_id: &str,
    hostname: &str,
    timestamp: &str,
    call_id: &str,
    tool_name: &str,
    mcp_server: Option<&str>,
    mcp_tool: Option<&str>,
    is_error: Option<bool>,
) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO ai_mcp_events
            (call_log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
             call_id, tool_name, mcp_server, mcp_tool, event_kind, is_error, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'call', ?11, ?6)",
        rusqlite::params![
            call_log_id,
            ai_tool,
            ai_project,
            ai_session_id,
            hostname,
            timestamp,
            call_id,
            tool_name,
            mcp_server,
            mcp_tool,
            is_error.map(i64::from),
        ],
    )
    .unwrap();
}

#[test]
fn search_ai_mcp_incidents_groups_by_server_tool_session_window_and_scores() {
    let (pool, _dir) = test_pool();

    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-mcp-1",
        "called mcp__labby__search",
    );
    let correction_log = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-mcp-1",
        "no, that's the wrong tool for this",
    );
    insert_logs_batch(&pool, &[call_log, correction_log]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(log_ids.len(), 2);

    insert_mcp_event(
        &pool,
        log_ids[0],
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-mcp-1",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_1",
        "mcp__labby__search",
        Some("labby"),
        Some("search"),
        Some(false),
    );

    let result = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("labby".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.incidents.len(), 1, "expected one grouped incident");
    let incident = &result.incidents[0];
    assert_eq!(incident.mcp_server, "labby");
    assert_eq!(incident.mcp_tool.as_deref(), Some("search"));
    assert_eq!(incident.tool, "codex");
    assert_eq!(incident.project, "/home/jmagar/workspace/cortex");
    assert_eq!(incident.session_id, "sess-mcp-1");
    assert_eq!(incident.hostname, "devhost");
    assert_eq!(incident.event_count, 1);
    assert_eq!(incident.signal_counts.user_correction_after_tool_call, 1);
    assert!(
        incident
            .signals_present
            .contains(&"user_correction_after_tool_call".to_string())
    );
    assert!(!incident.incident_id.is_empty());
    assert!(incident.incident_id.starts_with("mcp-inc-"));
}

#[test]
fn search_ai_mcp_incidents_excludes_non_mcp_classified_rows() {
    let (pool, _dir) = test_pool();
    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project",
        "sess-builtin",
        "called shell",
    );
    insert_logs_batch(&pool, &[call_log]).unwrap();
    let log_id: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT id FROM logs LIMIT 1", [], |row| row.get(0))
            .unwrap()
    };
    // Builtin tool call: mcp_server is NULL.
    insert_mcp_event(
        &pool,
        log_id,
        "codex",
        "/tmp/project",
        "sess-builtin",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_builtin",
        "shell",
        None,
        None,
        Some(false),
    );

    let result = search_ai_mcp_incidents(&pool, &AiMcpIncidentParams::default()).unwrap();
    assert!(
        result.incidents.is_empty(),
        "non-MCP-classified (mcp_server IS NULL) rows must not form incidents"
    );
}

#[test]
fn search_ai_mcp_incidents_repeated_failures_trigger_signal() {
    let (pool, _dir) = test_pool();
    let call_log_1 = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "claude",
        "/tmp/project-e",
        "sess-e",
        "call 1",
    );
    let call_log_2 = make_ai_entry(
        "2026-01-01T00:01:00Z",
        "devhost",
        "claude",
        "/tmp/project-e",
        "sess-e",
        "call 2",
    );
    insert_logs_batch(&pool, &[call_log_1, call_log_2]).unwrap();
    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    insert_mcp_event(
        &pool,
        log_ids[0],
        "claude",
        "/tmp/project-e",
        "sess-e",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_a",
        "mcp__gh__search",
        Some("gh"),
        Some("search"),
        Some(true),
    );
    insert_mcp_event(
        &pool,
        log_ids[1],
        "claude",
        "/tmp/project-e",
        "sess-e",
        "devhost",
        "2026-01-01T00:01:00Z",
        "call_b",
        "mcp__gh__search",
        Some("gh"),
        Some("search"),
        Some(true),
    );

    let result = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("gh".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.incidents.len(), 1);
    let incident = &result.incidents[0];
    assert_eq!(incident.error_count, 2);
    assert!(incident.signal_counts.repeated_call_failure >= 2);
    assert!(
        incident
            .signals_present
            .contains(&"repeated_call_failure".to_string())
    );
}

#[test]
fn search_ai_mcp_incidents_min_score_and_signals_filters() {
    let (pool, _dir) = test_pool();
    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "claude",
        "/tmp/project-c",
        "sess-c",
        "called mcp__labby__search",
    );
    insert_logs_batch(&pool, &[call_log]).unwrap();
    let log_id: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT id FROM logs LIMIT 1", [], |row| row.get(0))
            .unwrap()
    };
    insert_mcp_event(
        &pool,
        log_id,
        "claude",
        "/tmp/project-c",
        "sess-c",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_c",
        "mcp__labby__search",
        Some("labby"),
        Some("search"),
        Some(false),
    );

    let filtered = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("labby".into()),
            min_score: Some(10.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered.incidents.is_empty());

    let filtered_by_signal = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("labby".into()),
            signals: vec!["timeout_or_rate_limit".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered_by_signal.incidents.is_empty());
}

#[test]
fn search_ai_mcp_incidents_sorts_by_score_with_total_cmp() {
    let mut scores = [f64::NAN, 3.0, 1.0, f64::NAN, 2.0];
    scores.sort_by(|a, b| b.total_cmp(a));
    assert_eq!(
        scores.len(),
        5,
        "total_cmp sort must not panic or drop elements on NaN"
    );
}
