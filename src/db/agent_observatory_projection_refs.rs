//! Foreign-key resolution for Agent Observatory projection writes.

use super::sql::run_id;
use super::types::AgentRunUpsert;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, Transaction};

pub(super) struct RunRefs {
    pub parent_run_id: Option<i64>,
    pub previous_run_id: Option<i64>,
    pub primary_worktree_id: Option<i64>,
}

fn required_run_id(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    run_id(tx, key)?.with_context(|| format!("run not found for key {key}"))
}

pub(super) fn worktree_id(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    tx.query_row(
        "SELECT id FROM repository_worktrees WHERE worktree_key = ?1 AND removed_at IS NULL",
        [key],
        |row| row.get(0),
    )
    .optional()?
    .with_context(|| format!("worktree not found for key {key}"))
}

pub(super) fn resolve_run_refs(tx: &Transaction<'_>, input: &AgentRunUpsert) -> Result<RunRefs> {
    Ok(RunRefs {
        parent_run_id: input
            .parent_run_key
            .as_deref()
            .map(|key| required_run_id(tx, key))
            .transpose()?,
        previous_run_id: input
            .previous_run_key
            .as_deref()
            .map(|key| required_run_id(tx, key))
            .transpose()?,
        primary_worktree_id: input
            .primary_worktree_key
            .as_deref()
            .map(|key| worktree_id(tx, key))
            .transpose()?,
    })
}
