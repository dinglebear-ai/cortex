use std::sync::Arc;

use super::*;
use crate::app::models::{
    AiSkillIncidentRequest, ReflectIncident, ReflectKind, ReflectMode, ReflectRequest,
};
use crate::app::services::seed_test_support::{
    seed_hook_incident, seed_mcp_incident, seed_skill_incident,
};
use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("reflect-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn request(kinds: Vec<ReflectKind>, max_assess: u32) -> ReflectRequest {
    ReflectRequest {
        since: None,
        until: None,
        project: None,
        tool: None,
        kinds,
        run_llm: false,
        max_assess,
        index: false,
    }
}

fn seed_all(pool: &DbPool) {
    seed_skill_incident(pool, "sess-skill", "alpha-skill", 60);
    seed_mcp_incident(pool, "sess-mcp", "labby", "search", 40);
    seed_hook_incident(pool, "sess-hook", "format-on-save", 20);
}

#[tokio::test]
async fn report_only_merges_kinds_ranks_and_caps_detail() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let mut progress = Vec::new();

    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 2), |line| {
            progress.push(line.to_string())
        })
        .await
        .unwrap();

    assert_eq!(report.mode, ReflectMode::ReportOnly);
    assert_eq!(report.llm_fallback_reason, None);
    assert_eq!(report.index, None);
    assert_eq!(report.assessed.len(), 2);
    assert_eq!(report.unassessed.len(), 1);
    for summary in &report.summary {
        assert_eq!(
            (summary.listed, summary.total, summary.truncated),
            (1, 1, false),
            "{:?}",
            summary.kind
        );
    }
    let scores: Vec<f64> = report
        .assessed
        .iter()
        .map(|a| a.incident.priority_score)
        .chain(report.unassessed.iter().map(|i| i.priority_score))
        .collect();
    assert!(
        scores.windows(2).all(|w| w[0] >= w[1]),
        "not ranked: {scores:?}"
    );
    assert!(
        progress
            .iter()
            .any(|line| line.starts_with("assessing 1/2"))
    );

    let llm_rows: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM llm_invocations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(llm_rows, 0, "report-only must never call the LLM");
}

#[tokio::test]
async fn every_kind_resolves_by_incident_id_and_target() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 3), |_| {})
        .await
        .unwrap();
    assert_eq!(report.assessed.len(), 3);
    for assessed in &report.assessed {
        assert_eq!(
            assessed.failure, None,
            "{:?} did not resolve",
            assessed.incident.kind
        );
        assert_eq!(assessed.assessment, None);
        assert!(assessed.findings.is_object());
    }
}

#[tokio::test]
async fn pins_until_when_absent() {
    let (service, _pool, _dir) = test_service();
    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 5), |_| {})
        .await
        .unwrap();
    assert!(report.until.is_some());
}

#[tokio::test]
async fn kinds_filter_limits_detection() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service
        .run_reflect(request(vec![ReflectKind::Hook], 5), |_| {})
        .await
        .unwrap();
    assert_eq!(report.summary.len(), 1);
    assert_eq!(report.assessed.len(), 1);
    assert_eq!(report.assessed[0].incident.kind, ReflectKind::Hook);
}

#[tokio::test]
async fn max_assess_zero_reports_summary_only() {
    let (service, pool, _dir) = test_service();
    seed_all(&pool);
    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 0), |_| {})
        .await
        .unwrap();
    assert!(report.assessed.is_empty());
    assert_eq!(report.unassessed.len(), 3);
}

#[tokio::test]
async fn empty_database_produces_an_empty_report() {
    let (service, _pool, _dir) = test_service();
    let report = service
        .run_reflect(request(ReflectKind::ALL.to_vec(), 5), |_| {})
        .await
        .unwrap();
    assert!(report.assessed.is_empty());
    assert!(report.unassessed.is_empty());
    assert!(report.summary.iter().all(|s| s.total == 0));
    assert!(report.db_path.ends_with("reflect-test.db"));
}

#[tokio::test]
async fn empty_kinds_is_invalid_input() {
    let (service, _pool, _dir) = test_service();
    let error = service
        .run_reflect(request(vec![], 5), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::InvalidInput(_)));
}

#[tokio::test]
async fn an_incident_that_no_longer_resolves_is_missing_not_an_error() {
    let (service, pool, _dir) = test_service();
    seed_skill_incident(&pool, "sess-a", "alpha-skill", 60);
    let listed = service
        .list_ai_skill_incidents(AiSkillIncidentRequest::default())
        .await
        .unwrap();
    let mut incident = ReflectIncident::from(listed.incidents[0].clone());
    incident.incident_id = "no-such-incident".to_string();

    let outcome = service
        .assess_reflect_incident(&incident, &request(vec![ReflectKind::Skill], 1), None)
        .await
        .unwrap();
    assert!(matches!(outcome, AssessOutcome::Missing));
}
