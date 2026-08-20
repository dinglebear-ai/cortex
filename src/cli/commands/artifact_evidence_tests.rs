use super::*;

#[test]
fn parses_artifact_evidence_query_flags() {
    let command = parse_artifact_evidence(&[
        "--event-kind".into(),
        "runtime_call".into(),
        "--artifact-id".into(),
        "artifact-1".into(),
        "--revision-id".into(),
        "revision-1".into(),
        "--content-digest".into(),
        format!("sha256:{}", "a".repeat(64)),
        "--correlation-id".into(),
        "corr-1".into(),
        "--request-id".into(),
        "req-1".into(),
        "--target-id".into(),
        "target-node-a".into(),
        "--source-system".into(),
        "labby".into(),
        "--since".into(),
        "2026-08-19T15:00:00Z".into(),
        "--until".into(),
        "2026-08-19T17:00:00Z".into(),
        "--limit".into(),
        "25".into(),
        "--json".into(),
    ])
    .unwrap();

    let CliCommand::ArtifactEvidence(args) = command else {
        panic!("expected artifact evidence command");
    };
    assert_eq!(args.event_kind, Some(ArtifactEvidenceKind::RuntimeCall));
    assert_eq!(args.artifact_id.as_deref(), Some("artifact-1"));
    assert_eq!(args.revision_id.as_deref(), Some("revision-1"));
    assert_eq!(args.correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(args.request_id.as_deref(), Some("req-1"));
    assert_eq!(args.target_id.as_deref(), Some("target-node-a"));
    assert_eq!(args.source_system.as_deref(), Some("labby"));
    assert_eq!(args.limit, Some(25));
    assert!(args.json);
}

#[test]
fn rejects_unknown_event_kind_and_option() {
    let err = parse_artifact_evidence(&["--event-kind".into(), "publication_authorized".into()])
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported artifact evidence kind")
    );

    let err = parse_artifact_evidence(&["--raw-body".into(), "secret".into()]).unwrap_err();
    assert!(err.to_string().contains("unknown artifactevents option"));
}
