use std::sync::Arc;

use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};

use super::*;
use crate::app::models::HookAssessRequest;

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("hook-assess-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn default_hook_assess_request() -> HookAssessRequest {
    HookAssessRequest {
        incident_id: None,
        hook_event: None,
        hook_name: Some("nonexistent-hook-xyz".to_string()),
        hook_source: None,
        model: None,
        project: None,
        tool: None,
        since: None,
        until: None,
        window_minutes: None,
        correlation_window_minutes: None,
        limit: None,
        all: false,
    }
}

#[tokio::test]
async fn run_hook_assessment_errors_when_no_incident_found() {
    let (service, _pool, _dir) = test_service();
    let req = default_hook_assess_request();
    let err = service
        .run_hook_assessment_with_delta(req, false, |_| Ok(()))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("no hook incident found") || msg.contains("nonexistent-hook-xyz"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn run_hook_assessment_never_touches_gemini_when_run_llm_false() {
    // run_llm=false must skip LlmRunner::run entirely — assert via the
    // absence of any llm_invocations row for action='hook_assess', not by
    // stubbing a missing Gemini binary.
    let (service, pool, _dir) = test_service();
    let req = default_hook_assess_request();
    let _ = service
        .run_hook_assessment_with_delta(req, false, |_| Ok(()))
        .await; // Ok(_) or a "no incident found" Err are both fine here.
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_invocations WHERE action = 'hook_assess'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "run_llm=false must never invoke LlmRunner::run");
}

#[tokio::test]
async fn incident_id_targets_exactly_one_hook_incident() {
    use crate::app::models::AiHookIncidentRequest;
    use crate::app::services::seed_test_support::seed_hook_incident;

    let (service, pool, _dir) = test_service();
    seed_hook_incident(&pool, "sess-a", "format-on-save", 60);
    seed_hook_incident(&pool, "sess-b", "lint-on-stop", 30);
    let listed = service
        .list_ai_hook_incidents(AiHookIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_hook_assessment_with_delta(
            HookAssessRequest {
                incident_id: Some(target.clone()),
                ..Default::default()
            },
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].incident_id, target);
}
