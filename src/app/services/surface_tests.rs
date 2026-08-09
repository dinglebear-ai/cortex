use std::sync::Arc;

use crate::app::models::{
    AlertsRequest, AnalysisRequest, AnalysisResponse, CorrelateRequest, CorrelateResponse,
    HostStateRequest, IncidentRequest, IngestRateRequest, IngestRequest, StateRequest,
    StatsRequest,
};
use crate::app::{CortexService, ServiceError, ServiceResult};
use crate::config::StorageConfig;
use crate::db::{DbPool, init_pool};
use crate::filetail::FileTailRequest;

fn test_service() -> (CortexService, Arc<DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("surface-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

async fn assert_heavy_limited<T: std::fmt::Debug>(result: ServiceResult<T>) {
    let err = result.expect_err("held heavy permit should reject the grouped read");
    assert!(
        matches!(err, ServiceError::Busy(ref message) if message == "heavy_read_limited"),
        "expected heavy_read_limited, got {err:?}"
    );
}

#[tokio::test]
async fn state_host_without_live_heartbeats_returns_empty_state() {
    let (service, _pool, _dir) = test_service();

    let response = service
        .state(StateRequest::Host(HostStateRequest::default()))
        .await
        .unwrap();

    assert!(matches!(response, StateResponse::Host(state) if state.is_none()));
}

#[tokio::test]
async fn correlate_state_uses_shared_reference_time_default() {
    let (service, _pool, _dir) = test_service();

    let err = service
        .correlate_domain(CorrelateRequest::State(
            crate::app::models::CorrelateStateRequest {
                reference_time: None,
                window_minutes: None,
                host: None,
                severity_min: None,
                limit: None,
            },
        ))
        .await
        .unwrap();

    assert!(!err.window.from.is_empty());
    assert!(!err.window.to.is_empty());
}

#[tokio::test]
async fn correlate_topic_empty_topic_preserves_empty_response_short_circuit() {
    let (service, _pool, _dir) = test_service();

    let response = service
        .correlate_domain(CorrelateRequest::Topic(
            crate::app::models::TopicCorrelateRequest::default(),
        ))
        .await
        .unwrap();

    let CorrelateResponse::Topic(response) = response else {
        panic!("expected topic response");
    };
    assert_eq!(response.topic, "");
    assert!(response.timeline.is_empty());
    assert!(!response.truncated);
}

#[tokio::test]
async fn analysis_errors_preserves_group_by_validation() {
    let (service, _pool, _dir) = test_service();

    let err = service
        .analysis(AnalysisRequest::Errors(
            crate::app::models::GetErrorsRequest {
                group_by: Some("hostname".into()),
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::InvalidInput(ref message) if message == "Invalid group_by 'hostname'. Supported: app_name"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn analysis_incident_preserves_mutually_exclusive_host_service_validation() {
    let (service, _pool, _dir) = test_service();

    let err = service
        .analysis(AnalysisRequest::Incident(IncidentRequest {
            around: Some("2026-01-01T00:00:00Z".into()),
            minutes: None,
            host: Some("devhost".into()),
            service: Some("cortex.service".into()),
            limit: None,
        }))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::InvalidInput(ref message) if message.starts_with("host and service cannot be combined")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn ingest_file_tails_delegates_to_existing_registry_guard() {
    let (service, _pool, _dir) = test_service();

    let err = service
        .ingest(IngestRequest::FileTails(FileTailRequest::list()))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::InvalidInput(ref message) if message == "file-tail registry is not mounted"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn grouped_expensive_reads_preserve_heavy_limiter() {
    let (mut service, _pool, _dir) = test_service();
    service.acquire_timeout = std::time::Duration::from_millis(10);
    let held = service
        .heavy_read_permits
        .clone()
        .acquire_owned()
        .await
        .expect("heavy permit");

    assert_heavy_limited(service.state(StateRequest::Fleet(Default::default())).await).await;
    assert_heavy_limited(
        service
            .stats_domain(StatsRequest::IngestRate(IngestRateRequest::default()))
            .await,
    )
    .await;
    assert_heavy_limited(
        service
            .analysis(AnalysisRequest::Patterns(Default::default()))
            .await,
    )
    .await;

    drop(held);
}

#[test]
fn grouped_domain_requests_use_stable_wire_modes() {
    let payload =
        serde_json::to_value(StatsRequest::IngestRate(IngestRateRequest::default())).unwrap();
    assert_eq!(payload["mode"], "ingest_rate");

    let alert = serde_json::to_value(AlertsRequest::NotificationsRecent(
        crate::app::models::NotificationsRecentRequest {
            limit: None,
            rule_id: None,
            since: None,
        },
    ))
    .expect("alerts request serializes");
    assert_eq!(alert["mode"], "notifications_recent");

    let analysis = serde_json::to_value(AnalysisResponse::Errors(
        crate::app::models::GetErrorsResponse {
            summary: Vec::new(),
        },
    ))
    .expect("analysis response serializes");
    assert_eq!(analysis["mode"], "errors");
}
