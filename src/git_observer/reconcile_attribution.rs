//! Post-commit Agent Observatory attribution for durable Git transitions.

use super::commit_import::ObservedCommitTransition;
use crate::agent_observatory::git_attribution::attribute_exact_commits;
use crate::db::DbPool;
use crate::db::agent_observatory::{GitRepositoryReconcileResult, RepositoryObservationKind};
use std::path::Path;

pub(super) fn attribute_commit_transitions(
    pool: &DbPool,
    transitions: &[ObservedCommitTransition],
    result: &GitRepositoryReconcileResult,
) {
    for transition in transitions {
        let Some(worktree) = result
            .topology
            .worktrees
            .iter()
            .find(|worktree| Path::new(&worktree.path) == transition.path)
        else {
            continue;
        };
        let Some(observation) = result.observations.iter().find(|observation| {
            observation.observation_kind == RepositoryObservationKind::Head
                && observation.worktree_id == Some(worktree.id)
                && observation.new_head_sha.as_deref() == Some(transition.new_sha.as_str())
        }) else {
            continue;
        };
        let commits = transition
            .new_shas
            .iter()
            .filter_map(|sha| {
                result
                    .commits
                    .iter()
                    .find(|commit| commit.sha == *sha)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if let Err(error) = attribute_exact_commits(
            pool,
            worktree.id,
            &observation.observation_key,
            &observation.observed_at,
            observation.old_head_sha.as_deref(),
            &transition.new_sha,
            &commits,
        ) {
            tracing::error!(
                worktree_id = worktree.id,
                new_head_sha = %transition.new_sha,
                error = %error,
                "Agent Observatory exact commit attribution failed; backfill can repair it"
            );
        }
    }
}
