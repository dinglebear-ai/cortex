use super::*;
use crate::app::CortexService;
use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};
use serial_test::serial;
use std::sync::Arc;

// All tests below share the process-wide `backfill_guard()` singleton
// semaphore, so they must not run concurrently with each other, mirroring
// skill_backfill_tests.rs's rationale.

fn test_service() -> (CortexService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let storage = StorageConfig::for_test(db_path);
    let pool: Arc<DbPool> = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(pool, storage), dir)
}

fn insert_claude_log_row(pool: &DbPool, raw_json: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip, ai_tool, ai_project, ai_session_id)
         VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', '[tool_use test]', ?1, 'transcript://claude_project', 'claude', 'cortex', 'sess-1')",
        rusqlite::params![raw_json],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn claude_tool_use_json(call_id: &str, tool_name: &str) -> String {
    serde_json::json!({
        "message": {
            "content": [
                {"type": "tool_use", "id": call_id, "name": tool_name, "input": {}}
            ]
        }
    })
    .to_string()
}

#[tokio::test]
#[serial(mcp_backfill_guard)]
async fn dry_run_reports_counts_without_inserting() {
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    insert_claude_log_row(
        &pool,
        &claude_tool_use_json("toolu_1", "mcp__labby__search"),
    );

    let result = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: true,
        })
        .await
        .unwrap();

    assert_eq!(result.scanned, 1);
    assert_eq!(result.inserted, 0);
    assert!(result.dry_run);

    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ai_mcp_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
#[serial(mcp_backfill_guard)]
async fn real_run_inserts_events_and_is_idempotent() {
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    insert_claude_log_row(
        &pool,
        &claude_tool_use_json("toolu_2", "mcp__labby__search"),
    );

    let first = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(first.scanned, 1);
    assert_eq!(first.inserted, 1);
    assert_eq!(first.skipped_duplicates, 0);

    let second = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(second.scanned, 1);
    assert_eq!(second.inserted, 0);
    assert_eq!(second.skipped_duplicates, 1);
}

#[tokio::test]
#[serial(mcp_backfill_guard)]
async fn limit_is_clamped_to_hard_upper_bound() {
    let (service, _dir) = test_service();
    insert_claude_log_row(
        &service.pool_for_test(),
        &claude_tool_use_json("toolu_3", "Bash"),
    );

    let result = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(10_000_000),
            dry_run: true,
        })
        .await
        .unwrap();
    assert_eq!(result.scanned, 1);
}

#[tokio::test]
#[serial(mcp_backfill_guard)]
async fn concurrent_backfill_calls_return_busy_instead_of_racing() {
    let (service, _dir) = test_service();
    insert_claude_log_row(
        &service.pool_for_test(),
        &claude_tool_use_json("toolu_4", "Bash"),
    );

    let _held = super::backfill_guard()
        .clone()
        .try_acquire_owned()
        .expect("guard should be free at test start");

    let result = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: true,
        })
        .await;

    assert!(
        result.is_err(),
        "second concurrent backfill call must be rejected"
    );
}

#[tokio::test]
#[serial(mcp_backfill_guard)]
async fn malformed_raw_json_counts_as_parse_error_not_panic() {
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    insert_claude_log_row(&pool, "not valid json{{{");

    let result = service
        .backfill_mcp_events(McpBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(result.scanned, 1);
    assert_eq!(result.parse_errors, 1);
    assert_eq!(result.inserted, 0);
}
