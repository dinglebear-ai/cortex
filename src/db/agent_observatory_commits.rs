//! Transactional exact Git commit persistence.

use super::GitCommitRow;
#[cfg(test)]
use crate::db::pool::{DbPool, write_lock};
use anyhow::{Context, Result, bail};
#[cfg(test)]
use rusqlite::TransactionBehavior;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde_json::Value;
use std::collections::HashSet;

const COMMIT_COLUMNS: &str =
    "id, repository_id, sha, parent_shas_json, author_name, author_email_hash,
     authored_at, committed_at, subject, changed_files, insertions, deletions,
     changed_paths_json, first_observed_at, last_observed_at, reachable, metadata_json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommitReachabilityUpdate {
    pub sha: String,
    pub reachable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommitUpsert {
    pub sha: String,
    pub parent_shas_json: String,
    pub author_name: Option<String>,
    pub author_email_hash: Option<String>,
    pub authored_at: Option<String>,
    pub committed_at: Option<String>,
    pub subject: String,
    pub changed_files: Option<i64>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_paths_json: String,
    pub reachable: bool,
    pub metadata_json: String,
}

fn commit_row(row: &Row<'_>) -> rusqlite::Result<GitCommitRow> {
    Ok(GitCommitRow {
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
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_json(value: &str, field: &str) -> Result<Value> {
    serde_json::from_str(value).with_context(|| format!("{field} must be valid JSON"))
}

fn validate_timestamp(value: Option<&str>, field: &str) -> Result<()> {
    if let Some(value) = value {
        chrono::DateTime::parse_from_rfc3339(value)
            .with_context(|| format!("invalid {field}: {value}"))?;
    }
    Ok(())
}

fn validate_commit(input: &GitCommitUpsert) -> Result<()> {
    if !valid_object_id(&input.sha) {
        bail!("sha must be a 40- or 64-byte hex object ID");
    }
    let parents = parse_json(&input.parent_shas_json, "parent_shas_json")?;
    let Value::Array(parents) = parents else {
        bail!("parent_shas_json must be a JSON array");
    };
    for parent in parents {
        let Some(parent) = parent.as_str() else {
            bail!("parent_shas_json entries must be strings");
        };
        if !valid_object_id(parent) {
            bail!("parent_shas_json contains an invalid object ID");
        }
    }
    let paths = parse_json(&input.changed_paths_json, "changed_paths_json")?;
    if !paths.is_array() {
        bail!("changed_paths_json must be a JSON array");
    }
    let metadata = parse_json(&input.metadata_json, "metadata_json")?;
    if !metadata.is_object() {
        bail!("metadata_json must be a JSON object");
    }
    validate_timestamp(input.authored_at.as_deref(), "authored_at")?;
    validate_timestamp(input.committed_at.as_deref(), "committed_at")?;
    for (field, value) in [
        ("changed_files", input.changed_files),
        ("insertions", input.insertions),
        ("deletions", input.deletions),
    ] {
        if value.is_some_and(|value| value < 0) {
            bail!("{field} must be non-negative");
        }
    }
    if input
        .author_email_hash
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("author_email_hash must be non-empty when present");
    }
    Ok(())
}

fn validate_observed_at(observed_at: &str) -> Result<()> {
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid observed_at: {observed_at}"))?;
    Ok(())
}

fn repository_id(conn: &Connection, repository_key: &str) -> Result<i64> {
    if repository_key.trim().is_empty() {
        bail!("repository_key must be non-empty");
    }
    conn.query_row(
        "SELECT id FROM repositories WHERE repository_key = ?1",
        [repository_key],
        |row| row.get(0),
    )
    .optional()?
    .with_context(|| format!("repository not found for key {repository_key}"))
}

fn commit_by_sha(conn: &Connection, repository_id: i64, sha: &str) -> Result<Option<GitCommitRow>> {
    let sql = format!(
        "SELECT {COMMIT_COLUMNS} FROM git_commits
          WHERE repository_id = ?1 AND sha = ?2"
    );
    conn.query_row(&sql, params![repository_id, sha], commit_row)
        .optional()
        .context("query Git commit by SHA")
}

#[cfg(test)]
pub fn reconcile_git_commits(
    pool: &DbPool,
    repository_key: &str,
    commits: &[GitCommitUpsert],
    reachability: &[GitCommitReachabilityUpdate],
    observed_at: &str,
) -> Result<Vec<GitCommitRow>> {
    validate_reconcile_git_commits(commits, reachability, observed_at)?;
    if commits.is_empty() && reachability.is_empty() {
        return Ok(Vec::new());
    }
    let _write_guard = write_lock();
    let mut conn = pool.get().context("acquire database connection")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows = reconcile_git_commits_tx(&tx, repository_key, commits, reachability, observed_at)?;
    tx.commit()?;
    Ok(rows)
}

pub(super) fn validate_reconcile_git_commits(
    commits: &[GitCommitUpsert],
    reachability: &[GitCommitReachabilityUpdate],
    observed_at: &str,
) -> Result<()> {
    validate_observed_at(observed_at)?;
    let mut shas = HashSet::new();
    for commit in commits {
        validate_commit(commit)?;
        if !shas.insert(commit.sha.as_str()) {
            bail!("duplicate commit SHA in batch");
        }
    }
    let mut update_shas = HashSet::new();
    for update in reachability {
        if !valid_object_id(&update.sha) {
            bail!("reachability SHA must be a 40- or 64-byte hex object ID");
        }
        if !update_shas.insert(update.sha.as_str()) {
            bail!("duplicate reachability SHA in batch");
        }
    }
    Ok(())
}

pub(super) fn reconcile_git_commits_tx(
    tx: &Transaction<'_>,
    repository_key: &str,
    commits: &[GitCommitUpsert],
    reachability: &[GitCommitReachabilityUpdate],
    observed_at: &str,
) -> Result<Vec<GitCommitRow>> {
    let repository_id = repository_id(tx, repository_key)?;
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        tx.execute(
            "INSERT INTO git_commits
                (repository_id, sha, parent_shas_json, author_name, author_email_hash,
                 authored_at, committed_at, subject, changed_files, insertions, deletions,
                 changed_paths_json, first_observed_at, last_observed_at, reachable,
                 metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13,
                     ?14, ?15)
             ON CONFLICT(repository_id, sha) DO UPDATE SET
                 parent_shas_json = excluded.parent_shas_json,
                 author_name = COALESCE(excluded.author_name, git_commits.author_name),
                 author_email_hash = COALESCE(
                     excluded.author_email_hash, git_commits.author_email_hash
                 ),
                 authored_at = COALESCE(excluded.authored_at, git_commits.authored_at),
                 committed_at = COALESCE(excluded.committed_at, git_commits.committed_at),
                 subject = excluded.subject,
                 changed_files = excluded.changed_files,
                 insertions = excluded.insertions,
                 deletions = excluded.deletions,
                 changed_paths_json = excluded.changed_paths_json,
                 last_observed_at = excluded.last_observed_at,
                 reachable = excluded.reachable,
                 metadata_json = excluded.metadata_json",
            params![
                repository_id,
                commit.sha,
                commit.parent_shas_json,
                commit.author_name,
                commit.author_email_hash,
                commit.authored_at,
                commit.committed_at,
                commit.subject,
                commit.changed_files,
                commit.insertions,
                commit.deletions,
                commit.changed_paths_json,
                observed_at,
                commit.reachable,
                commit.metadata_json,
            ],
        )?;
        rows.push(
            commit_by_sha(tx, repository_id, &commit.sha)?
                .context("commit missing after upsert")?,
        );
    }
    for update in reachability {
        let changed = tx.execute(
            "UPDATE git_commits
                SET reachable = ?3, last_observed_at = ?4
              WHERE repository_id = ?1 AND sha = ?2",
            params![repository_id, update.sha, update.reachable, observed_at],
        )?;
        if changed != 1 {
            bail!(
                "Git commit not found for reachability update: {}",
                update.sha
            );
        }
    }
    Ok(rows)
}

#[cfg(test)]
pub fn upsert_git_commits(
    pool: &DbPool,
    repository_key: &str,
    commits: &[GitCommitUpsert],
    observed_at: &str,
) -> Result<Vec<GitCommitRow>> {
    reconcile_git_commits(pool, repository_key, commits, &[], observed_at)
}

#[cfg(test)]
pub fn get_git_commit(
    pool: &DbPool,
    repository_id: i64,
    sha: &str,
) -> Result<Option<GitCommitRow>> {
    if repository_id <= 0 {
        bail!("repository_id must be positive");
    }
    if !valid_object_id(sha) {
        bail!("sha must be a 40- or 64-byte hex object ID");
    }
    let conn = pool.get().context("acquire database connection")?;
    commit_by_sha(&conn, repository_id, sha)
}

#[cfg(test)]
pub fn list_git_commits(pool: &DbPool, repository_id: i64) -> Result<Vec<GitCommitRow>> {
    if repository_id <= 0 {
        bail!("repository_id must be positive");
    }
    let conn = pool.get().context("acquire database connection")?;
    let sql = format!(
        "SELECT {COMMIT_COLUMNS} FROM git_commits
          WHERE repository_id = ?1 ORDER BY id"
    );
    conn.prepare(&sql)?
        .query_map([repository_id], commit_row)?
        .collect::<rusqlite::Result<_>>()
        .context("list Git commits")
}

#[cfg(test)]
#[path = "agent_observatory_commits_tests.rs"]
mod tests;
