use anyhow::Error as AnyhowError;

use crate::artifact_evidence::{
    ArtifactEvidenceInput, MAX_REF_BYTES, validate_digest, validate_ref, validate_source_system,
};
use crate::db;

use super::super::models::{
    ListArtifactEvidenceRequest, ListArtifactEvidenceResponse, RecordArtifactEvidenceResponse,
};
use super::super::time::parse_optional_timestamp;
use super::{CortexService, ServiceError, ServiceResult};

impl CortexService {
    pub async fn record_artifact_evidence(
        &self,
        input: ArtifactEvidenceInput,
    ) -> ServiceResult<RecordArtifactEvidenceResponse> {
        let event = input
            .normalize()
            .map_err(|error| ServiceError::InvalidInput(error.to_string()))?;
        let result = self
            .run_db("record_artifact_evidence", move |pool| {
                db::record_artifact_evidence(pool, event).map_err(map_store_error)
            })
            .await?;
        Ok(result.into())
    }

    pub async fn list_artifact_evidence(
        &self,
        req: ListArtifactEvidenceRequest,
    ) -> ServiceResult<ListArtifactEvidenceResponse> {
        validate_query(&req)?;
        let from = parse_optional_timestamp(req.from.as_deref(), "from")?;
        let to = parse_optional_timestamp(req.to.as_deref(), "to")?;
        if from
            .as_deref()
            .zip(to.as_deref())
            .is_some_and(|(from, to)| from > to)
        {
            return Err(ServiceError::InvalidInput(
                "artifact evidence from must be less than or equal to to".to_string(),
            ));
        }
        let params = db::ArtifactEvidenceParams {
            event_kind: req.event_kind,
            artifact_id: req.artifact_id,
            revision_id: req.revision_id,
            content_digest: req.content_digest,
            correlation_id: req.correlation_id,
            request_id: req.request_id,
            target_id: req.target_id,
            source_system: req.source_system,
            from,
            to,
            limit: req.limit,
        };
        let result = self
            .run_db("list_artifact_evidence", move |pool| {
                db::list_artifact_evidence(pool, &params)
            })
            .await?;
        Ok(result.into())
    }
}

fn validate_query(req: &ListArtifactEvidenceRequest) -> ServiceResult<()> {
    if let Some(limit) = req.limit
        && !(1..=500).contains(&limit)
    {
        return Err(ServiceError::InvalidInput(
            "artifact evidence limit must be between 1 and 500".to_string(),
        ));
    }
    for (field, value) in [
        ("artifactId", req.artifact_id.as_deref()),
        ("revisionId", req.revision_id.as_deref()),
        ("correlationId", req.correlation_id.as_deref()),
        ("requestId", req.request_id.as_deref()),
        ("targetId", req.target_id.as_deref()),
    ] {
        if let Some(value) = value {
            validate_ref(field, value, MAX_REF_BYTES)
                .map_err(|error| ServiceError::InvalidInput(error.to_string()))?;
        }
    }
    if let Some(digest) = req.content_digest.as_deref() {
        validate_digest(digest).map_err(|error| ServiceError::InvalidInput(error.to_string()))?;
    }
    if let Some(source_system) = req.source_system.as_deref() {
        validate_source_system(source_system)
            .map_err(|error| ServiceError::InvalidInput(error.to_string()))?;
    }
    Ok(())
}

fn map_store_error(error: AnyhowError) -> AnyhowError {
    if matches!(
        error.downcast_ref::<db::ArtifactEvidenceStoreError>(),
        Some(db::ArtifactEvidenceStoreError::EventIdConflict)
    ) {
        return AnyhowError::new(ServiceError::ConstraintViolation {
            message: "artifact_evidence_event_id_conflict".to_string(),
        });
    }
    error
}

#[cfg(test)]
#[path = "artifact_evidence_tests.rs"]
mod tests;
