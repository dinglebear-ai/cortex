//! Durable Agent Observatory run-to-commit attribution relations.

use super::EvidenceTrustLevel;
use crate::db::pool::DbPool;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const ROW_COLUMNS: &str =
    "id, relation_key, run_id, commit_id, worktree_id, evidence_kind, evidence_source,
     trust_level, confidence, first_seen_at, last_seen_at, metadata_json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunCommitRow {
    pub id: i64,
    pub relation_key: String,
    pub run_id: i64,
    pub commit_id: i64,
    pub worktree_id: Option<i64>,
    pub evidence_kind: String,
    pub evidence_source: String,
    pub trust_level: EvidenceTrustLevel,
    pub confidence: f64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunCommitUpsert {
    pub run_id: i64,
    pub commit_id: i64,
    pub worktree_id: Option<i64>,
    pub evidence_kind: String,
    pub evidence_source: String,
    pub trust_level: EvidenceTrustLevel,
    pub confidence: f64,
    pub observed_at: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentCommitAttributionEvidence {
    pub run_id: i64,
    pub run_key: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub ended_at: Option<String>,
    pub evidence_kind: String,
    pub evidence_source: String,
    pub trust_level: EvidenceTrustLevel,
    pub confidence: f64,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

fn row(row: &Row<'_>) -> rusqlite::Result<AgentRunCommitRow> {
    let trust: String = row.get(7)?;
    let trust_level = EvidenceTrustLevel::from_str(&trust).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(AgentRunCommitRow {
        id: row.get(0)?,
        relation_key: row.get(1)?,
        run_id: row.get(2)?,
        commit_id: row.get(3)?,
        worktree_id: row.get(4)?,
        evidence_kind: row.get(5)?,
        evidence_source: row.get(6)?,
        trust_level,
        confidence: row.get(8)?,
        first_seen_at: row.get(9)?,
        last_seen_at: row.get(10)?,
        metadata_json: row.get(11)?,
    })
}

fn hash_component(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn relation_key(run_key: &str, commit_sha: &str, kind: &str, source: &str) -> String {
    let mut hasher = Sha256::new();
    for value in [run_key, commit_sha, kind, source] {
        hash_component(&mut hasher, value);
    }
    format!("v1:run_commit:{:x}", hasher.finalize())
}

fn validate(input: &AgentRunCommitUpsert) -> Result<()> {
    if input.run_id <= 0 || input.commit_id <= 0 {
        bail!("run_id and commit_id must be positive");
    }
    if input.worktree_id.is_some_and(|id| id <= 0) {
        bail!("worktree_id must be positive when present");
    }
    if input.evidence_kind.trim().is_empty() || input.evidence_source.trim().is_empty() {
        bail!("commit evidence kind/source must be non-empty");
    }
    if !input.confidence.is_finite() || !(0.0..=1.0).contains(&input.confidence) {
        bail!("commit confidence must be between 0 and 1");
    }
    chrono::DateTime::parse_from_rfc3339(&input.observed_at)
        .with_context(|| format!("invalid commit observed_at: {}", input.observed_at))?;
    let metadata: Value = serde_json::from_str(&input.metadata_json)
        .context("commit attribution metadata_json must be valid JSON")?;
    if !metadata.is_object() {
        bail!("commit attribution metadata_json must be an object");
    }
    Ok(())
}

pub fn upsert_agent_run_commit(
    pool: &DbPool,
    input: &AgentRunCommitUpsert,
) -> Result<AgentRunCommitRow> {
    validate(input)?;
    let mut connection = crate::db::write_conn(pool).context("acquire database connection")?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (run_key, commit_sha, commit_repository_id): (String, String, i64) = tx
        .query_row(
            "SELECT r.run_key, c.sha, c.repository_id FROM agent_runs r CROSS JOIN git_commits c
          WHERE r.id = ?1 AND c.id = ?2",
            params![input.run_id, input.commit_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .context("resolve run/commit identity")?;
    if let Some(worktree_id) = input.worktree_id {
        let worktree_repository_id = tx
            .query_row(
                "SELECT repository_id FROM repository_worktrees WHERE id = ?1",
                [worktree_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .context("worktree not found for commit attribution")?;
        if worktree_repository_id != commit_repository_id {
            bail!("worktree and commit repositories differ for commit attribution");
        }
    }
    let key = relation_key(
        &run_key,
        &commit_sha,
        &input.evidence_kind,
        &input.evidence_source,
    );
    tx.execute(
        "INSERT INTO agent_run_commits
            (relation_key, run_id, commit_id, worktree_id, evidence_kind, evidence_source,
             trust_level, confidence, first_seen_at, last_seen_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)
         ON CONFLICT(run_id, commit_id, evidence_kind, evidence_source) DO UPDATE SET
             worktree_id = COALESCE(agent_run_commits.worktree_id, excluded.worktree_id),
             trust_level = CASE WHEN excluded.confidence > agent_run_commits.confidence
                                THEN excluded.trust_level ELSE agent_run_commits.trust_level END,
             confidence = MAX(agent_run_commits.confidence, excluded.confidence),
             first_seen_at = MIN(agent_run_commits.first_seen_at, excluded.first_seen_at),
             last_seen_at = MAX(agent_run_commits.last_seen_at, excluded.last_seen_at),
             metadata_json = CASE WHEN excluded.last_seen_at >= agent_run_commits.last_seen_at
                                  THEN excluded.metadata_json ELSE agent_run_commits.metadata_json END",
        params![key, input.run_id, input.commit_id, input.worktree_id, input.evidence_kind,
            input.evidence_source, input.trust_level.as_str(), input.confidence,
            input.observed_at, input.metadata_json],
    )?;
    let relation = tx.query_row(
        &format!("SELECT {ROW_COLUMNS} FROM agent_run_commits
                  WHERE run_id = ?1 AND commit_id = ?2 AND evidence_kind = ?3 AND evidence_source = ?4"),
        params![input.run_id, input.commit_id, input.evidence_kind, input.evidence_source],
        row,
    )?;
    tx.commit()?;
    Ok(relation)
}

#[cfg(test)]
pub fn list_agent_run_commits(pool: &DbPool, run_id: i64) -> Result<Vec<AgentRunCommitRow>> {
    if run_id <= 0 {
        bail!("run_id must be positive");
    }
    let connection = pool.get().context("acquire database connection")?;
    let mut statement = connection.prepare(&format!(
        "SELECT {ROW_COLUMNS} FROM agent_run_commits WHERE run_id = ?1 ORDER BY first_seen_at, id"
    ))?;
    Ok(statement
        .query_map([run_id], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn commit_attribution_evidence(
    pool: &DbPool,
    worktree_id: i64,
    observed_at: &str,
) -> Result<Vec<AgentCommitAttributionEvidence>> {
    if worktree_id <= 0 {
        bail!("worktree_id must be positive");
    }
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid attribution observed_at: {observed_at}"))?;
    let connection = pool.get().context("acquire database connection")?;
    let mut statement = connection.prepare(
        "SELECT r.id, r.run_key, r.started_at, r.last_activity_at, r.ended_at,
                e.evidence_kind, e.evidence_source, e.trust_level, e.confidence,
                e.first_seen_at, e.last_seen_at
           FROM agent_runs r
           JOIN agent_run_worktrees e ON e.run_id = r.id
          WHERE e.worktree_id = ?1
            AND e.first_seen_at <= ?2
            AND r.started_at <= ?2
            AND (r.ended_at IS NULL OR r.ended_at >= ?2)
          ORDER BY r.id, e.confidence DESC, e.last_seen_at DESC, e.id",
    )?;
    let rows = statement
        .query_map(params![worktree_id, observed_at], |row| {
            let trust: String = row.get(7)?;
            let trust_level = EvidenceTrustLevel::from_str(&trust).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(AgentCommitAttributionEvidence {
                run_id: row.get(0)?,
                run_key: row.get(1)?,
                started_at: row.get(2)?,
                last_activity_at: row.get(3)?,
                ended_at: row.get(4)?,
                evidence_kind: row.get(5)?,
                evidence_source: row.get(6)?,
                trust_level,
                confidence: row.get(8)?,
                first_seen_at: row.get(9)?,
                last_seen_at: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn git_commit_by_repository_sha(
    pool: &DbPool,
    repository_id: i64,
    sha: &str,
) -> Result<Option<super::GitCommitRow>> {
    let connection = pool.get().context("acquire database connection")?;
    let row = connection
        .query_row(
            "SELECT id, repository_id, sha, parent_shas_json, author_name, author_email_hash,
                authored_at, committed_at, subject, changed_files, insertions, deletions,
                changed_paths_json, first_observed_at, last_observed_at, reachable, metadata_json
           FROM git_commits WHERE repository_id = ?1 AND sha = ?2",
            params![repository_id, sha],
            |row| {
                Ok(super::GitCommitRow {
                    id: row.get(0)?,
                    repository_id: row.get(1)?,
                    sha: row.get(2)?,
                    parent_shas_json: row.get(3)?,
                    author_name: row.get(4)?,
                    author_email_hash: row.get(5)?,
                    authored_at: row.get(6)?,
                    committed_at: row.get(7)?,
                    subject: row.get(8)?,
                    changed_files: row.get(9)?,
                    insertions: row.get(10)?,
                    deletions: row.get(11)?,
                    changed_paths_json: row.get(12)?,
                    first_observed_at: row.get(13)?,
                    last_observed_at: row.get(14)?,
                    reachable: row.get(15)?,
                    metadata_json: row.get(16)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}
