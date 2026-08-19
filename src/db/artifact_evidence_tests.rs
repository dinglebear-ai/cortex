use serde_json::json;

use super::*;
use crate::artifact_evidence::{
    ARTIFACT_EVIDENCE_SCHEMA_V1, ArtifactEvidenceInput, ArtifactEvidenceOutcome,
};
use crate::config::StorageConfig;
use crate::db::pool::init_pool;

fn test_pool() -> (crate::db::DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("test.db"))).unwrap();
    (pool, dir)
}

fn sample_event(event_id: &str, observed_at: &str) -> NormalizedArtifactEvidence {
    ArtifactEvidenceInput {
        schema_version: ARTIFACT_EVIDENCE_SCHEMA_V1.to_string(),
        event_id: event_id.to_string(),
        event_kind: ArtifactEvidenceKind::Installed,
        source_system: "labby".to_string(),
        source_issuer: "gateway:personal/jake".to_string(),
        observed_at: observed_at.to_string(),
        artifact_id: Some("artifact-123".to_string()),
        revision_id: Some("revision-456".to_string()),
        content_digest: Some(format!("sha256:{}", "a".repeat(64))),
        provenance_ref: Some("depot:artifact-123@revision-456".to_string()),
        request_id: Some(format!("req-{event_id}")),
        correlation_id: Some("corr-123".to_string()),
        causation_id: None,
        target_id: Some("target-dookie".to_string()),
        target_kind: Some("linux".to_string()),
        loadout_id: None,
        share_grant_id: None,
        capability_lease_id: None,
        deployment_plan_id: None,
        runtime_id: Some("runtime-1".to_string()),
        plugin_id: None,
        operation_ref: Some("mcp:tools/call".to_string()),
        outcome: Some(ArtifactEvidenceOutcome::Success),
        metadata: Some(json!({"durationMs": 12, "resultBytes": 80})),
    }
    .normalize()
    .unwrap()
}

#[test]
fn append_uses_canonical_logs_path_and_returns_log_id() {
    let (pool, _dir) = test_pool();
    let event = sample_event("evt-1", "2026-08-19T12:00:00Z");
    let result = record_artifact_evidence(&pool, event.clone()).unwrap();
    assert!(result.inserted);
    assert!(result.cortex_log_id > 0);
    assert_eq!(result.event, event);

    let conn = pool.get().unwrap();
    let stored: (String, String, String, String, String, String) = conn
        .query_row(
            "SELECT timestamp, hostname, app_name, message, raw, event_action FROM logs WHERE id = ?1",
            [result.cortex_log_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(stored.0, "2026-08-19T12:00:00.000Z");
    assert_eq!(stored.1, ARTIFACT_EVIDENCE_SYNTHETIC_HOST);
    assert_eq!(stored.2, ARTIFACT_EVIDENCE_APP_NAME);
    assert_eq!(stored.3, "artifact_evidence installed artifact-123");
    assert_eq!(stored.4, stored.3);
    assert_eq!(stored.5, "installed");
}

#[test]
fn exact_replay_is_idempotent_and_conflicting_reuse_fails_closed() {
    let (pool, _dir) = test_pool();
    let event = sample_event("evt-replay", "2026-08-19T12:00:00Z");
    let first = record_artifact_evidence(&pool, event.clone()).unwrap();
    let replay = record_artifact_evidence(&pool, event.clone()).unwrap();
    assert!(first.inserted);
    assert!(!replay.inserted);
    assert_eq!(first.cortex_log_id, replay.cortex_log_id);

    let mut conflict = event;
    conflict.target_id = Some("target-other".to_string());
    let err = record_artifact_evidence(&pool, conflict).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactEvidenceStoreError>(),
        Some(&ArtifactEvidenceStoreError::EventIdConflict)
    );

    let mut other_source = sample_event("evt-replay", "2026-08-19T12:00:00Z");
    other_source.source_system = "depot".to_string();
    other_source.source_issuer = "registry:hosted".to_string();
    let other = record_artifact_evidence(&pool, other_source).unwrap();
    assert!(other.inserted);
    assert_ne!(other.cortex_log_id, first.cortex_log_id);

    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE app_name = ?1",
            [ARTIFACT_EVIDENCE_APP_NAME],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn list_filters_exact_evidence_dimensions_and_orders_newest_first() {
    let (pool, _dir) = test_pool();
    let older = sample_event("evt-old", "2026-08-19T10:00:00Z");
    let mut newer = sample_event("evt-new", "2026-08-19T11:00:00Z");
    newer.event_kind = ArtifactEvidenceKind::RuntimeCall;
    newer.artifact_id = Some("artifact-999".to_string());
    newer.revision_id = Some("revision-999".to_string());
    newer.content_digest = Some(format!("sha256:{}", "b".repeat(64)));
    newer.correlation_id = Some("corr-999".to_string());
    newer.target_id = Some("target-steamy".to_string());
    record_artifact_evidence(&pool, older).unwrap();
    record_artifact_evidence(&pool, newer.clone()).unwrap();

    for params in [
        ArtifactEvidenceParams {
            event_kind: Some(ArtifactEvidenceKind::RuntimeCall),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            artifact_id: Some("artifact-999".into()),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            revision_id: Some("revision-999".into()),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            content_digest: newer.content_digest.clone(),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            correlation_id: Some("corr-999".into()),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            request_id: Some("req-evt-new".into()),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            target_id: Some("target-steamy".into()),
            ..Default::default()
        },
        ArtifactEvidenceParams {
            source_system: Some("labby".into()),
            from: Some("2026-08-19T10:30:00.000Z".into()),
            ..Default::default()
        },
    ] {
        let result = list_artifact_evidence(&pool, &params).unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].event.event_id, "evt-new");
        assert!(!result.truncated);
    }

    let all = list_artifact_evidence(&pool, &ArtifactEvidenceParams::default()).unwrap();
    assert_eq!(all.events.len(), 2);
    assert_eq!(all.events[0].event.event_id, "evt-new");
    assert_eq!(all.events[1].event.event_id, "evt-old");
}

#[test]
fn list_reports_truncation_without_over_returning() {
    let (pool, _dir) = test_pool();
    for index in 0..3 {
        record_artifact_evidence(
            &pool,
            sample_event(
                &format!("evt-{index}"),
                &format!("2026-08-19T12:00:0{index}Z"),
            ),
        )
        .unwrap();
    }
    let result = list_artifact_evidence(
        &pool,
        &ArtifactEvidenceParams {
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.events.len(), 2);
    assert!(result.truncated);
}
