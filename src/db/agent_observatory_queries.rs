//! Transactional repository and worktree persistence for Agent Observatory.

use super::{RepositoryRow, RepositoryWorktreeRow};
use crate::db::pool::DbPool;
#[cfg(test)]
use crate::db::pool::write_lock;
use anyhow::{Context, Result, bail};
#[cfg(test)]
use rusqlite::TransactionBehavior;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use std::collections::HashSet;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryUpsert {
    pub repository_key: String,
    pub hostname: String,
    pub common_git_dir: String,
    pub primary_path: String,
    pub display_name: String,
    pub remote_url_hash: Option<String>,
    pub metadata_json: String,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryWorktreeUpsert {
    pub worktree_key: String,
    pub hostname: String,
    pub path: String,
    pub git_dir: String,
    pub branch_ref: Option<String>,
    pub branch_name: Option<String>,
    pub head_sha: Option<String>,
    pub upstream_ref: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
    pub dirty: bool,
    pub staged_count: i64,
    pub unstaged_count: i64,
    pub untracked_count: i64,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub status_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryReconcileResult {
    pub repository: RepositoryRow,
    pub worktrees: Vec<RepositoryWorktreeRow>,
    pub removed_worktree_ids: Vec<i64>,
}

const REPOSITORY_BY_KEY_SQL: &str =
    "SELECT id, repository_key, hostname, common_git_dir, primary_path, display_name,
            remote_url_hash, first_seen_at, last_seen_at, removed_at, metadata_json,
            created_at, updated_at
       FROM repositories WHERE repository_key = ?1";

const WORKTREE_BY_KEY_SQL: &str =
    "SELECT id, worktree_key, repository_id, hostname, path, git_dir, branch_ref,
            branch_name, head_sha, upstream_ref, detached, bare, locked, lock_reason,
            prunable, prune_reason, dirty, staged_count, unstaged_count, untracked_count,
            ahead, behind, status_hash, first_seen_at, last_seen_at, removed_at,
            created_at, updated_at
       FROM repository_worktrees WHERE worktree_key = ?1";

const WORKTREE_LIST_ALL_SQL: &str =
    "SELECT id, worktree_key, repository_id, hostname, path, git_dir, branch_ref,
            branch_name, head_sha, upstream_ref, detached, bare, locked, lock_reason,
            prunable, prune_reason, dirty, staged_count, unstaged_count, untracked_count,
            ahead, behind, status_hash, first_seen_at, last_seen_at, removed_at,
            created_at, updated_at
       FROM repository_worktrees
      WHERE repository_id = ?1
      ORDER BY path, id";

const WORKTREE_LIST_ACTIVE_SQL: &str =
    "SELECT id, worktree_key, repository_id, hostname, path, git_dir, branch_ref,
            branch_name, head_sha, upstream_ref, detached, bare, locked, lock_reason,
            prunable, prune_reason, dirty, staged_count, unstaged_count, untracked_count,
            ahead, behind, status_hash, first_seen_at, last_seen_at, removed_at,
            created_at, updated_at
       FROM repository_worktrees
      WHERE repository_id = ?1 AND removed_at IS NULL
      ORDER BY path, id";

fn repository_row(row: &Row<'_>) -> rusqlite::Result<RepositoryRow> {
    Ok(RepositoryRow {
        id: row.get(0)?,
        repository_key: row.get(1)?,
        hostname: row.get(2)?,
        common_git_dir: row.get(3)?,
        primary_path: row.get(4)?,
        display_name: row.get(5)?,
        remote_url_hash: row.get(6)?,
        first_seen_at: row.get(7)?,
        last_seen_at: row.get(8)?,
        removed_at: row.get(9)?,
        metadata_json: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn worktree_row(row: &Row<'_>) -> rusqlite::Result<RepositoryWorktreeRow> {
    Ok(RepositoryWorktreeRow {
        id: row.get(0)?,
        worktree_key: row.get(1)?,
        repository_id: row.get(2)?,
        hostname: row.get(3)?,
        path: row.get(4)?,
        git_dir: row.get(5)?,
        branch_ref: row.get(6)?,
        branch_name: row.get(7)?,
        head_sha: row.get(8)?,
        upstream_ref: row.get(9)?,
        detached: row.get(10)?,
        bare: row.get(11)?,
        locked: row.get(12)?,
        lock_reason: row.get(13)?,
        prunable: row.get(14)?,
        prune_reason: row.get(15)?,
        dirty: row.get(16)?,
        staged_count: row.get(17)?,
        unstaged_count: row.get(18)?,
        untracked_count: row.get(19)?,
        ahead: row.get(20)?,
        behind: row.get(21)?,
        status_hash: row.get(22)?,
        first_seen_at: row.get(23)?,
        last_seen_at: row.get(24)?,
        removed_at: row.get(25)?,
        created_at: row.get(26)?,
        updated_at: row.get(27)?,
    })
}

fn required(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must be non-empty");
    }
    Ok(())
}

fn canonical_absolute_path(value: &str, field: &str) -> Result<()> {
    required(value, field)?;
    let path = Path::new(value);
    if !path.is_absolute() {
        bail!("{field} must be an absolute canonical path");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!("{field} must not contain dot path components");
    }
    Ok(())
}

fn validate_repository(input: &RepositoryUpsert, observed_at: &str) -> Result<()> {
    required(&input.repository_key, "repository_key")?;
    required(&input.hostname, "hostname")?;
    required(&input.display_name, "display_name")?;
    required(observed_at, "observed_at")?;
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid observed_at: {observed_at}"))?;
    canonical_absolute_path(&input.common_git_dir, "common_git_dir")?;
    canonical_absolute_path(&input.primary_path, "primary_path")?;
    serde_json::from_str::<serde_json::Value>(&input.metadata_json)
        .context("metadata_json must be valid JSON")?;
    Ok(())
}

fn validate_worktree(
    repository: &RepositoryUpsert,
    input: &RepositoryWorktreeUpsert,
) -> Result<()> {
    required(&input.worktree_key, "worktree_key")?;
    required(&input.hostname, "worktree hostname")?;
    if input.hostname != repository.hostname {
        bail!("worktree hostname must match repository hostname");
    }
    canonical_absolute_path(&input.path, "worktree path")?;
    canonical_absolute_path(&input.git_dir, "worktree git_dir")?;
    if input.staged_count < 0 || input.unstaged_count < 0 || input.untracked_count < 0 {
        bail!("worktree change counts must be non-negative");
    }
    if input.ahead.is_some_and(|value| value < 0) || input.behind.is_some_and(|value| value < 0) {
        bail!("worktree ahead/behind counts must be non-negative");
    }
    Ok(())
}

fn repository_by_key(conn: &Connection, repository_key: &str) -> Result<Option<RepositoryRow>> {
    conn.query_row(REPOSITORY_BY_KEY_SQL, [repository_key], repository_row)
        .optional()
        .context("query repository by key")
}

fn worktree_by_key(conn: &Connection, worktree_key: &str) -> Result<Option<RepositoryWorktreeRow>> {
    conn.query_row(WORKTREE_BY_KEY_SQL, [worktree_key], worktree_row)
        .optional()
        .context("query worktree by key")
}

fn list_worktrees(
    conn: &Connection,
    repository_id: i64,
    include_removed: bool,
) -> Result<Vec<RepositoryWorktreeRow>> {
    let sql = if include_removed {
        WORKTREE_LIST_ALL_SQL
    } else {
        WORKTREE_LIST_ACTIVE_SQL
    };
    conn.prepare(sql)?
        .query_map([repository_id], worktree_row)?
        .collect::<rusqlite::Result<_>>()
        .context("list repository worktrees")
}

fn upsert_repository_tx(
    tx: &Transaction<'_>,
    input: &RepositoryUpsert,
    observed_at: &str,
) -> Result<RepositoryRow> {
    let identity: Option<(String, String)> = tx
        .query_row(
            "SELECT hostname, common_git_dir FROM repositories WHERE repository_key = ?1",
            [&input.repository_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((hostname, common_git_dir)) = identity
        && (hostname != input.hostname || common_git_dir != input.common_git_dir)
    {
        bail!("repository identity fields cannot change");
    }

    let conflicting_key: Option<String> = tx
        .query_row(
            "SELECT repository_key FROM repositories
              WHERE hostname = ?1 AND common_git_dir = ?2 AND repository_key <> ?3",
            params![input.hostname, input.common_git_dir, input.repository_key],
            |row| row.get(0),
        )
        .optional()?;
    if conflicting_key.is_some() {
        bail!("hostname/common_git_dir already belongs to another repository key");
    }

    tx.execute(
        "INSERT INTO repositories
            (repository_key, hostname, common_git_dir, primary_path, display_name,
             remote_url_hash, first_seen_at, last_seen_at, removed_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, NULL, ?8)
         ON CONFLICT(repository_key) DO UPDATE SET
             primary_path = excluded.primary_path,
             display_name = excluded.display_name,
             remote_url_hash = excluded.remote_url_hash,
             last_seen_at = excluded.last_seen_at,
             removed_at = NULL,
             metadata_json = excluded.metadata_json,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            input.repository_key,
            input.hostname,
            input.common_git_dir,
            input.primary_path,
            input.display_name,
            input.remote_url_hash,
            observed_at,
            input.metadata_json,
        ],
    )?;

    repository_by_key(tx, &input.repository_key)?.context("repository missing after upsert")
}

fn upsert_worktree_tx(
    tx: &Transaction<'_>,
    repository_id: i64,
    input: &RepositoryWorktreeUpsert,
    observed_at: &str,
) -> Result<RepositoryWorktreeRow> {
    let identity: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT repository_id, hostname, path
               FROM repository_worktrees WHERE worktree_key = ?1",
            [&input.worktree_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((existing_repository_id, hostname, path)) = identity
        && (existing_repository_id != repository_id
            || hostname != input.hostname
            || path != input.path)
    {
        bail!("worktree identity fields cannot change");
    }

    let conflicting_key: Option<String> = tx
        .query_row(
            "SELECT worktree_key FROM repository_worktrees
              WHERE hostname = ?1 AND path = ?2 AND worktree_key <> ?3",
            params![input.hostname, input.path, input.worktree_key],
            |row| row.get(0),
        )
        .optional()?;
    if conflicting_key.is_some() {
        bail!("hostname/path already belongs to another worktree key");
    }

    tx.execute(
        "INSERT INTO repository_worktrees
            (worktree_key, repository_id, hostname, path, git_dir, branch_ref, branch_name,
             head_sha, upstream_ref, detached, bare, locked, lock_reason, prunable,
             prune_reason, dirty, staged_count, unstaged_count, untracked_count, ahead,
             behind, status_hash, first_seen_at, last_seen_at, removed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?23, NULL)
         ON CONFLICT(worktree_key) DO UPDATE SET
             git_dir = excluded.git_dir,
             branch_ref = excluded.branch_ref,
             branch_name = excluded.branch_name,
             head_sha = excluded.head_sha,
             upstream_ref = excluded.upstream_ref,
             detached = excluded.detached,
             bare = excluded.bare,
             locked = excluded.locked,
             lock_reason = excluded.lock_reason,
             prunable = excluded.prunable,
             prune_reason = excluded.prune_reason,
             dirty = excluded.dirty,
             staged_count = excluded.staged_count,
             unstaged_count = excluded.unstaged_count,
             untracked_count = excluded.untracked_count,
             ahead = excluded.ahead,
             behind = excluded.behind,
             status_hash = excluded.status_hash,
             last_seen_at = excluded.last_seen_at,
             removed_at = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            input.worktree_key,
            repository_id,
            input.hostname,
            input.path,
            input.git_dir,
            input.branch_ref,
            input.branch_name,
            input.head_sha,
            input.upstream_ref,
            input.detached,
            input.bare,
            input.locked,
            input.lock_reason,
            input.prunable,
            input.prune_reason,
            input.dirty,
            input.staged_count,
            input.unstaged_count,
            input.untracked_count,
            input.ahead,
            input.behind,
            input.status_hash,
            observed_at,
        ],
    )?;

    worktree_by_key(tx, &input.worktree_key)?.context("worktree missing after upsert")
}

#[cfg(test)]
pub fn reconcile_repository(
    pool: &DbPool,
    repository: &RepositoryUpsert,
    worktrees: &[RepositoryWorktreeUpsert],
    observed_at: &str,
) -> Result<RepositoryReconcileResult> {
    validate_reconcile_repository(repository, worktrees, observed_at)?;
    let _write_guard = write_lock();
    let mut conn = pool.get().context("acquire database connection")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = reconcile_repository_tx(&tx, repository, worktrees, observed_at)?;
    tx.commit()?;
    Ok(result)
}

pub(super) fn validate_reconcile_repository(
    repository: &RepositoryUpsert,
    worktrees: &[RepositoryWorktreeUpsert],
    observed_at: &str,
) -> Result<()> {
    validate_repository(repository, observed_at)?;
    let mut keys = HashSet::new();
    let mut paths = HashSet::new();
    for worktree in worktrees {
        validate_worktree(repository, worktree)?;
        if !keys.insert(worktree.worktree_key.as_str()) {
            bail!("duplicate worktree key in reconciliation");
        }
        if !paths.insert((worktree.hostname.as_str(), worktree.path.as_str())) {
            bail!("duplicate hostname/path in reconciliation");
        }
    }

    Ok(())
}

pub(super) fn reconcile_repository_tx(
    tx: &Transaction<'_>,
    repository: &RepositoryUpsert,
    worktrees: &[RepositoryWorktreeUpsert],
    observed_at: &str,
) -> Result<RepositoryReconcileResult> {
    let keys = worktrees
        .iter()
        .map(|worktree| worktree.worktree_key.as_str())
        .collect::<HashSet<_>>();
    let repository_row = upsert_repository_tx(tx, repository, observed_at)?;

    let mut active_worktrees = Vec::with_capacity(worktrees.len());
    for worktree in worktrees {
        active_worktrees.push(upsert_worktree_tx(
            tx,
            repository_row.id,
            worktree,
            observed_at,
        )?);
    }

    let existing_active: Vec<(i64, String)> = tx
        .prepare(
            "SELECT id, worktree_key FROM repository_worktrees
              WHERE repository_id = ?1 AND removed_at IS NULL
              ORDER BY id",
        )?
        .query_map([repository_row.id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut removed_worktree_ids = Vec::new();
    for (id, key) in existing_active {
        if !keys.contains(key.as_str()) {
            tx.execute(
                "UPDATE repository_worktrees
                    SET removed_at = COALESCE(removed_at, ?2),
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE id = ?1",
                params![id, observed_at],
            )?;
            removed_worktree_ids.push(id);
        }
    }

    active_worktrees.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
    Ok(RepositoryReconcileResult {
        repository: repository_row,
        worktrees: active_worktrees,
        removed_worktree_ids,
    })
}

pub fn get_repository_by_key(pool: &DbPool, repository_key: &str) -> Result<Option<RepositoryRow>> {
    let conn = pool.get().context("acquire database connection")?;
    repository_by_key(&conn, repository_key)
}

#[cfg(test)]
pub fn get_worktree_by_key(
    pool: &DbPool,
    worktree_key: &str,
) -> Result<Option<RepositoryWorktreeRow>> {
    let conn = pool.get().context("acquire database connection")?;
    worktree_by_key(&conn, worktree_key)
}

pub fn list_repository_worktrees(
    pool: &DbPool,
    repository_id: i64,
    include_removed: bool,
) -> Result<Vec<RepositoryWorktreeRow>> {
    let conn = pool.get().context("acquire database connection")?;
    list_worktrees(&conn, repository_id, include_removed)
}

#[cfg(test)]
pub fn mark_repository_removed(
    pool: &DbPool,
    repository_key: &str,
    removed_at: &str,
) -> Result<bool> {
    required(repository_key, "repository_key")?;
    required(removed_at, "removed_at")?;
    chrono::DateTime::parse_from_rfc3339(removed_at)
        .with_context(|| format!("invalid removed_at: {removed_at}"))?;
    let _write_guard = write_lock();
    let conn = pool.get().context("acquire database connection")?;
    Ok(conn.execute(
        "UPDATE repositories
            SET removed_at = COALESCE(removed_at, ?2),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE repository_key = ?1",
        params![repository_key, removed_at],
    )? > 0)
}

#[cfg(test)]
pub fn mark_worktree_removed(pool: &DbPool, worktree_key: &str, removed_at: &str) -> Result<bool> {
    required(worktree_key, "worktree_key")?;
    required(removed_at, "removed_at")?;
    chrono::DateTime::parse_from_rfc3339(removed_at)
        .with_context(|| format!("invalid removed_at: {removed_at}"))?;
    let _write_guard = write_lock();
    let conn = pool.get().context("acquire database connection")?;
    Ok(conn.execute(
        "UPDATE repository_worktrees
            SET removed_at = COALESCE(removed_at, ?2),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE worktree_key = ?1",
        params![worktree_key, removed_at],
    )? > 0)
}

#[cfg(test)]
#[path = "agent_observatory_queries_tests.rs"]
mod tests;
