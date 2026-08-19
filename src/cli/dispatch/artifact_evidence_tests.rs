use super::*;
use cortex::artifact_evidence::ArtifactEvidenceKind;

#[test]
fn artifact_evidence_args_map_to_shared_request_without_transport_specific_fields() {
    let req = ArtifactEvidenceArgs {
        event_kind: Some(ArtifactEvidenceKind::RuntimeCall),
        artifact_id: Some("artifact-1".to_string()),
        revision_id: Some("revision-1".to_string()),
        content_digest: Some(format!("sha256:{}", "a".repeat(64))),
        correlation_id: Some("corr-1".to_string()),
        request_id: Some("req-1".to_string()),
        target_id: Some("target-dookie".to_string()),
        source_system: Some("labby".to_string()),
        since: Some("2026-08-19T15:00:00.000Z".to_string()),
        until: Some("2026-08-19T17:00:00.000Z".to_string()),
        limit: Some(25),
        json: true,
    }
    .into_request();

    assert_eq!(req.event_kind, Some(ArtifactEvidenceKind::RuntimeCall));
    assert_eq!(req.artifact_id.as_deref(), Some("artifact-1"));
    assert_eq!(req.revision_id.as_deref(), Some("revision-1"));
    assert_eq!(req.correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(req.request_id.as_deref(), Some("req-1"));
    assert_eq!(req.target_id.as_deref(), Some("target-dookie"));
    assert_eq!(req.source_system.as_deref(), Some("labby"));
    assert_eq!(req.from.as_deref(), Some("2026-08-19T15:00:00.000Z"));
    assert_eq!(req.to.as_deref(), Some("2026-08-19T17:00:00.000Z"));
    assert_eq!(req.limit, Some(25));
}
