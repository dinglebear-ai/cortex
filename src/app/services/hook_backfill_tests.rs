use super::*;
use crate::app::CortexService;
use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};
use serial_test::serial;
use std::sync::Arc;

// All tests below share the process-wide `backfill_guard()` singleton
// semaphore for the hook backfill, so they must not run concurrently with
// each other (mirrors skill_backfill_tests.rs's rationale exactly, but
// scoped to `hook_backfill_guard` — a distinct guard from the skill
// backfill's, so the two families can run in parallel with each other).

fn test_service() -> (CortexService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let storage = StorageConfig::for_test(db_path);
    let pool: Arc<DbPool> = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(pool, storage), dir)
}

fn insert_claude_hook_log_row(pool: &DbPool, message: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip, ai_tool, ai_project, ai_session_id)
         VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', ?1, ?1, 'transcript://claude_project', 'claude', 'cortex', 'sess-1')",
        rusqlite::params![message],
    )
    .unwrap();
    conn.last_insert_rowid()
}

const HOOK_ATTACHMENT_JSON: &str = r#"{"attachment":{"type":"hook_success","hookName":"format-on-save","hookEvent":"PostToolUse","exitCode":0}}"#;

#[tokio::test]
#[serial(hook_backfill_guard)]
async fn dry_run_reports_counts_without_inserting() {
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    insert_claude_hook_log_row(&pool, HOOK_ATTACHMENT_JSON);

    let result = service
        .backfill_hook_events(HookBackfillRequest {
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
        .query_row("SELECT COUNT(*) FROM ai_hook_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
#[serial(hook_backfill_guard)]
async fn real_run_inserts_events_and_is_idempotent() {
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    insert_claude_hook_log_row(&pool, HOOK_ATTACHMENT_JSON);

    let first = service
        .backfill_hook_events(HookBackfillRequest {
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
        .backfill_hook_events(HookBackfillRequest {
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
#[serial(hook_backfill_guard)]
async fn codex_rows_are_skipped_entirely() {
    // GH #105: no Codex runtime-hook parser exists yet. The backfill's
    // candidate query filters to `ai_tool = 'claude'` only, so a Codex log
    // row is never even fetched as a candidate (scanned stays 0) even if it
    // happens to contain the literal substring "hook_" somewhere in
    // unrelated content — stronger than a post-fetch skip.
    let (service, _dir) = test_service();
    let pool = service.pool_for_test();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip, ai_tool, ai_project, ai_session_id)
         VALUES ('2026-06-01T00:00:00.000Z', 'devhost', 'info', ?1, ?1, 'transcript://codex_session', 'codex', 'cortex', 'sess-1')",
        rusqlite::params![HOOK_ATTACHMENT_JSON],
    )
    .unwrap();
    drop(conn);

    let result = service
        .backfill_hook_events(HookBackfillRequest {
            since: None,
            limit: Some(100),
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(result.scanned, 0);
    assert_eq!(result.inserted, 0);
}

#[tokio::test]
#[serial(hook_backfill_guard)]
async fn limit_is_clamped_to_hard_upper_bound() {
    let (service, _dir) = test_service();
    insert_claude_hook_log_row(&service.pool_for_test(), HOOK_ATTACHMENT_JSON);

    let result = service
        .backfill_hook_events(HookBackfillRequest {
            since: None,
            limit: Some(10_000_000),
            dry_run: true,
        })
        .await
        .unwrap();
    assert_eq!(result.scanned, 1);
}

#[tokio::test]
#[serial(hook_backfill_guard)]
async fn concurrent_backfill_calls_return_busy_instead_of_racing() {
    let (service, _dir) = test_service();
    insert_claude_hook_log_row(&service.pool_for_test(), HOOK_ATTACHMENT_JSON);

    let _held = super::backfill_guard()
        .clone()
        .try_acquire_owned()
        .expect("guard should be free at test start");

    let result = service
        .backfill_hook_events(HookBackfillRequest {
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
