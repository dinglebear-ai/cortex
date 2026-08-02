use super::{
    AttributionCandidate, AttributionError, AttributionKind, PRIMARY_CONFIDENCE_THRESHOLD,
    evidence_defaults, select_primary_worktree, trust_rank,
};
use crate::db::agent_observatory::EvidenceTrustLevel;
use chrono::{DateTime, TimeDelta, Utc};

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-02T16:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn candidate(
    worktree_id: i64,
    kind: AttributionKind,
    source: &str,
    trust: EvidenceTrustLevel,
    confidence: f64,
    seconds: i64,
) -> AttributionCandidate {
    AttributionCandidate {
        worktree_id,
        kind,
        source: source.to_string(),
        trust,
        confidence,
        observed_at: base_time() + TimeDelta::seconds(seconds),
    }
}

fn ranked_ids(candidates: &[AttributionCandidate]) -> Vec<i64> {
    select_primary_worktree(candidates)
        .unwrap()
        .ranked
        .iter()
        .map(|candidate| candidate.worktree_id)
        .collect()
}

#[test]
fn evidence_defaults_match_the_frozen_contract() {
    let cases = [
        (AttributionKind::HookCwd, EvidenceTrustLevel::Verified, 1.00),
        (
            AttributionKind::OtlpSessionPath,
            EvidenceTrustLevel::Verified,
            0.98,
        ),
        (
            AttributionKind::AgentCommandCwd,
            EvidenceTrustLevel::Verified,
            0.98,
        ),
        (
            AttributionKind::TranscriptProjectPath,
            EvidenceTrustLevel::Verified,
            0.95,
        ),
        (
            AttributionKind::LifecycleHostProcess,
            EvidenceTrustLevel::Verified,
            0.95,
        ),
        (
            AttributionKind::AtuinCwdWindow,
            EvidenceTrustLevel::Claimed,
            0.85,
        ),
        (
            AttributionKind::UniqueActiveHostCwd,
            EvidenceTrustLevel::Correlated,
            0.75,
        ),
        (
            AttributionKind::TimestampProximity,
            EvidenceTrustLevel::Inferred,
            0.50,
        ),
    ];

    for (kind, trust, confidence) in cases {
        let defaults = evidence_defaults(kind);
        assert_eq!(defaults.trust, trust);
        assert_eq!(defaults.confidence, confidence);
        assert_eq!(
            AttributionCandidate::with_defaults(42, kind, "source", base_time()).confidence,
            confidence
        );
    }
    assert_eq!(PRIMARY_CONFIDENCE_THRESHOLD, 0.75);
}

#[test]
fn trust_rank_matches_contract_strength() {
    assert!(trust_rank(EvidenceTrustLevel::Verified) > trust_rank(EvidenceTrustLevel::Claimed));
    assert!(trust_rank(EvidenceTrustLevel::Claimed) > trust_rank(EvidenceTrustLevel::Correlated));
    assert!(trust_rank(EvidenceTrustLevel::Correlated) > trust_rank(EvidenceTrustLevel::Inferred));
    assert!(trust_rank(EvidenceTrustLevel::Inferred) > trust_rank(EvidenceTrustLevel::Refuted));
}

#[test]
fn confidence_precedes_trust_rank() {
    let candidates = vec![
        candidate(
            1,
            AttributionKind::AtuinCwdWindow,
            "claimed",
            EvidenceTrustLevel::Verified,
            0.80,
            1,
        ),
        candidate(
            2,
            AttributionKind::AtuinCwdWindow,
            "correlated",
            EvidenceTrustLevel::Correlated,
            0.90,
            1,
        ),
    ];
    let selection = select_primary_worktree(&candidates).unwrap();
    assert_eq!(selection.primary_worktree_id, Some(2));
    assert_eq!(ranked_ids(&candidates), vec![2, 1]);
}

#[test]
fn equal_confidence_uses_trust_then_last_seen_then_worktree_id() {
    let trust_tie = vec![
        candidate(
            1,
            AttributionKind::AtuinCwdWindow,
            "claimed",
            EvidenceTrustLevel::Claimed,
            0.85,
            10,
        ),
        candidate(
            2,
            AttributionKind::AtuinCwdWindow,
            "verified",
            EvidenceTrustLevel::Verified,
            0.85,
            0,
        ),
    ];
    assert_eq!(ranked_ids(&trust_tie), vec![2, 1]);

    let time_tie = vec![
        candidate(
            1,
            AttributionKind::HookCwd,
            "old",
            EvidenceTrustLevel::Verified,
            1.0,
            1,
        ),
        candidate(
            2,
            AttributionKind::HookCwd,
            "new",
            EvidenceTrustLevel::Verified,
            1.0,
            2,
        ),
    ];
    assert_eq!(ranked_ids(&time_tie), vec![2, 1]);

    let id_tie = vec![
        candidate(
            9,
            AttributionKind::HookCwd,
            "nine",
            EvidenceTrustLevel::Verified,
            1.0,
            2,
        ),
        candidate(
            4,
            AttributionKind::HookCwd,
            "four",
            EvidenceTrustLevel::Verified,
            1.0,
            2,
        ),
    ];
    assert_eq!(ranked_ids(&id_tie), vec![4, 9]);
}

#[test]
fn below_threshold_and_timestamp_only_evidence_never_select_primary() {
    let below = vec![candidate(
        1,
        AttributionKind::UniqueActiveHostCwd,
        "below",
        EvidenceTrustLevel::Verified,
        0.749,
        1,
    )];
    assert_eq!(
        select_primary_worktree(&below).unwrap().primary_worktree_id,
        None
    );

    let timestamp = vec![AttributionCandidate::with_defaults(
        2,
        AttributionKind::TimestampProximity,
        "time",
        base_time(),
    )];
    let selection = select_primary_worktree(&timestamp).unwrap();
    assert_eq!(selection.primary_worktree_id, None);
    assert!(selection.ranked.is_empty());
}

