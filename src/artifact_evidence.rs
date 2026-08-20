//! Bounded, authority-free artifact ecosystem evidence contract.
//!
//! Cortex records source-attributed observations. Artifact IDs, revisions,
//! digests, policy/share/lease references, and deployment/runtime references
//! remain opaque dimensions and never become authorization or publication
//! authority here.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::assessment::{looks_secretish, redact_json_value_strings};

pub const ARTIFACT_EVIDENCE_SCHEMA_V1: &str = "dinglebear.cortex-artifact-evidence/v1";
pub const ARTIFACT_EVIDENCE_APP_NAME: &str = "artifact-evidence";
pub const ARTIFACT_EVIDENCE_SYNTHETIC_HOST: &str = "cortex-artifact-evidence";
pub const MAX_EVIDENCE_BYTES: usize = 16 * 1024;
pub const MAX_EVIDENCE_WIRE_BYTES: usize = 32 * 1024;
pub const MAX_METADATA_BYTES: usize = 8 * 1024;
pub const MAX_REF_BYTES: usize = 256;
pub const MAX_SOURCE_SYSTEM_BYTES: usize = 64;
pub const MAX_METADATA_KEY_BYTES: usize = 64;
pub const MAX_METADATA_STRING_BYTES: usize = 1024;
pub const MAX_METADATA_ENTRIES: usize = 32;
pub const MAX_METADATA_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEvidenceKind {
    DiscoveryObserved,
    IntakeObserved,
    Imported,
    Installed,
    Uninstalled,
    Updated,
    Forked,
    Followed,
    AddedToGateway,
    LoadoutBound,
    GatewayLifecycle,
    RuntimeLifecycle,
    RuntimeCall,
    DeploymentPlanned,
    DeploymentStaged,
    DeploymentVerified,
    DeploymentFailed,
    DeploymentRolledBack,
    TargetLifecycle,
    PhoenixPluginLifecycle,
    ApprovalRecorded,
    CapabilityLeaseIssued,
    CapabilityLeaseUsed,
    CapabilityLeaseRevoked,
    ShareGrantCreated,
    ShareGrantUsed,
    ShareGrantRevoked,
    SecurityFinding,
    LicenseFinding,
    TrustFinding,
    QuarantineFinding,
    Failed,
    Retried,
    Cancelled,
}

