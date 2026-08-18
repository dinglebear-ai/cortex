//! Agent Observatory evidence scoring and primary-worktree selection.

use crate::db::agent_observatory::EvidenceTrustLevel;
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Minimum confidence required for a candidate to become the primary worktree.
pub const PRIMARY_CONFIDENCE_THRESHOLD: f64 = 0.75;

/// Evidence kinds frozen by the Agent Observatory attribution contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttributionKind {
    HookCwd,
    OtlpSessionPath,
    AgentCommandCwd,
    TranscriptProjectPath,
    LifecycleHostProcess,
    AtuinCwdWindow,
    UniqueActiveHostCwd,
    TimestampProximity,
}

impl AttributionKind {
    /// Stable database/API representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HookCwd => "hook_cwd",
            Self::OtlpSessionPath => "otlp_session_path",
            Self::AgentCommandCwd => "agent_command_cwd",
            Self::TranscriptProjectPath => "transcript_project_path",
            Self::LifecycleHostProcess => "lifecycle_host_process",
            Self::AtuinCwdWindow => "atuin_cwd_window",
            Self::UniqueActiveHostCwd => "unique_active_host_cwd",
            Self::TimestampProximity => "timestamp_proximity",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "hook_cwd" => Some(Self::HookCwd),
            "otlp_session_path" => Some(Self::OtlpSessionPath),
            "agent_command_cwd" => Some(Self::AgentCommandCwd),
            "transcript_project_path" => Some(Self::TranscriptProjectPath),
            "lifecycle_host_process" => Some(Self::LifecycleHostProcess),
            "atuin_cwd_window" => Some(Self::AtuinCwdWindow),
            "unique_active_host_cwd" => Some(Self::UniqueActiveHostCwd),
            "timestamp_proximity" => Some(Self::TimestampProximity),
            _ => None,
        }
    }

    const fn maximum_confidence(self) -> f64 {
        match self {
            Self::TimestampProximity => 0.50,
            _ => 1.0,
        }
    }

    const fn can_select_primary(self) -> bool {
        !matches!(self, Self::TimestampProximity)
    }
}

/// Frozen trust/confidence defaults for an evidence kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttributionDefaults {
    pub trust: EvidenceTrustLevel,
    pub confidence: f64,
}

/// Return the contract default trust and confidence for one evidence kind.
pub const fn evidence_defaults(kind: AttributionKind) -> AttributionDefaults {
    match kind {
        AttributionKind::HookCwd => AttributionDefaults {
            trust: EvidenceTrustLevel::Verified,
            confidence: 1.00,
        },
        AttributionKind::OtlpSessionPath | AttributionKind::AgentCommandCwd => {
            AttributionDefaults {
                trust: EvidenceTrustLevel::Verified,
                confidence: 0.98,
            }
        }
        AttributionKind::TranscriptProjectPath | AttributionKind::LifecycleHostProcess => {
            AttributionDefaults {
                trust: EvidenceTrustLevel::Verified,
                confidence: 0.95,
            }
        }
        AttributionKind::AtuinCwdWindow => AttributionDefaults {
            trust: EvidenceTrustLevel::Claimed,
            confidence: 0.85,
        },
        AttributionKind::UniqueActiveHostCwd => AttributionDefaults {
            trust: EvidenceTrustLevel::Correlated,
            confidence: 0.75,
        },
        AttributionKind::TimestampProximity => AttributionDefaults {
            trust: EvidenceTrustLevel::Inferred,
            confidence: 0.50,
        },
    }
}

/// Numeric trust strength used after confidence during deterministic ranking.
pub const fn trust_rank(trust: EvidenceTrustLevel) -> u8 {
    match trust {
        EvidenceTrustLevel::Verified => 4,
        EvidenceTrustLevel::Claimed => 3,
        EvidenceTrustLevel::Correlated => 2,
        EvidenceTrustLevel::Inferred => 1,
        EvidenceTrustLevel::Refuted => 0,
    }
}

/// One durable worktree-attribution evidence candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributionCandidate {
    pub worktree_id: i64,
    pub kind: AttributionKind,
    pub source: String,
    pub trust: EvidenceTrustLevel,
    pub confidence: f64,
    pub observed_at: DateTime<Utc>,
}

impl AttributionCandidate {
    /// Construct a candidate using the frozen defaults for its evidence kind.
    pub fn with_defaults(
        worktree_id: i64,
        kind: AttributionKind,
        source: impl Into<String>,
        observed_at: DateTime<Utc>,
    ) -> Self {
        let defaults = evidence_defaults(kind);
        Self {
            worktree_id,
            kind,
            source: source.into(),
            trust: defaults.trust,
            confidence: defaults.confidence,
            observed_at,
        }
    }
}

