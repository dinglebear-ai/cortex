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
    evidence_kind: &str,
) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO ai_hook_events
            (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
             hook_event, hook_name, status, evidence_kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            evidence_kind,
        ],
    )
    .unwrap();
}

#[test]
fn investigate_returns_bounded_evidence_bundle_with_findings_ready_data() {
    let (pool, _dir) = test_pool();
    let entries = vec![
        make_ai_entry(
            "2026-01-01T00:00:00.000Z",
            "devhost",
            "claude",
            "/home/jmagar/workspace/cortex",
            "sess-1",
            "starting work",
        ),
        make_ai_entry(
            "2026-01-01T00:00:10.000Z",
            "devhost",
            "claude",
            "/home/jmagar/workspace/cortex",
            "sess-1",
            "after hook context, exit code nonzero",
        ),
    ];
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
        "sess-1",
        "devhost",
        "2026-01-01T00:00:05.000Z",
        "PostToolUse",
        "format-on-save",
        "failed",
        "runtime_transcript",
    );

    let result = investigate_ai_hook_incidents(
        &pool,
        &AiHookInvestigateParams {
            hook_name: Some("format-on-save".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.evidence.len(), 1);
    let bundle = &result.evidence[0];
    assert_eq!(bundle.incident.hook_name.as_deref(), Some("format-on-save"));
    assert_eq!(bundle.hook_events.len(), 1);
    assert!(!bundle.hook_events_truncated);
    assert!(!bundle.transcript_before.is_empty() || !bundle.transcript_after.is_empty());
}

#[test]
fn investigate_by_incident_id_narrows_to_one() {
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
        "hook-a",
        "success",
        "runtime_transcript",
    );
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/home/jmagar/workspace/cortex",
        "sess-b",
        "devhost",
        "2026-01-01T01:00:00.000Z",
        "PostToolUse",
        "hook-b",
        "failed",
        "runtime_transcript",
    );

    let all = search_ai_hook_incidents(&pool, &AiHookIncidentParams::default()).unwrap();
    assert_eq!(all.incidents.len(), 2);
    let target_id = all.incidents[0].incident_id.clone();

    let result = investigate_ai_hook_incidents(
        &pool,
        &AiHookInvestigateParams {
            incident_id: Some(target_id.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].incident.incident_id, target_id);
}

#[test]
fn investigate_with_no_matching_hook_returns_empty_evidence() {
    let (pool, _dir) = test_pool();
    let result = investigate_ai_hook_incidents(
        &pool,
        &AiHookInvestigateParams {
            hook_name: Some("nonexistent-hook".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.evidence.is_empty());
    assert_eq!(result.total_incidents, 0);
}

/// Regression test for a bug where an exact `incident_id` lookup routed
/// through `search_ai_hook_incidents` with `limit: Some(100)` and then
/// filtered client-side for the matching id — if the target incident ranked
/// below the top 100 by priority score, investigation silently returned
/// empty evidence for an incident that actually existed. This constructs
/// 100 higher-scored decoy incidents (failed hook status) plus one
/// lower-scored target (successful hook status) so the target provably
/// ranks outside any top-100 window, then asserts the exact lookup still
/// finds it.
#[test]
fn investigate_ai_hook_incidents_exact_incident_id_beyond_top_100_candidates() {
    let (pool, _dir) = test_pool();

    for i in 0..100 {
        insert_hook_event_row(
            &pool,
            None,
            "claude",
            "/tmp/project-g",
            &format!("sess-decoy-{i:03}"),
            "host-a",
            "2026-01-01T00:00:00.000Z",
            "PostToolUse",
            "format-on-save",
            "failed",
            "runtime_transcript",
        );
    }

    // Target group: successful status, no failure signal, guaranteeing it
    // ranks last among the 101 total matching incidents.
    let target_session_id = "sess-target";
    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/tmp/project-g",
        target_session_id,
        "host-a",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "format-on-save",
        "success",
        "runtime_transcript",
    );

    let target_lookup = search_ai_hook_incidents(
        &pool,
        &AiHookIncidentParams {
            hook_name: Some("format-on-save".to_string()),
            ai_session_id: Some(target_session_id.to_string()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(target_lookup.incidents.len(), 1);
    let target_id = target_lookup.incidents[0].incident_id.clone();

    let top100 = search_ai_hook_incidents(
        &pool,
        &AiHookIncidentParams {
            hook_name: Some("format-on-save".to_string()),
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
    let exact = investigate_ai_hook_incidents(
        &pool,
        &AiHookInvestigateParams {
            incident_id: Some(target_id.clone()),
            hook_name: Some("format-on-save".to_string()),
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
fn investigate_ai_hook_incidents_nearby_logs_scoped_to_incident_hostname() {
    let (pool, _dir) = test_pool();

    insert_hook_event_row(
        &pool,
        None,
        "claude",
        "/tmp/project-h",
        "sess-h",
        "host-a",
        "2026-01-01T00:00:00.000Z",
        "PostToolUse",
        "format-on-save",
        "success",
        "runtime_transcript",
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

    let result = investigate_ai_hook_incidents(
        &pool,
        &AiHookInvestigateParams {
            hook_name: Some("format-on-save".to_string()),
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