impl ArtifactEvidenceKind {
    pub const ALL: &[Self] = &[
        Self::DiscoveryObserved,
        Self::IntakeObserved,
        Self::Imported,
        Self::Installed,
        Self::Uninstalled,
        Self::Updated,
        Self::Forked,
        Self::Followed,
        Self::AddedToGateway,
        Self::LoadoutBound,
        Self::GatewayLifecycle,
        Self::RuntimeLifecycle,
        Self::RuntimeCall,
        Self::DeploymentPlanned,
        Self::DeploymentStaged,
        Self::DeploymentVerified,
        Self::DeploymentFailed,
        Self::DeploymentRolledBack,
        Self::TargetLifecycle,
        Self::PhoenixPluginLifecycle,
        Self::ApprovalRecorded,
        Self::CapabilityLeaseIssued,
        Self::CapabilityLeaseUsed,
        Self::CapabilityLeaseRevoked,
        Self::ShareGrantCreated,
        Self::ShareGrantUsed,
        Self::ShareGrantRevoked,
        Self::SecurityFinding,
        Self::LicenseFinding,
        Self::TrustFinding,
        Self::QuarantineFinding,
        Self::Failed,
        Self::Retried,
        Self::Cancelled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DiscoveryObserved => "discovery_observed",
            Self::IntakeObserved => "intake_observed",
            Self::Imported => "imported",
            Self::Installed => "installed",
            Self::Uninstalled => "uninstalled",
            Self::Updated => "updated",
            Self::Forked => "forked",
            Self::Followed => "followed",
            Self::AddedToGateway => "added_to_gateway",
            Self::LoadoutBound => "loadout_bound",
            Self::GatewayLifecycle => "gateway_lifecycle",
            Self::RuntimeLifecycle => "runtime_lifecycle",
            Self::RuntimeCall => "runtime_call",
            Self::DeploymentPlanned => "deployment_planned",
            Self::DeploymentStaged => "deployment_staged",
            Self::DeploymentVerified => "deployment_verified",
            Self::DeploymentFailed => "deployment_failed",
            Self::DeploymentRolledBack => "deployment_rolled_back",
            Self::TargetLifecycle => "target_lifecycle",
            Self::PhoenixPluginLifecycle => "phoenix_plugin_lifecycle",
            Self::ApprovalRecorded => "approval_recorded",
            Self::CapabilityLeaseIssued => "capability_lease_issued",
            Self::CapabilityLeaseUsed => "capability_lease_used",
            Self::CapabilityLeaseRevoked => "capability_lease_revoked",
            Self::ShareGrantCreated => "share_grant_created",
            Self::ShareGrantUsed => "share_grant_used",
            Self::ShareGrantRevoked => "share_grant_revoked",
            Self::SecurityFinding => "security_finding",
            Self::LicenseFinding => "license_finding",
            Self::TrustFinding => "trust_finding",
            Self::QuarantineFinding => "quarantine_finding",
            Self::Failed => "failed",
            Self::Retried => "retried",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for ArtifactEvidenceKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| format!("unsupported artifact evidence kind: {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEvidenceOutcome {
    Success,
    Failure,
    Denied,
    Cancelled,
    Pending,
    Unknown,
}

impl ArtifactEvidenceOutcome {
    pub const ALL: &[Self] = &[
        Self::Success,
        Self::Failure,
        Self::Denied,
        Self::Cancelled,
        Self::Pending,
        Self::Unknown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Pending => "pending",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactEvidenceInput {
    pub schema_version: String,
    pub event_id: String,
    pub event_kind: ArtifactEvidenceKind,
    pub source_system: String,
    pub source_issuer: String,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loadout_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_lease_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ArtifactEvidenceOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

pub type NormalizedArtifactEvidence = ArtifactEvidenceInput;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ArtifactEvidenceValidationError {
    #[error("unsupported artifact evidence schemaVersion")]
    UnsupportedSchema,
    #[error("{field} must be non-empty and at most {max_bytes} bytes")]
    InvalidBoundedField {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("{field} contains control characters")]
    ControlCharacters { field: &'static str },
    #[error("{field} resembles secret material")]
    SecretLikeReference { field: &'static str },
    #[error("contentDigest must be sha256:<64 lowercase hex>")]
    InvalidDigest,
    #[error("observedAt must be a valid RFC3339 timestamp")]
    InvalidObservedAt,
    #[error("at least one artifactId, revisionId, contentDigest, or provenanceRef is required")]
    MissingSubject,
    #[error("metadata must be a JSON object")]
    MetadataNotObject,
    #[error("metadata exceeds maximum nesting depth of {MAX_METADATA_DEPTH}")]
    MetadataDepth,
    #[error("metadata object/array cardinality exceeds {MAX_METADATA_ENTRIES}")]
    MetadataCardinality,
    #[error("metadata key must be non-empty and at most {MAX_METADATA_KEY_BYTES} bytes")]
    MetadataKeyBound,
    #[error("metadata key '{key}' is forbidden because it may carry secrets or raw content")]
    ForbiddenMetadataKey { key: String },
    #[error("metadata string exceeds {MAX_METADATA_STRING_BYTES} bytes")]
    MetadataStringBound,
    #[error("metadata exceeds {MAX_METADATA_BYTES} serialized bytes")]
    MetadataBytes,
    #[error("artifact evidence exceeds {MAX_EVIDENCE_BYTES} serialized bytes")]
    EvidenceBytes,
}

impl ArtifactEvidenceInput {
    pub fn normalize(mut self) -> Result<Self, ArtifactEvidenceValidationError> {
        if self.schema_version != ARTIFACT_EVIDENCE_SCHEMA_V1 {
            return Err(ArtifactEvidenceValidationError::UnsupportedSchema);
        }
        validate_ref("eventId", &self.event_id, MAX_REF_BYTES)?;
        validate_source_system(&self.source_system)?;
        validate_ref("sourceIssuer", &self.source_issuer, MAX_REF_BYTES)?;
        for (field, value) in [
            ("artifactId", self.artifact_id.as_deref()),
            ("revisionId", self.revision_id.as_deref()),
            ("provenanceRef", self.provenance_ref.as_deref()),
            ("requestId", self.request_id.as_deref()),
            ("correlationId", self.correlation_id.as_deref()),
            ("causationId", self.causation_id.as_deref()),
            ("targetId", self.target_id.as_deref()),
            ("targetKind", self.target_kind.as_deref()),
            ("loadoutId", self.loadout_id.as_deref()),
            ("shareGrantId", self.share_grant_id.as_deref()),
            ("capabilityLeaseId", self.capability_lease_id.as_deref()),
            ("deploymentPlanId", self.deployment_plan_id.as_deref()),
            ("runtimeId", self.runtime_id.as_deref()),
            ("pluginId", self.plugin_id.as_deref()),
            ("operationRef", self.operation_ref.as_deref()),
        ] {
            if let Some(value) = value {
                validate_ref(field, value, MAX_REF_BYTES)?;
            }
        }
        if let Some(digest) = &self.content_digest {
            validate_digest(digest)?;
        }
        if self.artifact_id.is_none()
            && self.revision_id.is_none()
            && self.content_digest.is_none()
            && self.provenance_ref.is_none()
        {
            return Err(ArtifactEvidenceValidationError::MissingSubject);
        }
        self.observed_at = normalize_timestamp(&self.observed_at)?;
        if let Some(metadata) = &mut self.metadata {
            if !metadata.is_object() {
                return Err(ArtifactEvidenceValidationError::MetadataNotObject);
            }
            validate_metadata(metadata, 0)?;
            redact_json_value_strings(metadata);
            if serde_json::to_vec(metadata)
                .map_err(|_| ArtifactEvidenceValidationError::MetadataBytes)?
                .len()
                > MAX_METADATA_BYTES
            {
                return Err(ArtifactEvidenceValidationError::MetadataBytes);
            }
        }
        if serde_json::to_vec(&self)
            .map_err(|_| ArtifactEvidenceValidationError::EvidenceBytes)?
            .len()
            > MAX_EVIDENCE_BYTES
        {
            return Err(ArtifactEvidenceValidationError::EvidenceBytes);
        }
        Ok(self)
    }

    pub fn summary(&self) -> String {
        let subject = self
            .artifact_id
            .as_deref()
            .or(self.revision_id.as_deref())
            .or(self.content_digest.as_deref())
            .or(self.provenance_ref.as_deref())
            .unwrap_or("unknown");
        format!("artifact_evidence {} {subject}", self.event_kind.as_str())
    }

    pub fn source_identifier(&self) -> String {
        format!(
            "artifact-evidence://{}/{}",
            self.source_system,
            percent_encode_component(&self.source_issuer)
        )
    }
}

pub(crate) fn validate_source_system(value: &str) -> Result<(), ArtifactEvidenceValidationError> {
    if value.is_empty()
        || value.len() > MAX_SOURCE_SYSTEM_BYTES
        || value.trim() != value
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        return Err(ArtifactEvidenceValidationError::InvalidBoundedField {
            field: "sourceSystem",
            max_bytes: MAX_SOURCE_SYSTEM_BYTES,
        });
    }
    Ok(())
}

pub(crate) fn validate_ref(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ArtifactEvidenceValidationError> {
    if value.is_empty() || value.len() > max_bytes || value.trim() != value {
        return Err(ArtifactEvidenceValidationError::InvalidBoundedField { field, max_bytes });
    }
    if value.chars().any(char::is_control) {
        return Err(ArtifactEvidenceValidationError::ControlCharacters { field });
    }
    if looks_secretish(value)
        || value.to_ascii_lowercase().starts_with("bearer ")
        || value.contains("-----BEGIN ")
    {
        return Err(ArtifactEvidenceValidationError::SecretLikeReference { field });
    }
    Ok(())
}

pub(crate) fn validate_digest(value: &str) -> Result<(), ArtifactEvidenceValidationError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ArtifactEvidenceValidationError::InvalidDigest);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ArtifactEvidenceValidationError::InvalidDigest);
    }
    Ok(())
}

fn normalize_timestamp(value: &str) -> Result<String, ArtifactEvidenceValidationError> {
    DateTime::parse_from_rfc3339(value)
        .map(|ts| {
            ts.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
        .map_err(|_| ArtifactEvidenceValidationError::InvalidObservedAt)
}

fn validate_metadata(value: &Value, depth: usize) -> Result<(), ArtifactEvidenceValidationError> {
    if depth > MAX_METADATA_DEPTH {
        return Err(ArtifactEvidenceValidationError::MetadataDepth);
    }
    match value {
        Value::Object(map) => {
            if map.len() > MAX_METADATA_ENTRIES {
                return Err(ArtifactEvidenceValidationError::MetadataCardinality);
            }
            for (key, child) in map {
                if key.is_empty() || key.len() > MAX_METADATA_KEY_BYTES {
                    return Err(ArtifactEvidenceValidationError::MetadataKeyBound);
                }
                if forbidden_metadata_key(key) {
                    return Err(ArtifactEvidenceValidationError::ForbiddenMetadataKey {
                        key: key.clone(),
                    });
                }
                validate_metadata(child, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > MAX_METADATA_ENTRIES {
                return Err(ArtifactEvidenceValidationError::MetadataCardinality);
            }
            for child in values {
                validate_metadata(child, depth + 1)?;
            }
        }
        Value::String(text) if text.len() > MAX_METADATA_STRING_BYTES => {
            return Err(ArtifactEvidenceValidationError::MetadataStringBound);
        }
        _ => {}
    }
    Ok(())
}

fn forbidden_metadata_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    const SECRET: &[&str] = &[
        "apikey",
        "authorization",
        "clientsecret",
        "credential",
        "password",
        "passwd",
        "privatekey",
        "refreshtoken",
        "accesstoken",
        "secret",
        "token",
    ];
    const RAW: &[&str] = &[
        "args",
        "arguments",
        "artifactcontent",
        "artifactcontents",
        "body",
        "content",
        "inputbody",
        "outputbody",
        "prompt",
        "raw",
        "rawbody",
        "requestbody",
        "requestpayload",
        "responsebody",
        "responsepayload",
        "toolinput",
        "tooloutput",
    ];
    SECRET.iter().any(|marker| normalized.contains(marker)) || RAW.contains(&normalized.as_str())
}

fn percent_encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
#[path = "artifact_evidence_tests.rs"]
mod tests;
