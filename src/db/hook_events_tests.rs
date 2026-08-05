use super::*;
use crate::config::StorageConfig;
use crate::db::pool::init_pool;
use crate::scanner::hook_events::{ExtractedHookEvent, HookEvidenceKind, HookStatus};

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

fn sample_event(hook_name: &str) -> ExtractedHookEvent {
    ExtractedHookEvent {
        hook_event: "PostToolUse".to_string(),
        hook_name: Some(hook_name.to_string()),
        hook_source: None,
        hook_command: Some("cargo fmt".to_string()),
        status: HookStatus::Success,
        exit_code: Some(0),
        duration_ms: Some(120),
        stdout_preview: Some("ok".to_string()),
        stderr_preview: None,
        persisted_output_path: None,
        trusted_hash: None,
        evidence_kind: HookEvidenceKind::RuntimeTranscript,
        metadata_json: None,
    }
}

#[test]
fn insert_and_list_round_trips() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = HookEventInsert {
        log_id: Some(log_id),
        ai_tool: "claude".to_string(),
        ai_project: Some("cortex".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: sample_event("format-on-save"),
    };
    let inserted = insert_hook_events(&pool, &[insert]).unwrap();
    assert_eq!(inserted, 1);

    let result = list_hook_events(&pool, &AiHookEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(
        result.events[0].hook_name.as_deref(),
        Some("format-on-save")
    );
    assert_eq!(result.events[0].hook_event, "PostToolUse");
    assert_eq!(result.events[0].status, "success");
    assert_eq!(result.events[0].evidence_kind, "runtime_transcript");
    assert_eq!(result.events[0].log_id, Some(log_id));
}

#[test]
fn insert_or_ignore_is_idempotent_on_duplicate() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = HookEventInsert {
        log_id: Some(log_id),
        ai_tool: "claude".to_string(),
        ai_project: None,
        ai_session_id: Some("sess-1".to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: sample_event("format-on-save"),
    };
    assert_eq!(
        insert_hook_events(&pool, std::slice::from_ref(&insert)).unwrap(),
        1
    );
    assert_eq!(insert_hook_events(&pool, &[insert]).unwrap(), 0);

    let result = list_hook_events(&pool, &AiHookEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
}

#[test]
fn insert_succeeds_without_log_id_for_config_inventory_rows() {
    let (pool, _dir) = test_pool();
    let insert = HookEventInsert {
        log_id: None,
        ai_tool: "codex".to_string(),
        ai_project: None,
        ai_session_id: None,
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: ExtractedHookEvent {
            hook_event: "PreToolUse".to_string(),
            hook_name: Some("lint-check".to_string()),
            hook_source: Some("~/.codex/hooks.json".to_string()),
            hook_command: None,
            status: HookStatus::Unknown,
            exit_code: None,
            duration_ms: None,
            stdout_preview: None,
            stderr_preview: None,
            persisted_output_path: None,
            trusted_hash: Some("abc123".to_string()),
            evidence_kind: HookEvidenceKind::ConfigInventory,
            metadata_json: None,
        },
    };
    assert_eq!(insert_hook_events(&pool, &[insert]).unwrap(), 1);
    let result = list_hook_events(&pool, &AiHookEventParams::default()).unwrap();
    assert_eq!(result.events[0].log_id, None);
    assert_eq!(result.events[0].evidence_kind, "config_inventory");
    assert_eq!(result.events[0].trusted_hash.as_deref(), Some("abc123"));
}

#[test]
fn list_filters_by_hook_name_project_and_tool() {
    let (pool, _dir) = test_pool();
    let log_id_a = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let log_id_b = insert_log_row(&pool, "nashost", "2026-06-01T01:00:00.000Z");
    insert_hook_events(
        &pool,
        &[
            HookEventInsert {
                log_id: Some(log_id_a),
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-a".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                event: sample_event("format-on-save"),
            },
            HookEventInsert {
                log_id: Some(log_id_b),
                ai_tool: "codex".to_string(),
                ai_project: Some("axon".to_string()),
                ai_session_id: Some("sess-b".to_string()),
                hostname: "nashost".to_string(),
                timestamp: "2026-06-01T01:00:00.000Z".to_string(),
                event: sample_event("lint-check"),
            },
        ],
    )
    .unwrap();

    let result = list_hook_events(
        &pool,
        &AiHookEventParams {
            project: Some("cortex".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(
        result.events[0].hook_name.as_deref(),
        Some("format-on-save")
    );

    let result = list_hook_events(
        &pool,
        &AiHookEventParams {
            tool: Some("codex".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].ai_tool, "codex");
}

#[test]
fn list_filters_by_status_and_evidence_kind() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let mut failed = sample_event("lint-check");
    failed.status = HookStatus::Failed;
    failed.hook_event = "PreToolUse".to_string();
    insert_hook_events(
        &pool,
        &[
            HookEventInsert {
                log_id: Some(log_id),
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-a".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                event: sample_event("format-on-save"),
            },
            HookEventInsert {
                log_id: Some(log_id),
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-a".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:01.000Z".to_string(),
                event: failed,
            },
        ],
    )
    .unwrap();

    let result = list_hook_events(
        &pool,
        &AiHookEventParams {
            status: Some("failed".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].status, "failed");

    let result = list_hook_events(
        &pool,
        &AiHookEventParams {
            evidence_kind: Some("runtime_transcript".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 2);
}
