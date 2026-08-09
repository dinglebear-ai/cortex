use super::*;
use crate::config::StorageConfig;
use crate::db::pool::init_pool;
use crate::db::skill_incidents::{AiSkillIncidentParams, search_ai_skill_incidents};
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
fn insert_skill_event(
    pool: &DbPool,
    log_id: i64,
    ai_tool: &str,
    ai_project: &str,
    ai_session_id: &str,
    hostname: &str,
    timestamp: &str,
    skill_name: &str,
    skill_plugin: Option<&str>,
) {
    // Note (PR 2 eng review sync): `skill_path`/`metadata_json` were dropped
    // from `ai_skill_events` before PR 2 shipped — neither extractor ever set
    // them. This INSERT reflects the shipped column set.
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO ai_skill_events
            (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
             skill_name, skill_plugin, event_kind, evidence_kind, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'skill_invoked', 'transcript', ?6)",
        rusqlite::params![
            log_id,
            ai_tool,
            ai_project,
            ai_session_id,
            hostname,
            timestamp,
            skill_name,
            skill_plugin,
        ],
    )
    .unwrap();
}

#[test]
fn investigate_ai_skill_incidents_bundle_has_bounded_collections_and_truncation_flags() {
    let (pool, _dir) = test_pool();

    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "loaded skill lavra:lavra-plan",
    );
    let before_log = make_ai_entry(
        "2026-01-01T00:00:00.000Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "user asked to plan the feature",
    );
    let correction_log = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "that's not what I asked, wrong file",
    );
    let failure_log = make_ai_entry(
        "2026-01-01T00:03:00Z",
        "devhost",
        "codex",
        "/tmp/project-d",
        "sess-d",
        "command exited with exit code 1",
    );
    insert_logs_batch(&pool, &[before_log, skill_log, correction_log, failure_log]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM logs ORDER BY timestamp ASC, id ASC")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    // log_ids: [before, skill, correction, failure] in timestamp order.
    insert_skill_event(
        &pool,
        log_ids[1],
        "codex",
        "/tmp/project-d",
        "sess-d",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    let result = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(3),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.evidence.len(), 1);
    let bundle = &result.evidence[0];
    assert_eq!(bundle.incident.skill_name, "lavra:lavra-plan");
    assert!(!bundle.skill_events.is_empty());
    assert!(!bundle.skill_events_truncated);
    assert!(!bundle.signal_anchors.is_empty());
    assert!(!bundle.signal_anchors_truncated);
    // transcript_before should include the pre-skill "user asked to plan" row.
    assert!(
        bundle
            .transcript_before
            .iter()
            .any(|e| e.message.contains("user asked to plan"))
    );
    assert!(!bundle.transcript_before_truncated);
    assert!(!bundle.transcript_after_truncated);
    // The correction log should land in nearby_user_corrections; the failure
    // log should land in nearby_tool_failures.
    assert!(
        bundle
            .nearby_user_corrections
            .iter()
            .any(|e| e.message.contains("wrong file"))
    );
    assert!(
        bundle
            .nearby_tool_failures
            .iter()
            .any(|e| e.message.contains("exit code 1"))
    );
    assert!(!bundle.nearby_logs_truncated);
    assert!(!bundle.nearby_errors_truncated);
}

#[test]
fn investigate_ai_skill_incidents_exact_incident_id_can_target_outside_top_page() {
    let (pool, _dir) = test_pool();
    let mut entries = Vec::new();
    for i in 0..12 {
        entries.push(make_ai_entry(
            &format!("2026-01-01T00:{i:02}:00Z"),
            "host-a",
            "codex",
            "/tmp/project-e",
            &format!("sess-e-{i:02}"),
            "loaded skill lavra:lavra-plan",
        ));
    }
    insert_logs_batch(&pool, &entries).unwrap();
    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    for (i, log_id) in log_ids.iter().enumerate() {
        insert_skill_event(
            &pool,
            *log_id,
            "codex",
            "/tmp/project-e",
            &format!("sess-e-{i:02}"),
            "host-a",
            &format!("2026-01-01T00:{i:02}:00Z"),
            "lavra:lavra-plan",
            Some("lavra"),
        );
    }

    let listed = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(listed.incidents.len(), 12);
    let target_id = listed.incidents.last().unwrap().incident_id.clone();

    let top_page = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !top_page
            .evidence
            .iter()
            .any(|b| b.incident.incident_id == target_id)
    );

    let exact = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            incident_id: Some(target_id.clone()),
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(exact.evidence.len(), 1);
    assert_eq!(exact.evidence[0].incident.incident_id, target_id);
}

