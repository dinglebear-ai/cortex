use super::*;

#[test]
fn artifact_evidence_exact_fields_have_bounded_schemas() {
    let event_id = exact_property_schema("eventId").unwrap();
    assert_eq!(event_id["type"], "string");
    assert_eq!(event_id["minLength"], 1);
    assert_eq!(event_id["maxLength"], MAX_REF_BYTES);

    let digest = exact_property_schema("contentDigest").unwrap();
    assert_eq!(digest["pattern"], "^sha256:[0-9a-f]{64}$");

    let metadata = exact_property_schema("metadata").unwrap();
    assert_eq!(metadata["type"], "object");
    assert_eq!(metadata["maxProperties"], MAX_METADATA_ENTRIES);
}

#[test]
fn artifact_evidence_exact_enums_derive_from_domain_contract() {
    let event_kind = exact_property_schema("eventKind").unwrap();
    assert_eq!(
        event_kind["enum"].as_array().unwrap().len(),
        ArtifactEvidenceKind::ALL.len()
    );

    let outcome = exact_property_schema("outcome").unwrap();
    assert_eq!(
        outcome["enum"].as_array().unwrap().len(),
        ArtifactEvidenceOutcome::ALL.len()
    );
    assert!(exact_property_schema("notAnArtifactEvidenceField").is_none());
}
