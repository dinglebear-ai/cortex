use super::*;
use crate::artifact_evidence::{ArtifactEvidenceInput, ArtifactEvidenceKind};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListArtifactEvidenceRequest {
    pub event_kind: Option<ArtifactEvidenceKind>,
    pub artifact_id: Option<String>,
    pub revision_id: Option<String>,
    pub content_digest: Option<String>,
    pub correlation_id: Option<String>,
    pub request_id: Option<String>,
    pub target_id: Option<String>,
    pub source_system: Option<String>,
    #[serde(alias = "since")]
    pub from: Option<String>,
    #[serde(alias = "until")]
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordArtifactEvidenceResponse {
    pub cortex_log_id: i64,
    pub inserted: bool,
    pub event: ArtifactEvidenceInput,
}

impl From<db::ArtifactEvidenceAppendResult> for RecordArtifactEvidenceResponse {
    fn from(value: db::ArtifactEvidenceAppendResult) -> Self {
        Self {
            cortex_log_id: value.cortex_log_id,
            inserted: value.inserted,
            event: value.event,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEvidenceEntry {
    pub cortex_log_id: i64,
    #[serde(flatten)]
    pub event: ArtifactEvidenceInput,
}

impl From<db::ArtifactEvidenceEntry> for ArtifactEvidenceEntry {
    fn from(value: db::ArtifactEvidenceEntry) -> Self {
        Self {
            cortex_log_id: value.cortex_log_id,
            event: value.event,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListArtifactEvidenceResponse {
    pub events: Vec<ArtifactEvidenceEntry>,
    pub truncated: bool,
}

impl From<db::ListArtifactEvidenceResult> for ListArtifactEvidenceResponse {
    fn from(value: db::ListArtifactEvidenceResult) -> Self {
        Self {
            events: value.events.into_iter().map(Into::into).collect(),
            truncated: value.truncated,
        }
    }
}
