use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::app::models::ListArtifactEvidenceRequest;
use crate::artifact_evidence::{
    ARTIFACT_EVIDENCE_SCHEMA_V1, ArtifactEvidenceKind, ArtifactEvidenceOutcome,
};
use crate::config::StorageConfig;
use crate::db::init_pool;

fn test_service() -> (CortexService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(pool, storage), dir)
}

fn input(event_id: &str, artifact_id: &str) -> ArtifactEvidenceInput {
    ArtifactEvidenceInput {
        schema_version: ARTIFACT_EVIDENCE_SCHEMA_V1.to_string(),
        event_id: event_id.to_string(),
        event_kind: ArtifactEvidenceKind::RuntimeCall,
        source_system: "labby".to_string(),
        source_issuer: "gateway:personal/jake".to_string(),
        observed_at: "2026-08-19T12:00:00-04:00".to_string(),
        artifact_id: Some(artifact_id.to_string()),
        revision_id: Some("revision-1".to_string()),
        content_digest: Some(format!("sha256:{}", "a".repeat(64))),
        provenance_ref: None,
        request_id: Some("req-1".to_string()),
        correlation_id: Some("corr-1".to_string()),
        causation_id: None,
        target_id: Some("target-node-a".to_string()),
        target_kind: Some("linux".to_string()),
        loadout_id: None,
        share_grant_id: None,
        capability_lease_id: None,
        deployment_plan_id: None,
        runtime_id: Some("runtime-1".to_string()),
        plugin_id: None,
        operation_ref: Some("mcp:tools/call".to_string()),
        outcome: Some(ArtifactEvidenceOutcome::Success),
        metadata: Some(json!({"message": "result sk-secret", "resultBytes": 42})),
    }
}

#[tokio::test]
async fn record_and_query_share_validation_and_persistence_semantics() {
    let (service, _dir) = test_service();
    let recorded = service
        .record_artifact_evidence(input("evt-1", "artifact-1"))
        .await
        .unwrap();
    assert!(recorded.inserted);
    assert_eq!(recorded.event.observed_at, "2026-08-19T16:00:00.000Z");
    assert_eq!(
        recorded.event.metadata.unwrap()["message"],
        "result [REDACTED]"
    );

    let result = service
        .list_artifact_evidence(ListArtifactEvidenceRequest {
            artifact_id: Some("artifact-1".to_string()),
            correlation_id: Some("corr-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event.event_id, "evt-1");
    assert!(!result.truncated);
}

#[tokio::test]
async fn exact_replay_is_noop_and_conflict_is_typed() {
    let (service, _dir) = test_service();
    let original = input("evt-replay", "artifact-1");
    let first = service
        .record_artifact_evidence(original.clone())
        .await
        .unwrap();
    let replay = service.record_artifact_evidence(original).await.unwrap();
    assert!(first.inserted);
    assert!(!replay.inserted);
    assert_eq!(first.cortex_log_id, replay.cortex_log_id);

    let conflict = service
        .record_artifact_evidence(input("evt-replay", "artifact-2"))
        .await
        .unwrap_err();
    assert!(matches!(
        conflict,
        ServiceError::ConstraintViolation { ref message }
            if message == "artifact_evidence_event_id_conflict"
    ));
}

#[tokio::test]
async fn query_rejects_unbounded_or_secret_like_filters_before_sql() {
    let (service, _dir) = test_service();
    let err = service
        .list_artifact_evidence(ListArtifactEvidenceRequest {
            limit: Some(501),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::InvalidInput(_)));

    let err = service
        .list_artifact_evidence(ListArtifactEvidenceRequest {
            request_id: Some("sk-not-a-request-id".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::InvalidInput(_)));

    let err = service
        .list_artifact_evidence(ListArtifactEvidenceRequest {
            content_digest: Some("sha256:ABC".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::InvalidInput(_)));
}

#[tokio::test]
async fn query_rejects_inverted_time_window() {
    let (service, _dir) = test_service();
    let err = service
        .list_artifact_evidence(ListArtifactEvidenceRequest {
            from: Some("2026-08-20T00:00:00Z".to_string()),
            to: Some("2026-08-19T00:00:00Z".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::InvalidInput(_)));
}
