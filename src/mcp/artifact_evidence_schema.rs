use serde_json::{Value, json};

use crate::artifact_evidence::{
    ARTIFACT_EVIDENCE_SCHEMA_V1, ArtifactEvidenceKind, ArtifactEvidenceOutcome,
    MAX_METADATA_ENTRIES, MAX_METADATA_KEY_BYTES, MAX_METADATA_STRING_BYTES, MAX_REF_BYTES,
    MAX_SOURCE_SYSTEM_BYTES,
};

pub(super) fn exact_property_schema(field: &str) -> Option<Value> {
    let opaque_ref = || {
        json!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_REF_BYTES,
            "description": "Opaque evidence reference. Cortex records it without granting authority."
        })
    };

    Some(match field {
        "schemaVersion" => json!({
            "type": "string",
            "const": ARTIFACT_EVIDENCE_SCHEMA_V1
        }),
        "eventId" => opaque_ref(),
        "eventKind" => json!({
            "type": "string",
            "enum": ArtifactEvidenceKind::ALL
                .iter()
                .copied()
                .map(ArtifactEvidenceKind::as_str)
                .collect::<Vec<_>>()
        }),
        "sourceSystem" => json!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_SOURCE_SYSTEM_BYTES,
            "pattern": "^[A-Za-z0-9._:-]+$"
        }),
        "sourceIssuer" => opaque_ref(),
        "observedAt" => json!({
            "type": "string",
            "format": "date-time"
        }),
        "artifactId" | "revisionId" | "provenanceRef" | "requestId" | "correlationId"
        | "causationId" | "targetId" | "targetKind" | "loadoutId" | "shareGrantId"
        | "capabilityLeaseId" | "deploymentPlanId" | "runtimeId" | "pluginId" | "operationRef" => {
            opaque_ref()
        }
        "contentDigest" => json!({
            "type": "string",
            "pattern": "^sha256:[0-9a-f]{64}$"
        }),
        "outcome" => json!({
            "type": "string",
            "enum": ArtifactEvidenceOutcome::ALL
                .iter()
                .copied()
                .map(ArtifactEvidenceOutcome::as_str)
                .collect::<Vec<_>>()
        }),
        "metadata" => json!({
            "type": "object",
            "maxProperties": MAX_METADATA_ENTRIES,
            "propertyNames": {
                "minLength": 1,
                "maxLength": MAX_METADATA_KEY_BYTES
            },
            "description": format!(
                "Bounded evidence metadata only; raw artifact/tool/request/result bodies and secret-bearing keys are rejected. String leaves are limited to {MAX_METADATA_STRING_BYTES} bytes and secret-looking values are redacted before persistence."
            )
        }),
        "from" | "to" => json!({
            "type": "string",
            "description": "Evidence time bound; parsed by the shared Cortex service."
        }),
        _ => return None,
    })
}

#[cfg(test)]
#[path = "artifact_evidence_schema_tests.rs"]
mod tests;
