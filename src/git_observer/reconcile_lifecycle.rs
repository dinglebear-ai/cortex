//! Worktree lifecycle state comparison for repository reconciliation.

use crate::db::DbPool;
use crate::db::agent_observatory::{
    RepositoryObservationInput, RepositoryObservationKind, RepositoryReconcileResult,
    RepositoryWorktreeRow, get_repository_by_key, list_repository_worktrees,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;

pub(super) fn previous_worktrees(
    pool: &DbPool,
    repository_key: &str,
) -> Result<(bool, Vec<RepositoryWorktreeRow>)> {
    let Some(repository) = get_repository_by_key(pool, repository_key)? else {
        return Ok((false, Vec::new()));
    };
    Ok((true, list_repository_worktrees(pool, repository.id, true)?))
}

pub(super) fn lifecycle_observations(
    repository_existed: bool,
    previous: &[RepositoryWorktreeRow],
    topology: &RepositoryReconcileResult,
    observed_at: &str,
) -> Result<Vec<RepositoryObservationInput>> {
    let by_key = previous
        .iter()
        .map(|row| (row.worktree_key.as_str(), row))
        .collect::<HashMap<_, _>>();
    let by_id = previous
        .iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let mut inputs = Vec::new();

    for id in &topology.removed_worktree_ids {
        let row = by_id.get(id).with_context(|| {
            format!("removed worktree id {id} was not present before reconcile")
        })?;
        inputs.push(RepositoryObservationInput {
            worktree_key: Some(row.worktree_key.clone()),
            observation_kind: RepositoryObservationKind::WorktreeRemoved,
            new_head_sha: None,
            summary: "worktree removed".to_string(),
            payload_json: json!({
                "path": row.path,
                "removed_at": observed_at,
            })
            .to_string(),
        });
    }

    if repository_existed {
        for row in &topology.worktrees {
            let previous_row = by_key.get(row.worktree_key.as_str());
            if previous_row.is_some_and(|previous| previous.removed_at.is_none()) {
                continue;
            }
            inputs.push(RepositoryObservationInput {
                worktree_key: Some(row.worktree_key.clone()),
                observation_kind: RepositoryObservationKind::WorktreeAdded,
                new_head_sha: None,
                summary: "worktree added".to_string(),
                payload_json: json!({
                    "path": row.path,
                    "reappeared": previous_row.is_some(),
                    "reappeared_at": observed_at,
                })
                .to_string(),
            });
        }
    }
    Ok(inputs)
}
