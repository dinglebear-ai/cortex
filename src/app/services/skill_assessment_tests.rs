use std::sync::Arc;

use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};

use super::*;
use crate::app::models::SkillAssessRequest;

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("skill-assess-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

#[tokio::test]
async fn run_skill_assessment_errors_when_no_incident_found() {
    let (service, _pool, _dir) = test_service();
    let req = SkillAssessRequest {
        incident_id: None,
        skill: Some("nonexistent-skill-xyz".to_string()),
        plugin: None,
        model: None,
        project: None,
        tool: None,
        since: None,
        until: None,
        window_minutes: None,
        correlation_window_minutes: None,
        limit: None,
        all: false,
    };
    let err = service
        .run_skill_assessment_with_delta(req, false, |_| Ok(()))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("no skill incident found") || msg.contains("nonexistent-skill-xyz"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn run_skill_assessment_never_touches_gemini_when_run_llm_false() {
    // run_llm=false must skip LlmRunner::run entirely — assert via the
    // absence of any llm_invocations row for action='skill_assess', not by
    // stubbing a missing Gemini binary (LlmRunner would itself refuse to
    // spawn a nonexistent binary, so a binary-not-found assertion alone
    // does not prove run_llm was honored; the audit-table absence does).
    let (service, pool, _dir) = test_service();
    let req = SkillAssessRequest {
        incident_id: None,
        skill: Some("frustration-assessment".to_string()),
        plugin: None,
        model: None,
        project: None,
        tool: None,
        since: None,
        until: None,
        window_minutes: None,
        correlation_window_minutes: None,
        limit: None,
        all: false,
    };
    let _ = service
        .run_skill_assessment_with_delta(req, false, |_| Ok(()))
        .await; // Ok(_) or a "no incident found" Err are both fine here.
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_invocations WHERE action = 'skill_assess'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "run_llm=false must never invoke LlmRunner::run");
}

#[tokio::test]
async fn plugin_only_request_forwards_plugin_to_investigate_ai_skill_incidents() {
    let (service, _pool, _dir) = test_service();
    let req = SkillAssessRequest {
        incident_id: None,
        skill: None,
        plugin: Some("no-such-plugin-xyz".to_string()),
        model: None,
        project: None,
        tool: None,
        since: None,
        until: None,
        window_minutes: None,
        correlation_window_minutes: None,
        limit: None,
        all: false,
    };
    // No matching data: expect the "no skill incident found" InvalidInput
    // path (proves the plugin field was forwarded and consulted, not
    // silently dropped) rather than the "skill name or --plugin is
    // required" validation error (which would prove it was dropped).
    let err = service
        .run_skill_assessment_with_delta(req, false, |_| Ok(()))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("no skill incident found"),
        "plugin-only request must reach investigate_ai_skill_incidents, got: {msg}"
    );
}

#[tokio::test]
async fn run_skill_assessment_with_delta_run_llm_false_writes_no_llm_invocation_row() {
    let (service, pool, _dir) = test_service();
    let req = SkillAssessRequest {
        incident_id: None,
        skill: Some("frustration-assessment".to_string()),
        plugin: None,
        model: None,
        project: None,
        tool: None,
        since: None,
        until: None,
        window_minutes: None,
        correlation_window_minutes: None,
        limit: None,
        all: false,
    };
    let _ = service
        .run_skill_assessment_with_delta(req, false, |_| Ok(()))
        .await;
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_invocations WHERE action = 'skill_assess'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "run_llm=false must never write an llm_invocations row (LlmRunner::run must not be called)"
    );
}

#[tokio::test]
async fn incident_id_targets_exactly_one_skill_incident() {
    use crate::app::models::AiSkillIncidentRequest;
    use crate::app::services::seed_test_support::seed_skill_incident;

    let (service, pool, _dir) = test_service();
    seed_skill_incident(&pool, "sess-a", "alpha-skill", 60);
    seed_skill_incident(&pool, "sess-b", "beta-skill", 30);
    let listed = service
        .list_ai_skill_incidents(AiSkillIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_skill_assessment_with_delta(
            SkillAssessRequest {
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
