//! Stable total-order tie-breaks for equal-freshness projection state.

use super::{AgentActorRow, AgentActorUpsert, AgentRunUpsert, AgentWorktreeEvidenceUpsert};
use crate::db::agent_observatory::{AgentRunRow, AgentRunWorktreeEvidenceRow};

pub(super) fn run_activity_wins(
    input: &AgentRunUpsert,
    primary_worktree_id: Option<i64>,
    row: &AgentRunRow,
) -> bool {
    input.last_activity_at > row.last_activity_at
        || (input.last_activity_at == row.last_activity_at
            && format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
                input.provider_tool,
                primary_worktree_id,
                input.transcript_path,
                input.process_id,
                input.primary_branch,
                input.current_head_sha,
                input.freshness_json,
                input.metadata_json
            ) > format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
                row.provider_tool,
                row.primary_worktree_id,
                row.transcript_path,
                row.process_id,
                row.primary_branch,
                row.current_head_sha,
                row.freshness_json,
                row.metadata_json
            ))
}

pub(super) fn run_status_wins(input: &AgentRunUpsert, row: &AgentRunRow) -> bool {
    input.status_observed_at > row.status_observed_at
        || (input.status_observed_at == row.status_observed_at
            && format!("{}|{}", input.status.as_str(), input.status_reason)
                > format!("{}|{}", row.status.as_str(), row.status_reason))
}

pub(super) fn actor_wins(input: &AgentActorUpsert, row: &AgentActorRow) -> bool {
    let incoming_time = input
        .last_activity_at
        .as_ref()
        .or(input.started_at.as_ref());
    let existing_time = row.last_activity_at.as_ref().or(row.started_at.as_ref());
    incoming_time > existing_time
        || (incoming_time == existing_time
            && format!(
                "{:?}|{:?}|{}",
                input.actor_type, input.display_name, input.metadata_json
            ) > format!(
                "{:?}|{:?}|{}",
                row.actor_type, row.display_name, row.metadata_json
            ))
}

pub(super) fn evidence_wins(
    input: &AgentWorktreeEvidenceUpsert,
    row: &AgentRunWorktreeEvidenceRow,
) -> bool {
    input.last_seen_at > row.last_seen_at
        || (input.last_seen_at == row.last_seen_at
            && format!(
                "{}|{}|{}|{}",
                input.trust_level.as_str(),
                input.confidence,
                input.is_primary,
                input.metadata_json
            ) > format!(
                "{}|{}|{}|{}",
                row.trust_level.as_str(),
                row.confidence,
                row.is_primary,
                row.metadata_json
            ))
}