#[test]
fn latest_refutation_blocks_the_same_source_relation() {
    let candidates = vec![
        candidate(
            1,
            AttributionKind::HookCwd,
            "hook:7",
            EvidenceTrustLevel::Verified,
            1.0,
            1,
        ),
        candidate(
            1,
            AttributionKind::HookCwd,
            "hook:7",
            EvidenceTrustLevel::Refuted,
            1.0,
            2,
        ),
        candidate(
            2,
            AttributionKind::AtuinCwdWindow,
            "atuin:9",
            EvidenceTrustLevel::Claimed,
            0.85,
            1,
        ),
    ];
    let selection = select_primary_worktree(&candidates).unwrap();
    assert_eq!(selection.primary_worktree_id, Some(2));
    assert_eq!(ranked_ids(&candidates), vec![2]);
}

#[test]
fn newer_stronger_evidence_recovers_after_refutation() {
    let candidates = vec![
        candidate(
            1,
            AttributionKind::AgentCommandCwd,
            "command:3",
            EvidenceTrustLevel::Refuted,
            0.98,
            2,
        ),
        candidate(
            1,
            AttributionKind::HookCwd,
            "command:3",
            EvidenceTrustLevel::Verified,
            1.0,
            3,
        ),
    ];
    assert_eq!(
        select_primary_worktree(&candidates)
            .unwrap()
            .primary_worktree_id,
        Some(1)
    );
}

#[test]
fn newer_weaker_evidence_remains_blocked_by_refutation() {
    let candidates = vec![
        candidate(
            1,
            AttributionKind::HookCwd,
            "hook:3",
            EvidenceTrustLevel::Refuted,
            1.0,
            2,
        ),
        candidate(
            1,
            AttributionKind::AtuinCwdWindow,
            "hook:3",
            EvidenceTrustLevel::Claimed,
            0.85,
            3,
        ),
        candidate(
            2,
            AttributionKind::UniqueActiveHostCwd,
            "host:4",
            EvidenceTrustLevel::Correlated,
            0.75,
            1,
        ),
    ];
    assert_eq!(
        select_primary_worktree(&candidates)
            .unwrap()
            .primary_worktree_id,
        Some(2)
    );
}

#[test]
fn strongest_evidence_per_worktree_is_ranked_once() {
    let candidates = vec![
        candidate(
            1,
            AttributionKind::AtuinCwdWindow,
            "weak",
            EvidenceTrustLevel::Claimed,
            0.85,
            3,
        ),
        candidate(
            1,
            AttributionKind::HookCwd,
            "strong",
            EvidenceTrustLevel::Verified,
            1.0,
            1,
        ),
        candidate(
            2,
            AttributionKind::OtlpSessionPath,
            "other",
            EvidenceTrustLevel::Verified,
            0.98,
            2,
        ),
    ];
    assert_eq!(ranked_ids(&candidates), vec![1, 2]);
}

fn shuffled(mut values: Vec<AttributionCandidate>, seed: u64) -> Vec<AttributionCandidate> {
    let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    for index in (1..values.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let swap = (state as usize) % (index + 1);
        values.swap(index, swap);
    }
    values
}

#[test]
fn shuffled_input_order_never_changes_selection() {
    let candidates = vec![
        candidate(
            7,
            AttributionKind::AtuinCwdWindow,
            "a",
            EvidenceTrustLevel::Claimed,
            0.85,
            1,
        ),
        candidate(
            3,
            AttributionKind::HookCwd,
            "b",
            EvidenceTrustLevel::Verified,
            1.0,
            1,
        ),
        candidate(
            5,
            AttributionKind::OtlpSessionPath,
            "c",
            EvidenceTrustLevel::Verified,
            0.98,
            2,
        ),
        candidate(
            3,
            AttributionKind::TimestampProximity,
            "d",
            EvidenceTrustLevel::Inferred,
            0.5,
            3,
        ),
    ];
    let expected = ranked_ids(&candidates);
    assert_eq!(expected, vec![3, 5, 7]);
    for seed in 0..128 {
        assert_eq!(ranked_ids(&shuffled(candidates.clone(), seed)), expected);
    }
}

#[test]
fn invalid_candidates_are_rejected_before_sorting() {
    let mut invalid_id =
        AttributionCandidate::with_defaults(0, AttributionKind::HookCwd, "source", base_time());
    assert_eq!(
        select_primary_worktree(&[invalid_id.clone()]),
        Err(AttributionError::InvalidWorktreeId)
    );

    invalid_id.worktree_id = 1;
    invalid_id.source = "   ".to_string();
    assert_eq!(
        select_primary_worktree(&[invalid_id.clone()]),
        Err(AttributionError::EmptySource)
    );

    invalid_id.source = "source".to_string();
    invalid_id.confidence = f64::NAN;
    assert_eq!(
        select_primary_worktree(&[invalid_id.clone()]),
        Err(AttributionError::NonFiniteConfidence)
    );

    invalid_id.confidence = 1.1;
    assert_eq!(
        select_primary_worktree(&[invalid_id]),
        Err(AttributionError::ConfidenceOutOfRange)
    );

    let timestamp_too_strong = candidate(
        1,
        AttributionKind::TimestampProximity,
        "time",
        EvidenceTrustLevel::Inferred,
        0.51,
        1,
    );
    assert_eq!(
        select_primary_worktree(&[timestamp_too_strong]),
        Err(AttributionError::ConfidenceExceedsKindMaximum)
    );
}
