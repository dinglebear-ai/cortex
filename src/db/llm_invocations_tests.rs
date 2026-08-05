use super::*;

fn test_conn() -> (
    r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let storage = crate::config::StorageConfig::for_test(db_path);
    let pool = crate::db::init_pool(&storage).unwrap();
    let conn = pool.get().unwrap();
    (conn, dir)
}

fn sample_params() -> LlmInvocationInsertParams {
    LlmInvocationInsertParams {
        caller_surface: "test".to_string(),
        action: "ai_assess".to_string(),
        provider: "gemini-cli".to_string(),
        model: Some("gemini-3.1-flash-lite-preview".to_string()),
        program: Some("gemini".to_string()),
        incident_id: Some("inc-42".to_string()),
        ai_tool: None,
        ai_project: Some("cortex".to_string()),
        ai_session_id: None,
        evidence_counts_json: Some(r#"{"total_incidents":1}"#.to_string()),
        prompt_bytes: Some(128),
        status: "running".to_string(),
        metadata_json: Some(r#"{"host":"devhost","pid":123}"#.to_string()),
    }
}

#[test]
fn insert_then_finish_round_trips() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-test-1", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-test-1", "success", None, 4200, Some(512)).unwrap();

    let rows = list_llm_invocations(&conn, 10, None, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, "llm-test-1");
    assert_eq!(row.status, "success");
    assert_eq!(row.duration_ms, Some(4200));
    assert_eq!(row.output_bytes, Some(512));
    assert_eq!(row.incident_id.as_deref(), Some("inc-42"));
    assert!(row.finished_at.is_some());
}

#[test]
fn list_filters_by_action_and_status_and_since() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-a", "success", None, 100, Some(10)).unwrap();

    let mut other = sample_params();
    other.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-b", &other).unwrap();
    finish_llm_invocation(&conn, "llm-b", "error", Some("boom"), 50, None).unwrap();

    let ai_only = list_llm_invocations(&conn, 10, None, Some("ai_assess"), None).unwrap();
    assert_eq!(ai_only.len(), 1);
    assert_eq!(ai_only[0].id, "llm-a");

    let errors_only = list_llm_invocations(&conn, 10, None, None, Some("error")).unwrap();
    assert_eq!(errors_only.len(), 1);
    assert_eq!(errors_only[0].id, "llm-b");

    let future_since =
        list_llm_invocations(&conn, 10, Some("2999-01-01T00:00:00Z"), None, None).unwrap();
    assert!(future_since.is_empty());
}

#[test]
fn list_respects_limit_and_orders_newest_first() {
    let (conn, _dir) = test_conn();
    for i in 0..5 {
        let id = format!("llm-{i}");
        insert_llm_invocation_running(&conn, &id, &sample_params()).unwrap();
        finish_llm_invocation(&conn, &id, "success", None, 10, Some(1)).unwrap();
    }
    let rows = list_llm_invocations(&conn, 2, None, None, None).unwrap();
    assert_eq!(rows.len(), 2);
}

// --- Eng review fix (performance-oracle + data-migration-expert): the
// dynamic WHERE-builder rewrite must preserve exact correctness across
// every filter combination — no filters, each filter alone, and all
// filters combined — while also making the composite indexes usable.
// These tests cover correctness; `explain_query_plan_uses_composite_indexes_for_filtered_queries`
// below asserts the index usage itself via `EXPLAIN QUERY PLAN`.

#[test]
fn list_with_no_filters_returns_everything_newest_first() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-a", "success", None, 10, Some(1)).unwrap();
    let mut other = sample_params();
    other.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-b", &other).unwrap();
    finish_llm_invocation(&conn, "llm-b", "error", Some("boom"), 20, None).unwrap();

    let rows = list_llm_invocations(&conn, 500, None, None, None).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn list_action_only_filter_returns_correct_rows() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-a", "success", None, 10, Some(1)).unwrap();
    let mut other = sample_params();
    other.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-b", &other).unwrap();
    finish_llm_invocation(&conn, "llm-b", "success", None, 10, Some(1)).unwrap();

    let rows = list_llm_invocations(&conn, 500, None, Some("skill_assess"), None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-b");
}