/// Validation failures encountered before attribution sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionError {
    InvalidWorktreeId,
    EmptySource,
    NonFiniteConfidence,
    ConfidenceOutOfRange,
    ConfidenceExceedsKindMaximum,
}

impl fmt::Display for AttributionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorktreeId => "worktree ID must be positive",
            Self::EmptySource => "evidence source must be non-empty",
            Self::NonFiniteConfidence => "confidence must be finite",
            Self::ConfidenceOutOfRange => "confidence must be between zero and one",
            Self::ConfidenceExceedsKindMaximum => "confidence exceeds the evidence-kind maximum",
        })
    }
}

impl std::error::Error for AttributionError {}

/// Deterministic primary-worktree selection result.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributionSelection {
    pub primary_worktree_id: Option<i64>,
    pub ranked: Vec<AttributionCandidate>,
}

fn validate(candidate: &AttributionCandidate) -> Result<(), AttributionError> {
    if candidate.worktree_id <= 0 {
        return Err(AttributionError::InvalidWorktreeId);
    }
    if candidate.source.trim().is_empty() {
        return Err(AttributionError::EmptySource);
    }
    if !candidate.confidence.is_finite() {
        return Err(AttributionError::NonFiniteConfidence);
    }
    if !(0.0..=1.0).contains(&candidate.confidence) {
        return Err(AttributionError::ConfidenceOutOfRange);
    }
    if candidate.confidence > candidate.kind.maximum_confidence() {
        return Err(AttributionError::ConfidenceExceedsKindMaximum);
    }
    Ok(())
}

fn ranking_order(left: &AttributionCandidate, right: &AttributionCandidate) -> Ordering {
    right
        .confidence
        .total_cmp(&left.confidence)
        .then_with(|| trust_rank(right.trust).cmp(&trust_rank(left.trust)))
        .then_with(|| right.observed_at.cmp(&left.observed_at))
        .then_with(|| left.worktree_id.cmp(&right.worktree_id))
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
}

fn refutation_order(left: &AttributionCandidate, right: &AttributionCandidate) -> Ordering {
    left.observed_at
        .cmp(&right.observed_at)
        .then_with(|| left.confidence.total_cmp(&right.confidence))
        .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
}

fn stronger_than_refutation(
    candidate: &AttributionCandidate,
    refutation: &AttributionCandidate,
) -> bool {
    match candidate.confidence.total_cmp(&refutation.confidence) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => {
            let refuted_original_trust = evidence_defaults(refutation.kind).trust;
            trust_rank(candidate.trust) > trust_rank(refuted_original_trust)
        }
    }
}

/// Select and rank primary-worktree candidates deterministically.
///
/// Refuted rows remain input evidence but never appear in the result. A newer
/// candidate for the same worktree/source relation must be strictly stronger
/// than the latest refutation before the relation can participate again.
pub fn select_primary_worktree(
    candidates: &[AttributionCandidate],
) -> Result<AttributionSelection, AttributionError> {
    for candidate in candidates {
        validate(candidate)?;
    }

    let mut latest_refutations: HashMap<(i64, &str), &AttributionCandidate> = HashMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.trust == EvidenceTrustLevel::Refuted)
    {
        let key = (candidate.worktree_id, candidate.source.trim());
        latest_refutations
            .entry(key)
            .and_modify(|current| {
                if refutation_order(current, candidate) == Ordering::Less {
                    *current = candidate;
                }
            })
            .or_insert(candidate);
    }

    let mut eligible: Vec<AttributionCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.trust != EvidenceTrustLevel::Refuted)
        .filter(|candidate| candidate.kind.can_select_primary())
        .filter(|candidate| {
            let key = (candidate.worktree_id, candidate.source.trim());
            latest_refutations.get(&key).is_none_or(|refutation| {
                candidate.observed_at > refutation.observed_at
                    && stronger_than_refutation(candidate, refutation)
            })
        })
        .cloned()
        .collect();

    eligible.sort_by(ranking_order);

    let mut seen_worktrees = HashSet::new();
    eligible.retain(|candidate| seen_worktrees.insert(candidate.worktree_id));

    let primary_worktree_id = eligible
        .first()
        .filter(|candidate| candidate.confidence >= PRIMARY_CONFIDENCE_THRESHOLD)
        .map(|candidate| candidate.worktree_id);

    Ok(AttributionSelection {
        primary_worktree_id,
        ranked: eligible,
    })
}

#[cfg(test)]
#[path = "attribution_tests.rs"]
mod tests;