/// Regression test for a bug where an exact `incident_id` lookup routed
/// through `search_ai_skill_incidents` with `limit: Some(100)` and then
/// filtered client-side for the matching id — if the target incident
/// ranked below the top 100 by priority score, investigation silently
/// returned empty evidence for an incident that actually existed. This
/// constructs 100 higher-scored decoy incidents plus one lower-scored
/// target so the target provably ranks outside any top-100 window, then
/// asserts the exact lookup still finds it.
#[test]
fn investigate_ai_skill_incidents_exact_incident_id_beyond_top_100_candidates() {
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
    // user_correction_after_skill anchor, so every decoy outranks the target.
    for i in 0..100 {
        let session_id = format!("sess-decoy-{i:03}");
        let skill_log = make_ai_entry(
            "2026-01-01T00:00:00Z",
            "host-a",
            "codex",
            "/tmp/project-g",
            &session_id,
            "loaded skill lavra:lavra-plan",
        );
        let correction_log = make_ai_entry(
            "2026-01-01T00:00:30Z",
            "host-a",
            "codex",
            "/tmp/project-g",
            &session_id,
            "that's not what I asked, wrong file",
        );
        insert_logs_batch(&pool, &[skill_log, correction_log]).unwrap();
        let ids = log_ids_for_session(&pool, &session_id);
        insert_skill_event(
            &pool,
            ids[0],
            "codex",
            "/tmp/project-g",
            &session_id,
            "host-a",
            "2026-01-01T00:00:00Z",
            "lavra:lavra-plan",
            Some("lavra"),
        );
    }

    // Target group: baseline score only (no anchor signal), guaranteeing it
    // ranks last among the 101 total matching incidents.
    let target_session_id = "sess-target".to_string();
    let target_skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "host-a",
        "codex",
        "/tmp/project-g",
        &target_session_id,
        "loaded skill lavra:lavra-plan",
    );
    insert_logs_batch(&pool, &[target_skill_log]).unwrap();
    let target_log_ids = log_ids_for_session(&pool, &target_session_id);
    insert_skill_event(
        &pool,
        target_log_ids[0],
        "codex",
        "/tmp/project-g",
        &target_session_id,
        "host-a",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    let target_lookup = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            ai_session_id: Some(target_session_id.clone()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(target_lookup.incidents.len(), 1);
    let target_id = target_lookup.incidents[0].incident_id.clone();

    let top100 = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
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
    let exact = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            incident_id: Some(target_id.clone()),
            skill: Some("lavra:lavra-plan".into()),
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
fn investigate_ai_skill_incidents_nearby_logs_scoped_to_incident_hostname() {
    let (pool, _dir) = test_pool();

    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "host-a",
        "codex",
        "/tmp/project-h",
        "sess-h",
        "loaded skill lavra:lavra-plan",
    );
    insert_logs_batch(&pool, &[skill_log]).unwrap();
    let log_id: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT id FROM logs LIMIT 1", [], |row| row.get(0))
            .unwrap()
    };
    insert_skill_event(
        &pool,
        log_id,
        "codex",
        "/tmp/project-h",
        "sess-h",
        "host-a",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
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

    let result = investigate_ai_skill_incidents(
        &pool,
        &AiSkillInvestigateParams {
            skill: Some("lavra:lavra-plan".into()),
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
