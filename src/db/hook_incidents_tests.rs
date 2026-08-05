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
fn insert_hook_event_row(
    pool: &DbPool,
    log_id: Option<i64>,
    ai_tool: &str,
    ai_project: &str,
    ai_session_id: &str,
    hostname: &str,
    timestamp: &str,
    hook_event: &str,
    hook_name: &str,
    status: &str,
    duration_ms: Option<i64>,
    evidence_kind: &str,
) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO ai_hook_events
            (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
             hook_event, hook_name, status, duration_ms, evidence_kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            log_id,
            ai_tool,
            ai_project,
            ai_session_id,
            hostname,
            timestamp,
            hook_event,
            hook_name,
            status,
            duration_ms,
            evidence_kind,
        ],
    )
    .unwrap();
}

#[test]
fn search_ai_hook_incidents_groups_and_scores_failures() {
    let (pool, _dir) = test_pool();

    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-hook-1",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "format-on-save",
        "failed",
        None,
        "runtime_transcript",
    );
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-hook-1",
        "devhost",
        "2026-01-01T00:00:05.000Z",
        "PostToolUse",
        "format-on-save",
        "failed",
        None,
        "runtime_transcript",
    );

    let result = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(result.incidents.len(), 1);
    let incident = &result.incidents[0];
    assert_eq!(incident.hook_name.as_deref(), Some("format-on-save"));
    assert_eq!(incident.hook_event_count, 2);
    assert_eq!(incident.signal_counts.hook_failed, 2);
    assert!(
        incident
            .signals_present
            .contains(&"hook_failed".to_string())
    );
    assert!(incident.has_runtime_evidence);
    assert!(incident.priority_score > 0.0);
    assert!(incident.incident_id.starts_with("hook-inc-"));
}

#[test]
fn config_only_incident_has_runtime_evidence_false() {
    let (pool, _dir) = test_pool();
    insert_hook_event_row(
        &pool,
        None,
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-hook-2",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PreToolUse",
        "lint-check",
        "unknown",
        None,
        "config_inventory",
    );

    let result = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(result.incidents.len(), 1);
    assert!(!result.incidents[0].has_runtime_evidence);
}

#[test]
fn timeout_signal_detected_from_high_duration() {
    let (pool, _dir) = test_pool();
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-hook-3",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "slow-hook",
        "success",
        Some(45_000),
        "runtime_transcript",
    );

    let result = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(result.incidents.len(), 1);
    assert_eq!(result.incidents[0].signal_counts.hook_timed_out, 1);
    assert!(
        result.incidents[0]
            .signals_present
            .contains(&"hook_timed_out".to_string())
    );
}

#[test]
fn user_correction_after_hook_detected_from_nearby_transcript() {
    let (pool, _dir) = test_pool();
    let entries = vec![make_ai_entry(
        "2026-01-01T00:00:05.000Z",
        "devhost",
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-hook-4",
        "That's not what I asked for, you shouldn't have run that hook",
    )];
    insert_logs_batch(&pool, &entries).unwrap();
    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };

    insert_hook_event_row(
        &pool,
        Some(log_ids[0]),
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-hook-4",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "auto-format",
        "success",
        Some(100),
        "runtime_transcript",
    );

    let result = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(result.incidents.len(), 1);
    let incident = &result.incidents[0];
    assert_eq!(incident.signal_counts.user_correction_after_hook, 1);
    assert_eq!(incident.anchor_log_ids, vec![log_ids[0]]);
    assert!(
        incident
            .signals_present
            .contains(&"user_correction_after_hook".to_string())
    );
}

#[test]
fn filters_by_min_score_and_signals() {
    let (pool, _dir) = test_pool();
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-a",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "quiet-hook",
        "success",
        Some(50),
        "runtime_transcript",
    );
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-b",
        "devhost",
        "2026-01-01T00:05:00.000Z",
        "PostToolUse",
        "loud-hook",
        "failed",
        None,
        "runtime_transcript",
    );

    let result = search_ai_hook_incidents(
        &pool,
        &AiHookIncidentParams {
            min_score: Some(10.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.incidents.iter().all(|i| i.priority_score >= 10.0));

    let result = search_ai_hook_incidents(
        &pool,
        &AiHookIncidentParams {
            signals: vec!["hook_failed".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.incidents.len(), 1);
    assert_eq!(result.incidents[0].hook_name.as_deref(), Some("loud-hook"));
}

#[test]
fn sorted_by_priority_score_desc_using_total_cmp() {
    let (pool, _dir) = test_pool();
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-low",
        "devhost",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "low-signal",
        "success",
        Some(10),
        "runtime_transcript",
    );
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-high",
        "devhost",
        "2026-01-01T01:00:00.000Z",
        "PostToolUse",
        "high-signal",
        "failed",
        None,
        "runtime_transcript",
    );

    let result = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(result.incidents.len(), 2);
    assert!(result.incidents[0].priority_score >= result.incidents[1].priority_score);
    assert_eq!(
        result.incidents[0].hook_name.as_deref(),
        Some("high-signal")
    );
}
