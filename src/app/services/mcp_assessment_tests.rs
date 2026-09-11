use std::sync::Arc;

use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};

use super::*;
use crate::app::models::McpAssessRequest;

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("mcp-assess-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn base_req() -> McpAssessRequest {
    McpAssessRequest {
        incident_id: None,
        mcp_server: Some("nonexistent-server-xyz".to_string()),
        mcp_tool: None,
        tool_name: None,
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
async fn run_mcp_assessment_errors_when_no_target_specified() {
    let (service, _pool, _dir) = test_service();
    let req = McpAssessRequest {
        mcp_server: None,
        mcp_tool: None,
        tool_name: None,
        ..base_req()
    };
    let err = service
        .run_mcp_assessment_with_delta(req, false, |_| Ok(()))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("requires an mcp_server, mcp_tool, tool_name, or incident_id"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn run_mcp_assessment_errors_when_no_incident_found() {
    let (service, _pool, _dir) = test_service();
    let req = base_req();
    let err = service
        .run_mcp_assessment_with_delta(req, false, |_| Ok(()))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("no MCP incident found") || msg.contains("nonexistent-server-xyz"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn run_mcp_assessment_never_touches_gemini_when_run_llm_false() {
    // run_llm=false must skip LlmRunner::run entirely — assert via the
    // absence of any llm_invocations row for action='mcp_assess'.
    let (service, pool, _dir) = test_service();
    let req = base_req();
    let _ = service
        .run_mcp_assessment_with_delta(req, false, |_| Ok(()))
        .await; // Ok(_) or a "no incident found" Err are both fine here.
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_invocations WHERE action = 'mcp_assess'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "run_llm=false must never invoke LlmRunner::run");
}

#[tokio::test]
async fn incident_id_targets_exactly_one_mcp_incident() {
    use crate::app::models::AiMcpIncidentRequest;
    use crate::app::services::seed_test_support::seed_mcp_incident;

    let (service, pool, _dir) = test_service();
    seed_mcp_incident(&pool, "sess-a", "labby", "search", 60);
    seed_mcp_incident(&pool, "sess-b", "unifi", "clients", 30);
    let listed = service
        .list_ai_mcp_incidents(AiMcpIncidentRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.incidents.len(), 2);
    let target = listed.incidents[1].incident_id.clone();

    let resp = service
        .run_mcp_assessment_with_delta(
            McpAssessRequest {
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
