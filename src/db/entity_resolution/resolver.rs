//! Deterministic resolver: converts bounded observations into ranked,
//! evidence-backed entity decisions and lookup diagnostics.
//!
//! Resolution is deterministic-first: no LLM calls, no fuzzy or substring
//! matching. Raw app labels never upgrade themselves into logical-service
//! identity; only structured observations (agent Docker metadata, verified
//! inventory) produce `logical_service` / `service_instance` decisions.

use std::collections::BTreeMap;

use super::observation::{ObservationKind, ResolverObservation, ResolverTrust};
use super::vocab::{ENTITY_TYPE_LOGICAL_SERVICE, ENTITY_TYPE_SERVICE_INSTANCE};

/// Evidence rows kept per decision/diagnostic sample.
pub const MAX_RESOLVER_EVIDENCE_SAMPLE: usize = 5;

/// Outcome class of a resolution or lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverStatus {
    Resolved,
    Ambiguous,
    RejectedLegacyShape,
    Degraded,
}

impl ResolverStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResolverStatus::Resolved => "resolved",
            ResolverStatus::Ambiguous => "ambiguous",
            ResolverStatus::RejectedLegacyShape => "rejected_legacy_shape",
            ResolverStatus::Degraded => "degraded",
        }
    }
}

/// One piece of evidence backing a resolver decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverEvidence {
    pub rule_id: &'static str,
    pub source_kind: String,
    pub source_id: String,
    pub evidence_path: String,
    pub trust: ResolverTrust,
    pub safe_excerpt: Option<String>,
}

/// A resolved canonical entity with its supporting evidence sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntityDecision {
    pub entity_type: &'static str,
    pub canonical_key: String,
    pub display_label: String,
    pub status: ResolverStatus,
    pub trust: ResolverTrust,
    pub evidence: Vec<ResolverEvidence>,
}

/// Diagnostic result for a lookup input (topic, graph key, alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverDiagnostic {
    pub status: ResolverStatus,
    pub input: String,
    pub reason: String,
    pub candidates: Vec<ResolvedEntityDecision>,
    pub evidence_sample: Vec<ResolverEvidence>,
    pub total_evidence_count: usize,
}

/// Deterministically resolve observations into entity decisions.
///
/// Only structured service identity observations produce decisions:
/// `ServiceInstance` observations yield both the instance and its logical
/// service; `LogicalService` observations yield the logical service.
/// `RawAppLabel` observations never produce decisions (no self-upgrade).
/// Single pass over the observations with keyed aggregation.
pub fn resolve_observations(observations: &[ResolverObservation]) -> Vec<ResolvedEntityDecision> {
    let mut by_entity: BTreeMap<(&'static str, String), Vec<ResolverEvidence>> = BTreeMap::new();
    for obs in observations {
        match obs.kind {
            ObservationKind::LogicalService => {
                if let Some(key) = obs.logical_service_key.clone() {
                    by_entity
                        .entry((ENTITY_TYPE_LOGICAL_SERVICE, key))
                        .or_default()
                        .push(evidence(obs, "logical_service_observation"));
                } else {
                    skipped_missing_key(obs, "logical_service_key");
                }
            }
            ObservationKind::ServiceInstance => {
                if let Some(key) = obs.service_instance_key.clone() {
                    by_entity
                        .entry((ENTITY_TYPE_SERVICE_INSTANCE, key))
                        .or_default()
                        .push(evidence(obs, "service_instance_observation"));
                } else {
                    skipped_missing_key(obs, "service_instance_key");
                }
                if let Some(key) = obs.logical_service_key.clone() {
                    by_entity
                        .entry((ENTITY_TYPE_LOGICAL_SERVICE, key))
                        .or_default()
                        .push(evidence(obs, "service_instance_logical_service"));
                } else {
                    skipped_missing_key(obs, "logical_service_key");
                }
            }
            // Raw app labels are weak claims: never a decision by themselves.
            ObservationKind::RawAppLabel => {}
            _ => {}
        }
    }
    by_entity
        .into_iter()
        .map(|((entity_type, canonical_key), evidence)| {
            let trust = evidence
                .iter()
                .map(|e| e.trust)
                .min()
                .unwrap_or(ResolverTrust::Inferred);
            ResolvedEntityDecision {
                entity_type,
                display_label: canonical_key.clone(),
                canonical_key,
                status: ResolverStatus::Resolved,
                trust,
                evidence: evidence
                    .into_iter()
                    .take(MAX_RESOLVER_EVIDENCE_SAMPLE)
                    .collect(),
            }
        })
        .collect()
}

/// Classify a lookup input before any graph lookup. Legacy nested service
/// shapes (`nashost:plex`, `nashost:plex:plex`, `plex/plex/plex`) are rejected
/// outright; anything else is degraded pending candidate resolution by the
/// caller (which owns database access).
pub fn diagnose_lookup_input(input: &str) -> ResolverDiagnostic {
    if super::vocab::classify_legacy_shape(input).is_some() {
        return ResolverDiagnostic {
            status: ResolverStatus::RejectedLegacyShape,
            input: input.to_string(),
            reason: "rejected_legacy_shape".to_string(),
            candidates: Vec::new(),
            evidence_sample: Vec::new(),
            total_evidence_count: 0,
        };
    }
    ResolverDiagnostic {
        status: ResolverStatus::Degraded,
        input: input.to_string(),
        reason: "no_resolver_candidates".to_string(),
        candidates: Vec::new(),
        evidence_sample: Vec::new(),
        total_evidence_count: 0,
    }
}

/// Adapters always populate the keys their observation kind requires; a miss
/// here means an adapter bug, so it is loud in debug builds and traced (not
/// silently dropped) in release builds.
fn skipped_missing_key(obs: &ResolverObservation, missing: &'static str) {
    debug_assert!(
        false,
        "{:?} observation missing required {missing} (observed_key={:?}, source={}:{})",
        obs.kind, obs.observed_key, obs.source_kind, obs.source_id
    );
    tracing::debug!(
        kind = ?obs.kind,
        observed_key = %obs.observed_key,
        source_kind = %obs.source_kind,
        source_id = %obs.source_id,
        missing,
        "resolver skipped observation missing its required key"
    );
}

fn evidence(obs: &ResolverObservation, rule_id: &'static str) -> ResolverEvidence {
    ResolverEvidence {
        rule_id,
        source_kind: obs.source_kind.clone(),
        source_id: obs.source_id.clone(),
        evidence_path: obs.evidence_path.clone(),
        trust: obs.trust,
        safe_excerpt: Some(obs.display_label.clone()),
    }
}
