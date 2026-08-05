use super::*;
use crate::config::StorageConfig;
use crate::db::mcp_incidents::AiMcpIncidentParams;
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
fn investigate_ai_mcp_incidents_bundle_has_bounded_collections_and_truncation_flags() {
    let (pool, _dir) = test_pool();

    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "called mcp__labby__search",
    );
    let correction_log = make_ai_entry(
        "2026-01-01T00:01:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "no, that's the wrong tool",
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
    insert_mcp_event(
        &pool,
        log_ids[0],
        "codex",
        "/tmp/project-d",
        "sess-d",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_d",
        "mcp__labby__search",
        Some("labby"),
        Some("search"),
        Some(false),
    );

    let result = investigate_ai_mcp_incidents(
        &pool,
        &AiMcpInvestigateParams {
            mcp_server: Some("labby".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.evidence.len(), 1);
    let bundle = &result.evidence[0];
    assert_eq!(bundle.incident.mcp_server, "labby");
    assert_eq!(bundle.mcp_events.len(), 1);
    assert!(!bundle.mcp_events_truncated);
    assert!(
        bundle
            .signal_anchors
            .iter()
            .any(|e| e.message.contains("wrong tool"))
    );
}

#[test]
fn investigate_ai_mcp_incidents_filters_by_incident_id() {
    let (pool, _dir) = test_pool();
    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "claude",
        "/tmp/project-f",
        "sess-f",
        "called mcp__gh__search",
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
        "/tmp/project-f",
        "sess-f",
        "devhost",
        "2026-01-01T00:00:00Z",
        "call_f",
        "mcp__gh__search",
        Some("gh"),
        Some("search"),
        Some(false),
    );

    let all = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("gh".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(all.incidents.len(), 1);
    let incident_id = all.incidents[0].incident_id.clone();

    let result = investigate_ai_mcp_incidents(
        &pool,
        &AiMcpInvestigateParams {
            incident_id: Some(incident_id.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].incident.incident_id, incident_id);

    let none_result = investigate_ai_mcp_incidents(
        &pool,
        &AiMcpInvestigateParams {
            incident_id: Some("mcp-inc-doesnotexist".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(none_result.evidence.is_empty());
}

/// Regression test for a bug where an exact `incident_id` lookup routed
/// through `search_ai_mcp_incidents` with `limit: Some(100)` and then
/// filtered client-side for the matching id — if the target incident ranked
/// below the top 100 by priority score, investigation silently returned
/// empty evidence for an incident that actually existed. This constructs
/// 100 higher-scored decoy incidents plus one lower-scored target so the
/// target provably ranks outside any top-100 window, then asserts the exact
/// lookup still finds it.
#[test]
fn investigate_ai_mcp_incidents_exact_incident_id_beyond_top_100_candidates() {
    let (pool, _dir) = test_pool();

    fn log_ids_for_session(pool: &DbPool, session_id: &str) -> Vec<i64> {
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM logs WHERE ai_session_id = ?1 ORDER BY timestamp ASC, id ASC")
            .unwrap();
        stmt.query_map([session_id], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    // 100 decoy groups, each scored higher than baseline via a
    // user_correction_after_tool_call anchor, so every decoy outranks the
    // target.
    for i in 0..100 {
        let session_id = format!("sess-decoy-{i:03}");
        let call_log = make_ai_entry(
            "2026-01-01T00:00:00Z",
            "host-a",
            "codex",
            "/tmp/project-g",
            &session_id,
            "called mcp__labby__search",
        );
        let correction_log = make_ai_entry(
            "2026-01-01T00:00:30Z",
            "host-a",
            "codex",
            "/tmp/project-g",
            &session_id,
            "no, that's the wrong tool",
        );
        insert_logs_batch(&pool, &[call_log, correction_log]).unwrap();
        let ids = log_ids_for_session(&pool, &session_id);
        insert_mcp_event(
            &pool,
            ids[0],
            "codex",
            "/tmp/project-g",
            &session_id,
            "host-a",
            "2026-01-01T00:00:00Z",
            &format!("call-decoy-{i:03}"),
            "mcp__labby__search",
            Some("labby"),
            Some("search"),
            Some(false),
        );
    }

    // Target group: baseline score only (no anchor signal), guaranteeing it
    // ranks last among the 101 total matching incidents.
    let target_session_id = "sess-target".to_string();
    let target_call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "host-a",
        "codex",
        "/tmp/project-g",
        &target_session_id,
        "called mcp__labby__search",
    );
    insert_logs_batch(&pool, &[target_call_log]).unwrap();
    let target_log_ids = log_ids_for_session(&pool, &target_session_id);
    insert_mcp_event(
        &pool,
        target_log_ids[0],
        "codex",
        "/tmp/project-g",
        &target_session_id,
        "host-a",
        "2026-01-01T00:00:00Z",
        "call-target",
        "mcp__labby__search",
        Some("labby"),
        Some("search"),
        Some(false),
    );

    let target_lookup = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("labby".into()),
            ai_session_id: Some(target_session_id.clone()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(target_lookup.incidents.len(), 1);
    let target_id = target_lookup.incidents[0].incident_id.clone();

    let top100 = search_ai_mcp_incidents(
        &pool,
        &AiMcpIncidentParams {
            mcp_server: Some("labby".into()),
            limit: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(top100.total_incidents, 101, "100 decoys + 1 target");
    assert_eq!(top100.incidents.len(), 100);
    assert!(
        !top100
            .incidents
            .iter()
            .any(|inc| inc.incident_id == target_id),
        "test setup invariant: target must rank outside the top 100"
    );

    // The regression check: an exact incident_id lookup must still find the
    // target even though it ranks outside the top-100 candidate window.
    let exact = investigate_ai_mcp_incidents(
        &pool,
        &AiMcpInvestigateParams {
            incident_id: Some(target_id.clone()),
            mcp_server: Some("labby".into()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        exact.evidence.len(),
        1,
        "exact incident_id lookup must find an incident ranked outside the top 100"
    );
    assert_eq!(exact.evidence[0].incident.incident_id, target_id);
}

/// Regression test for a bug where the `nearby_logs` query only filtered by
/// timestamp range, with no hostname scope, so an incident on one host could
/// pull in unrelated log rows from a different host in the same time window.
#[test]
fn investigate_ai_mcp_incidents_nearby_logs_scoped_to_incident_hostname() {
    let (pool, _dir) = test_pool();

    let call_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "host-a",
        "codex",
        "/tmp/project-h",
        "sess-h",
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
        "codex",
        "/tmp/project-h",
        "sess-h",
        "host-a",
        "2026-01-01T00:00:00Z",
        "call-h",
        "mcp__labby__search",
        Some("labby"),
        Some("search"),
        Some(false),
    );

    // Unrelated non-AI log on a DIFFERENT host, within the correlation window.
    let other_host_log = LogBatchEntry {
        timestamp: "2026-01-01T00:01:00Z".to_string(),
        hostname: "host-b".to_string(),
        facility: Some("local0".to_string()),
        severity: "error".to_string(),
        app_name: Some("nginx".to_string()),
        process_id: None,
        message: "connection refused".to_string(),
        raw: "connection refused".to_string(),
        source_ip: "10.0.0.5:514".to_string(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    };
    insert_logs_batch(&pool, &[other_host_log]).unwrap();

    let result = investigate_ai_mcp_incidents(
        &pool,
        &AiMcpInvestigateParams {
            mcp_server: Some("labby".into()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.evidence.len(), 1);
    let bundle = &result.evidence[0];
    assert!(
        bundle.nearby_logs.iter().all(|e| e.hostname == "host-a"),
        "nearby_logs leaked a cross-host row: {:?}",
        bundle.nearby_logs
    );
    assert!(
        !bundle
            .nearby_logs
            .iter()
            .any(|e| e.message.contains("connection refused")),
        "cross-host log should not appear in nearby_logs"
    );
}
