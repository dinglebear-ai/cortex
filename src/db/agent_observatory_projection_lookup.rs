//! Read-side lookups used by Agent Observatory source projectors.

use super::super::AgentRunRow;
use super::sql;
use crate::agent_observatory::identity::canonical_tool;
use crate::db::pool::DbPool;
use anyhow::{Context, Result, bail};
use rusqlite::params;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProjectionWorktreeRef {
    pub id: i64,
    pub worktree_key: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentProjectionRunMatch {
    None,
    Unique(Box<AgentRunRow>),
    Ambiguous,
}

fn canonical_absolute(path: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

pub fn find_active_projection_worktree(
    pool: &DbPool,
    hostname: &str,
    path: &str,
) -> Result<Option<AgentProjectionWorktreeRef>> {
    if hostname.trim().is_empty() {
        bail!("hostname must be non-empty");
    }
    if !canonical_absolute(path) {
        bail!("worktree path must be canonical and absolute");
    }
    let connection = pool.get().context("acquire database connection")?;
    let mut statement = connection.prepare(
        "SELECT id, worktree_key, path FROM repository_worktrees
          WHERE hostname = ?1
            AND removed_at IS NULL
            AND (
              path = ?2
              OR path = '/'
              OR (
                length(?2) > length(path)
                AND substr(?2, 1, length(path)) = path
                AND substr(?2, length(path) + 1, 1) = '/'
              )
            )
          ORDER BY length(path) DESC, id
          LIMIT 1",
    )?;
    let rows = statement
        .query_map(params![hostname.trim(), path], |row| {
            Ok(AgentProjectionWorktreeRef {
                id: row.get(0)?,
                worktree_key: row.get(1)?,
                path: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().next())
}

pub fn find_unique_overlapping_projection_run(
    pool: &DbPool,
    hostname: &str,
    observed_at: &str,
) -> Result<AgentProjectionRunMatch> {
    if hostname.trim().is_empty() {
        bail!("hostname must be non-empty");
    }
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid observed_at: {observed_at}"))?;
    let connection = pool.get().context("acquire database connection")?;
    let mut statement = connection.prepare(
        "SELECT id FROM agent_runs
          WHERE hostname = ?1
            AND started_at <= ?2
            AND last_activity_at >= ?2
            AND status IN ('starting','active','waiting','idle','stale')
          ORDER BY id LIMIT 2",
    )?;
    let ids = statement
        .query_map(params![hostname.trim(), observed_at], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match ids.as_slice() {
        [] => Ok(AgentProjectionRunMatch::None),
        [id] => Ok(AgentProjectionRunMatch::Unique(Box::new(sql::run_by_id(
            &connection,
            *id,
        )?))),
        _ => Ok(AgentProjectionRunMatch::Ambiguous),
    }
}

pub fn find_unique_projection_run_by_session(
    pool: &DbPool,
    tool: &str,
    session_id: &str,
) -> Result<AgentProjectionRunMatch> {
    if tool.trim().is_empty() {
        bail!("tool must be non-empty");
    }
    if session_id.trim().is_empty() {
        bail!("session_id must be non-empty");
    }
    let canonical_tool = canonical_tool(tool)?;
    let connection = pool.get().context("acquire database connection")?;
    let mut statement = connection.prepare(
        "SELECT id FROM agent_runs
          WHERE tool = ?1
            AND native_session_id = ?2
            AND status IN ('starting','active','waiting','idle','stale')
          ORDER BY id LIMIT 2",
    )?;
    let ids = statement
        .query_map(params![canonical_tool, session_id.trim()], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match ids.as_slice() {
        [] => Ok(AgentProjectionRunMatch::None),
        [id] => Ok(AgentProjectionRunMatch::Unique(Box::new(sql::run_by_id(
            &connection,
            *id,
        )?))),
        _ => Ok(AgentProjectionRunMatch::Ambiguous),
    }
}
