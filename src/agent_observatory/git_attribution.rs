//! Exact Git commit attribution for Agent Observatory runs.

use super::attribution::{AttributionCandidate, AttributionKind, select_primary_worktree};
use crate::db::DbPool;
use crate::db::agent_observatory::{
    AgentRunCommitUpsert, EvidenceTrustLevel, GitCommitRow, commit_attribution_evidence,
    upsert_agent_run_commit,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::collections::BTreeMap;

const ACTIVE_COMMIT_WINDOW_SECS: i64 = 300;

#[derive(Debug, Clone)]
struct RunAttribution {
    run_id: i64,
    kind: AttributionKind,
    source: String,
    trust: EvidenceTrustLevel,
    confidence: f64,
}

fn direct_actor_evidence(kind: AttributionKind) -> bool {
    matches!(
        kind,
        AttributionKind::HookCwd
            | AttributionKind::OtlpSessionPath
            | AttributionKind::AgentCommandCwd
            | AttributionKind::LifecycleHostProcess
    )
}

fn parse_timestamp(value: &str, label: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid {label}: {value}"))?
        .with_timezone(&Utc))
}

fn active_at(
    observed_at: DateTime<Utc>,
    started_at: &str,
    last_activity_at: &str,
    ended_at: Option<&str>,
    evidence_first_seen_at: &str,
    evidence_last_seen_at: &str,
) -> Result<bool> {
    let started = parse_timestamp(started_at, "run started_at")?;
    if started > observed_at {
        return Ok(false);
    }
    if let Some(ended_at) = ended_at
        && parse_timestamp(ended_at, "run ended_at")? < observed_at
    {
        return Ok(false);
    }

    let mut latest = started;
    for (value, label) in [
        (last_activity_at, "run last_activity_at"),
        (evidence_first_seen_at, "evidence first_seen_at"),
        (evidence_last_seen_at, "evidence last_seen_at"),
    ] {
        let timestamp = parse_timestamp(value, label)?;
        if timestamp <= observed_at && timestamp > latest {
            latest = timestamp;
        }
    }
    Ok(observed_at.signed_duration_since(latest) <= Duration::seconds(ACTIVE_COMMIT_WINDOW_SECS))
}

fn run_attributions(
    pool: &DbPool,
    worktree_id: i64,
    observed_at: &str,
) -> Result<Vec<RunAttribution>> {
    let observed = DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid commit observation time: {observed_at}"))?
        .with_timezone(&Utc);
    let evidence = commit_attribution_evidence(pool, worktree_id, observed_at)?;
    let mut grouped = BTreeMap::<i64, Vec<_>>::new();
    for row in evidence {
        if !active_at(
            observed,
            &row.started_at,
            &row.last_activity_at,
            row.ended_at.as_deref(),
            &row.first_seen_at,
            &row.last_seen_at,
        )? {
            continue;
        }
        grouped.entry(row.run_id).or_default().push(row);
    }

    let mut attributions = Vec::new();
    for (run_id, rows) in grouped {
        let mut candidates = Vec::new();
        for row in rows {
            let Some(kind) = AttributionKind::from_db(&row.evidence_kind) else {
                continue;
            };
            let evidence_last = parse_timestamp(&row.last_seen_at, "evidence last_seen_at")?;
            let evidence_at = if evidence_last <= observed {
                evidence_last
            } else {
                parse_timestamp(&row.first_seen_at, "evidence first_seen_at")?
            };
            candidates.push(AttributionCandidate {
                worktree_id,
                kind,
                source: row.evidence_source,
                trust: row.trust_level,
                confidence: row.confidence,
                observed_at: evidence_at,
            });
        }
        let selection = select_primary_worktree(&candidates)?;
        if selection.primary_worktree_id != Some(worktree_id) {
            continue;
        }
        let Some(strongest) = selection.ranked.first() else {
            continue;
        };
        attributions.push(RunAttribution {
            run_id,
            kind: strongest.kind,
            source: strongest.source.clone(),
            trust: strongest.trust,
            confidence: strongest.confidence,
        });
    }
    Ok(attributions)
}

/// Attach an exact set of commits from one durable HEAD observation to all
/// runs that were active in that worktree at the observation instant.
pub fn attribute_exact_commits(
    pool: &DbPool,
    worktree_id: i64,
    observation_key: &str,
    observed_at: &str,
    old_head_sha: Option<&str>,
    new_head_sha: &str,
    commits: &[GitCommitRow],
) -> Result<usize> {
    if observation_key.trim().is_empty() || new_head_sha.trim().is_empty() {
        anyhow::bail!("HEAD observation key and new SHA must be non-empty");
    }
    let attributions = run_attributions(pool, worktree_id, observed_at)?;
    let ambiguous = attributions.len() > 1;
    let direct_count = attributions
        .iter()
        .filter(|attribution| direct_actor_evidence(attribution.kind))
        .count();
    let mut written = 0usize;
    for attribution in attributions {
        for commit in commits {
            let direct =
                direct_actor_evidence(attribution.kind) && (!ambiguous || direct_count == 1);
            let (trust_level, confidence) = if direct {
                (attribution.trust, attribution.confidence)
            } else {
                (
                    EvidenceTrustLevel::Correlated,
                    attribution.confidence.min(0.75),
                )
            };
            upsert_agent_run_commit(
                pool,
                &AgentRunCommitUpsert {
                    run_id: attribution.run_id,
                    commit_id: commit.id,
                    worktree_id: Some(worktree_id),
                    evidence_kind: if direct {
                        attribution.kind.as_str().to_string()
                    } else {
                        "git_head_overlap".to_string()
                    },
                    evidence_source: format!("{observation_key}:{}", attribution.source),
                    trust_level,
                    confidence,
                    observed_at: observed_at.to_string(),
                    metadata_json: json!({
                        "ambiguous_active_runs": ambiguous,
                        "commit_sha": commit.sha,
                        "new_head_sha": new_head_sha,
                        "old_head_sha": old_head_sha,
                        "source_evidence_kind": attribution.kind.as_str(),
                    })
                    .to_string(),
                },
            )?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
#[path = "git_attribution_tests.rs"]
mod tests;