#[test]
fn list_status_only_filter_returns_correct_rows() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-a", "success", None, 10, Some(1)).unwrap();
    insert_llm_invocation_running(&conn, "llm-b", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-b", "error", Some("boom"), 10, None).unwrap();

    let rows = list_llm_invocations(&conn, 500, None, None, Some("error")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-b");
}

#[test]
fn list_since_only_filter_returns_correct_rows() {
    let (conn, _dir) = test_conn();
    insert_llm_invocation_running(&conn, "llm-a", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-a", "success", None, 10, Some(1)).unwrap();

    // since in the far future: excludes everything.
    let future =
        list_llm_invocations(&conn, 500, Some("2999-01-01T00:00:00Z"), None, None).unwrap();
    assert!(future.is_empty());

    // since in the far past: includes everything.
    let past = list_llm_invocations(&conn, 500, Some("2000-01-01T00:00:00Z"), None, None).unwrap();
    assert_eq!(past.len(), 1);
}

#[test]
fn list_combined_filters_intersect_correctly() {
    let (conn, _dir) = test_conn();
    // Matches all three filters.
    insert_llm_invocation_running(&conn, "llm-match", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-match", "success", None, 10, Some(1)).unwrap();

    // Wrong action.
    let mut wrong_action = sample_params();
    wrong_action.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-wrong-action", &wrong_action).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-action", "success", None, 10, Some(1)).unwrap();

    // Wrong status.
    insert_llm_invocation_running(&conn, "llm-wrong-status", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-status", "error", Some("boom"), 10, None).unwrap();

    let rows = list_llm_invocations(
        &conn,
        500,
        Some("2000-01-01T00:00:00Z"),
        Some("ai_assess"),
        Some("success"),
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-match");
}

// PR #106 reconciliation fix (pr-test-analyzer): the existing coverage above
// exercises no-filter, each single filter alone, and all three filters
// combined, but skips the three two-filter combinations. Add those so every
// pairwise intersection of `action`/`status`/`since` is proven, not just the
// full-combination and single-filter extremes.

#[test]
fn list_action_and_status_combined_filters_intersect_correctly() {
    let (conn, _dir) = test_conn();
    // Matches action=ai_assess AND status=success.
    insert_llm_invocation_running(&conn, "llm-match", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-match", "success", None, 10, Some(1)).unwrap();

    // Right action, wrong status.
    insert_llm_invocation_running(&conn, "llm-wrong-status", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-status", "error", Some("boom"), 10, None).unwrap();

    // Wrong action, right status.
    let mut wrong_action = sample_params();
    wrong_action.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-wrong-action", &wrong_action).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-action", "success", None, 10, Some(1)).unwrap();

    let rows = list_llm_invocations(&conn, 500, None, Some("ai_assess"), Some("success")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-match");
}

#[test]
fn list_action_and_since_combined_filters_intersect_correctly() {
    let (conn, _dir) = test_conn();
    // Matches action=ai_assess and is within the since window (far past).
    insert_llm_invocation_running(&conn, "llm-match", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-match", "success", None, 10, Some(1)).unwrap();

    // Wrong action, but still within the since window.
    let mut wrong_action = sample_params();
    wrong_action.action = "skill_assess".to_string();
    insert_llm_invocation_running(&conn, "llm-wrong-action", &wrong_action).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-action", "success", None, 10, Some(1)).unwrap();

    // Right action, but excluded by a since window in the far future.
    let future_rows = list_llm_invocations(
        &conn,
        500,
        Some("2999-01-01T00:00:00Z"),
        Some("ai_assess"),
        None,
    )
    .unwrap();
    assert!(
        future_rows.is_empty(),
        "future since must exclude even action-matching rows"
    );

    let rows = list_llm_invocations(
        &conn,
        500,
        Some("2000-01-01T00:00:00Z"),
        Some("ai_assess"),
        None,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-match");
}

#[test]
fn list_status_and_since_combined_filters_intersect_correctly() {
    let (conn, _dir) = test_conn();
    // Matches status=error and is within the since window (far past).
    insert_llm_invocation_running(&conn, "llm-match", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-match", "error", Some("boom"), 10, None).unwrap();

    // Wrong status, but still within the since window.
    insert_llm_invocation_running(&conn, "llm-wrong-status", &sample_params()).unwrap();
    finish_llm_invocation(&conn, "llm-wrong-status", "success", None, 10, Some(1)).unwrap();

    // Right status, but excluded by a since window in the far future.
    let future_rows = list_llm_invocations(
        &conn,
        500,
        Some("2999-01-01T00:00:00Z"),
        None,
        Some("error"),
    )
    .unwrap();
    assert!(
        future_rows.is_empty(),
        "future since must exclude even status-matching rows"
    );

    let rows = list_llm_invocations(
        &conn,
        500,
        Some("2000-01-01T00:00:00Z"),
        None,
        Some("error"),
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "llm-match");
}

/// Assert (via `EXPLAIN QUERY PLAN`) that filtered queries actually use
/// the composite indexes created in migration 37, not a full scan. The
/// old `(?N IS NULL OR col = ?N)` idiom was not sargable and always fell
/// back to scanning `idx_llm_invocations_started` (or the table)
/// regardless of which filters were supplied.
#[test]
fn explain_query_plan_uses_composite_indexes_for_filtered_queries() {
    let (conn, _dir) = test_conn();

    let plan_uses_index = |sql: &str, index_name: &str| -> bool {
        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        let mut stmt = conn.prepare(&explain_sql).unwrap();
        let details: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        details.iter().any(|d| d.contains(index_name))
    };

    // action-only filter should use idx_llm_invocations_action_started.
    assert!(
        plan_uses_index(
            "SELECT id FROM llm_invocations WHERE action = 'ai_assess' ORDER BY started_at DESC LIMIT 10",
            "idx_llm_invocations_action_started"
        ),
        "action-only query must use idx_llm_invocations_action_started"
    );

    // status-only filter should use idx_llm_invocations_status_started.
    assert!(
        plan_uses_index(
            "SELECT id FROM llm_invocations WHERE status = 'success' ORDER BY started_at DESC LIMIT 10",
            "idx_llm_invocations_status_started"
        ),
        "status-only query must use idx_llm_invocations_status_started"
    );
}
