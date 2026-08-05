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
fn search_ai_skill_incidents_groups_by_skill_session_window_and_scores() {
    let (pool, _dir) = test_pool();

    // Skill event log row.
    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "loaded skill lavra:lavra-plan",
    );
    // Correction anchor shortly after, same session.
    let correction_log = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "That's not what I asked for, please redo it.",
    );
    insert_logs_batch(&pool, &[skill_log, correction_log]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(log_ids.len(), 2);

    insert_skill_event(
        &pool,
        log_ids[0],
        "codex",
        "/home/jmagar/workspace/cortex",
        "sess-skill-1",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    let result = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.incidents.len(), 1, "expected one grouped incident");
    let incident = &result.incidents[0];
    assert_eq!(incident.skill_name, "lavra:lavra-plan");
    assert_eq!(incident.skill_plugin.as_deref(), Some("lavra"));
    assert_eq!(incident.tool, "codex");
    assert_eq!(incident.project, "/home/jmagar/workspace/cortex");
    assert_eq!(incident.session_id, "sess-skill-1");
    assert_eq!(incident.hostname, "devhost");
    assert_eq!(incident.skill_event_count, 1);
    assert_eq!(incident.signal_counts.user_correction_after_skill, 1);
    assert!(
        incident
            .signals_present
            .contains(&"user_correction_after_skill".to_string())
    );
    // score = skill_event_count*2 + user_correction_count*15 + signal_variety*5
    //       = 1*2 + 1*15 + 1*5 = 22 -> "medium" (>=15, <35)
    assert!((incident.priority_score - 22.0).abs() < f64::EPSILON);
    assert_eq!(incident.priority_label, "medium");
    assert!(!incident.incident_id.is_empty());
    assert!(incident.incident_id.starts_with("skill-inc-"));
}

#[test]
fn search_ai_skill_incidents_sorts_by_score_with_total_cmp() {
    let (pool, _dir) = test_pool();

    // Two independent sessions -> two incidents with different scores.
    // Session A: skill event only, no negative signal (low score).
    let a_skill = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-a",
        "sess-a",
        "loaded skill lavra:lavra-plan",
    );
    // Session B: skill event + correction + tool failure (higher score).
    let b_skill = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "loaded skill lavra:lavra-plan",
    );
    let b_correction = make_ai_entry(
        "2026-01-01T00:01:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "you said you would run the tests but you didn't",
    );
    let b_failure = make_ai_entry(
        "2026-01-01T00:02:00Z",
        "devhost",
        "codex",
        "/tmp/project-b",
        "sess-b",
        "command exited with exit code 1",
    );
    insert_logs_batch(&pool, &[a_skill, b_skill, b_correction, b_failure]).unwrap();

    let log_ids: Vec<i64> = {
        let conn = pool.get().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM logs ORDER BY id ASC").unwrap();
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    insert_skill_event(
        &pool,
        log_ids[0],
        "codex",
        "/tmp/project-a",
        "sess-a",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );
    insert_skill_event(
        &pool,
        log_ids[1],
        "codex",
        "/tmp/project-b",
        "sess-b",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    let result = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            limit: Some(10),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.incidents.len(), 2);
    // Highest score first (session B).
    assert_eq!(result.incidents[0].session_id, "sess-b");
    assert_eq!(result.incidents[1].session_id, "sess-a");
    assert!(result.incidents[0].priority_score > result.incidents[1].priority_score);
    // Regression guard: scores must be a total order even in pathological
    // cases (NaN would break partial_cmp/unwrap_or(Equal) but not total_cmp).
    let mut scores = [f64::NAN, 3.0, 1.0, f64::NAN, 2.0];
    scores.sort_by(|a, b| b.total_cmp(a));
    assert_eq!(
        scores.len(),
        5,
        "total_cmp sort must not panic or drop elements on NaN"
    );
}

#[test]
fn search_ai_skill_incidents_min_score_and_signals_filters() {
    let (pool, _dir) = test_pool();
    let skill_log = make_ai_entry(
        "2026-01-01T00:00:00Z",
        "devhost",
        "claude",
        "/tmp/project-c",
        "sess-c",
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
        "claude",
        "/tmp/project-c",
        "sess-c",
        "devhost",
        "2026-01-01T00:00:00Z",
        "lavra:lavra-plan",
        Some("lavra"),
    );

    // min_score above what a bare skill-event-only incident can reach (score=2) excludes it.
    let filtered = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            min_score: Some(10.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered.incidents.is_empty());

    // signals filter for a category with zero hits also excludes it.
    let filtered_by_signal = search_ai_skill_incidents(
        &pool,
        &AiSkillIncidentParams {
            skill: Some("lavra:lavra-plan".into()),
            signals: vec!["tool_failure_after_skill".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(filtered_by_signal.incidents.is_empty());
}
