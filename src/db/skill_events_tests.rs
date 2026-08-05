use super::*;
use crate::config::StorageConfig;
use crate::db::pool::init_pool;
use crate::scanner::skill_events::{ExtractedSkillEvent, SkillEventKind, SkillEvidenceKind};

fn test_pool() -> (crate::db::DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    (pool, dir)
}

fn insert_log_row(pool: &crate::db::DbPool, hostname: &str, timestamp: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip)
         VALUES (?1, ?2, 'info', 'msg', 'raw', 'transcript://claude_project')",
        rusqlite::params![timestamp, hostname],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn sample_event(skill_name: &str) -> ExtractedSkillEvent {
    ExtractedSkillEvent {
        skill_name: skill_name.to_string(),
        skill_plugin: Some("cortex".to_string()),
        event_kind: SkillEventKind::ClaudeAttribution,
        evidence_kind: SkillEvidenceKind::StructuredJsonField,
    }
}

#[test]
fn insert_and_list_round_trips() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = SkillEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: Some("cortex".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: sample_event("cortex-troubleshoot"),
    };
    let inserted = insert_skill_events(&pool, &[insert]).unwrap();
    assert_eq!(inserted, 1);

    let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].skill_name, "cortex-troubleshoot");
    assert_eq!(result.events[0].skill_plugin.as_deref(), Some("cortex"));
    assert_eq!(result.events[0].event_kind, "claude_attribution");
    assert_eq!(result.events[0].evidence_kind, "structured_json_field");
    assert_eq!(result.events[0].log_id, log_id);
}

#[test]
fn insert_or_ignore_is_idempotent_on_duplicate() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = SkillEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: None,
        ai_session_id: None,
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: sample_event("cortex-troubleshoot"),
    };
    assert_eq!(
        insert_skill_events(&pool, std::slice::from_ref(&insert)).unwrap(),
        1
    );
    assert_eq!(insert_skill_events(&pool, &[insert]).unwrap(), 0);

    let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
}

#[test]
fn insert_succeeds_without_project_or_session_id() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = SkillEventInsert {
        log_id,
        ai_tool: "codex".to_string(),
        ai_project: None,
        ai_session_id: None,
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: ExtractedSkillEvent {
            skill_name: "rustarr".to_string(),
            skill_plugin: None,
            event_kind: SkillEventKind::CodexSkillBlock,
            evidence_kind: SkillEvidenceKind::TranscriptContent,
        },
    };
    assert_eq!(insert_skill_events(&pool, &[insert]).unwrap(), 1);
    let result = list_skill_events(&pool, &AiSkillEventParams::default()).unwrap();
    assert_eq!(result.events[0].ai_project, None);
    assert_eq!(result.events[0].ai_session_id, None);
}

#[test]
fn list_filters_by_skill_project_and_tool() {
    let (pool, _dir) = test_pool();
    let log_id_a = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let log_id_b = insert_log_row(&pool, "nashost", "2026-06-01T01:00:00.000Z");
    insert_skill_events(
        &pool,
        &[
            SkillEventInsert {
                log_id: log_id_a,
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-a".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                event: sample_event("cortex-troubleshoot"),
            },
            SkillEventInsert {
                log_id: log_id_b,
                ai_tool: "codex".to_string(),
                ai_project: Some("axon".to_string()),
                ai_session_id: Some("sess-b".to_string()),
                hostname: "nashost".to_string(),
                timestamp: "2026-06-01T01:00:00.000Z".to_string(),
                event: sample_event("axon-deploy"),
            },
        ],
    )
    .unwrap();

    let result = list_skill_events(
        &pool,
        &AiSkillEventParams {
            project: Some("cortex".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].skill_name, "cortex-troubleshoot");

    let result = list_skill_events(
        &pool,
        &AiSkillEventParams {
            tool: Some("codex".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].ai_tool, "codex");
}
