use serde_json::{Value, json};

use super::*;

fn valid_input() -> ArtifactEvidenceInput {
    ArtifactEvidenceInput {
        schema_version: ARTIFACT_EVIDENCE_SCHEMA_V1.to_string(),
        event_id: "evt-01JABC".to_string(),
        event_kind: ArtifactEvidenceKind::Installed,
        source_system: "labby".to_string(),
        source_issuer: "gateway:personal/jake".to_string(),
        observed_at: "2026-08-19T15:00:00-04:00".to_string(),
        artifact_id: Some("artifact-123".to_string()),
        revision_id: Some("revision-456".to_string()),
        content_digest: Some(format!("sha256:{}", "a".repeat(64))),
        provenance_ref: Some("depot:artifact-123@revision-456".to_string()),
        request_id: Some("req-123".to_string()),
        correlation_id: Some("corr-123".to_string()),
        causation_id: None,
        target_id: Some("target-node-a".to_string()),
        target_kind: Some("linux".to_string()),
        loadout_id: None,
        share_grant_id: None,
        capability_lease_id: None,
        deployment_plan_id: None,
        runtime_id: None,
        plugin_id: None,
        operation_ref: Some("mcp:tools/call".to_string()),
        outcome: Some(ArtifactEvidenceOutcome::Success),
        metadata: Some(json!({
            "durationMs": 18,
            "resultBytes": 240,
            "message": "completed with sk-super-secret hidden"
        })),
    }
}

#[test]
fn valid_event_normalizes_time_and_redacts_metadata_values() {
    let event = valid_input().normalize().expect("valid event");
    assert_eq!(event.observed_at, "2026-08-19T19:00:00.000Z");
    assert_eq!(
        event.metadata.as_ref().unwrap()["message"],
        Value::String("completed with [REDACTED] hidden".to_string())
    );
    assert!(
        event
            .source_identifier()
            .contains("gateway%3Apersonal%2Fjake")
    );
}

#[test]
fn rejects_unsupported_schema_and_unknown_wire_fields() {
    let mut event = valid_input();
    event.schema_version = "dinglebear.cortex-artifact-evidence/v2".to_string();
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::UnsupportedSchema
    );

    let mut value = serde_json::to_value(valid_input()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("publicationAuthority".into(), json!(true));
    assert!(serde_json::from_value::<ArtifactEvidenceInput>(value).is_err());
}

#[test]
fn rejects_malformed_digest_timestamp_and_missing_subject() {
    let mut event = valid_input();
    event.content_digest = Some(format!("sha256:{}", "A".repeat(64)));
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::InvalidDigest
    );

    let mut event = valid_input();
    event.observed_at = "yesterday-ish".to_string();
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::InvalidObservedAt
    );

    let mut event = valid_input();
    event.artifact_id = None;
    event.revision_id = None;
    event.content_digest = None;
    event.provenance_ref = None;
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MissingSubject
    );
}

#[test]
fn rejects_secret_keys_and_raw_tool_bodies() {
    for key in [
        "apiToken",
        "client-secret",
        "authorization",
        "requestBody",
        "response_payload",
        "arguments",
        "artifactContents",
        "prompt",
    ] {
        let mut event = valid_input();
        let mut map = serde_json::Map::new();
        map.insert(key.to_string(), json!("should never be stored"));
        event.metadata = Some(Value::Object(map));
        assert!(matches!(
            event.normalize(),
            Err(ArtifactEvidenceValidationError::ForbiddenMetadataKey { .. })
        ));
    }
}

#[test]
fn rejects_secret_like_identity_dimensions() {
    let mut event = valid_input();
    event.request_id = Some("sk-this-is-not-an-id".to_string());
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::SecretLikeReference { field: "requestId" }
    );
}

#[test]
fn enforces_metadata_cardinality_depth_and_string_bounds_without_panicking() {
    let mut too_many = serde_json::Map::new();
    for index in 0..=MAX_METADATA_ENTRIES {
        too_many.insert(format!("k{index}"), json!(index));
    }
    let mut event = valid_input();
    event.metadata = Some(Value::Object(too_many));
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MetadataCardinality
    );

    let mut nested = json!({"leaf": true});
    for index in 0..=MAX_METADATA_DEPTH {
        let mut map = serde_json::Map::new();
        map.insert(format!("level{index}"), nested);
        nested = Value::Object(map);
    }
    let mut event = valid_input();
    event.metadata = Some(nested);
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MetadataDepth
    );

    let mut event = valid_input();
    event.metadata = Some(json!({"message": "x".repeat(MAX_METADATA_STRING_BYTES + 1)}));
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MetadataStringBound
    );

    let mut oversized_array = Vec::new();
    for index in 0..=MAX_METADATA_ENTRIES {
        oversized_array.push(json!(index));
    }
    let mut event = valid_input();
    event.metadata = Some(json!({"samples": oversized_array}));
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MetadataCardinality
    );

    let mut byte_heavy = serde_json::Map::new();
    for index in 0..9 {
        byte_heavy.insert(format!("field{index}"), json!("x".repeat(1000)));
    }
    let mut event = valid_input();
    event.metadata = Some(Value::Object(byte_heavy));
    assert_eq!(
        event.normalize().unwrap_err(),
        ArtifactEvidenceValidationError::MetadataBytes
    );
}

#[test]
fn malformed_metadata_shapes_fail_closed_without_panicking() {
    for metadata in [
        Value::Null,
        Value::Bool(true),
        json!([]),
        json!("string"),
        json!(42),
    ] {
        let mut event = valid_input();
        event.metadata = Some(metadata);
        assert_eq!(
            event.normalize().unwrap_err(),
            ArtifactEvidenceValidationError::MetadataNotObject
        );
    }
}

#[test]
fn event_kind_strings_are_stable_contract_values() {
    assert_eq!(
        serde_json::to_value(ArtifactEvidenceKind::ShareGrantRevoked).unwrap(),
        json!("share_grant_revoked")
    );
    assert_eq!(
        ArtifactEvidenceKind::DeploymentRolledBack.as_str(),
        "deployment_rolled_back"
    );
    assert_eq!(
        "deployment_rolled_back"
            .parse::<ArtifactEvidenceKind>()
            .unwrap(),
        ArtifactEvidenceKind::DeploymentRolledBack
    );
    assert!(
        "publication_authorized"
            .parse::<ArtifactEvidenceKind>()
            .is_err()
    );
}
