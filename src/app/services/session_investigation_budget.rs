use super::session_investigation_support::{
    build_session_source_counts, summarize_session_source_evidence,
};
use super::*;

pub(super) async fn within_session_budget<T>(
    wall_time_ms: u32,
    work: impl std::future::Future<Output = ServiceResult<T>>,
) -> ServiceResult<T> {
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_millis(u64::from(wall_time_ms));
    let result = tokio::time::timeout(deadline, work).await.map_err(|_| {
        ServiceError::Busy("session_investigation_wall_time_budget_exceeded".into())
    })?;
    // A future that blocks within a ready poll can overshoot a Tokio timer.
    if started.elapsed() > deadline {
        return Err(ServiceError::Busy(
            "session_investigation_wall_time_budget_exceeded".into(),
        ));
    }
    result
}

type SessionEnvelope = InvestigationEnvelope<models::SessionInvestigateResponse>;

/// Bound the complete escaped JSON envelope, including its metadata. Preserve
/// section omission reasons and rebuild counts after removing returned rows.
pub(super) fn finalize_session_envelope(
    mut envelope: SessionEnvelope,
) -> ServiceResult<SessionEnvelope> {
    loop {
        refresh_counts(&mut envelope);
        // The byte count contributes digits to its own serialized envelope.
        // Iterate to a fixed point before comparing with the advertised limit.
        loop {
            let size = serde_json::to_vec(&envelope)
                .map_err(|error| ServiceError::Internal(error.into()))?
                .len();
            if envelope.metadata.budget_used.payload_bytes as usize == size {
                break;
            }
            envelope.metadata.budget_used.payload_bytes = u32::try_from(size).unwrap_or(u32::MAX);
        }
        if envelope.metadata.budget_used.payload_bytes <= envelope.metadata.payload_limit_bytes {
            return Ok(envelope);
        }
        if !prune_largest_section(&mut envelope.result)? {
            return Err(ServiceError::Busy(
                "session_investigation_identity_exceeds_payload_budget".into(),
            ));
        }
        let reason = "payload_budget_truncated".to_string();
        if !envelope.result.partial_reasons.contains(&reason) {
            envelope.result.partial_reasons.push(reason);
        }
        envelope.metadata.partial = true;
        envelope.metadata.truncated = true;
        envelope.metadata.partial_reasons = envelope.result.partial_reasons.clone();
        envelope.metadata.truncation_reasons = envelope
            .result
            .partial_reasons
            .iter()
            .filter(|reason| reason.contains("truncated"))
            .cloned()
            .collect();
    }
}

fn refresh_counts(envelope: &mut SessionEnvelope) {
    let result = &mut envelope.result;
    let source_id_limit = result
        .source_evidence
        .values()
        .map(|summary| summary.log_ids.len())
        .max()
        .unwrap_or(50)
        .max(1);
    result.source_evidence =
        summarize_session_source_evidence(result.correlation.as_ref(), source_id_limit);
    result.source_counts = build_session_source_counts(
        &result.source_evidence,
        &result.observatory,
        result.skill_events.len(),
        result.mcp_events.len(),
        result.hook_events.len(),
        result.artifact_evidence.len(),
        result.graph_neighborhood.as_ref().map(|graph| {
            (
                graph.entities.len(),
                graph.relationships.len(),
                graph.evidence.len(),
            )
        }),
        result
            .correlation
            .as_ref()
            .map_or(0, |correlation| correlation.heartbeat_summaries.len()),
        result.incident_context.error_logs.len(),
        result.notifications.len(),
        result.retention_lineage.len(),
    );
    result
        .source_counts
        .insert("transcript".into(), result.transcript.len());
    result
        .source_counts
        .insert("related_session".into(), result.related_sessions.len());
    result.source_counts.insert(
        "external_reference".into(),
        result.external_references.len(),
    );
    result
        .source_counts
        .insert("attributed_commit".into(), result.observatory.commits.len());
    result
        .source_counts
        .insert("observatory_run".into(), result.observatory.runs.len());
    envelope.metadata.budget_used.log_rows = (result.incident_context.error_logs.len()
        + result
            .correlation
            .as_ref()
            .map_or(0, |correlation| correlation.logs.len()))
        as u32;
    let observatory = &result.observatory;
    let graph_rows = result.graph_neighborhood.as_ref().map_or(0, |graph| {
        graph.entities.len() + graph.relationships.len() + graph.evidence.len()
    });
    envelope.metadata.budget_used.evidence_rows = (result.transcript.len()
        + result.skill_events.len()
        + result.mcp_events.len()
        + result.hook_events.len()
        + result.artifact_evidence.len()
        + result.notifications.len()
        + result.retention_lineage.len()
        + result.related_sessions.len()
        + result.external_references.len()
        + result.incident_context.ai_sessions.len()
        + observatory.runs.len()
        + observatory.actors.len()
        + observatory.related_runs.len()
        + observatory.commits.len()
        + observatory.events.len()
        + observatory.spans.len()
        + observatory.metrics.len()
        + graph_rows
        + usize::from(observatory.parent_run.is_some())
        + usize::from(observatory.previous_run.is_some())
        + usize::from(observatory.repository.is_some())
        + usize::from(observatory.worktree.is_some())
        + result
            .correlation
            .as_ref()
            .map_or(0, |correlation| correlation.heartbeat_summaries.len()))
        as u32;
}

fn prune_largest_section(result: &mut models::SessionInvestigateResponse) -> ServiceResult<bool> {
    let mut largest = (0, 0);
    macro_rules! candidate {
        ($id:expr, $value:expr, $present:expr) => {
            if $present {
                let bytes = serde_json::to_vec(&$value)
                    .map_err(|error| ServiceError::Internal(error.into()))?
                    .len();
                if bytes > largest.1 {
                    largest = ($id, bytes);
                }
            }
        };
    }
    candidate!(1, result.transcript, !result.transcript.is_empty());
    candidate!(2, result.correlation, result.correlation.is_some());
    candidate!(
        3,
        result.graph_neighborhood,
        result.graph_neighborhood.is_some()
    );
    candidate!(4, result.skill_events, !result.skill_events.is_empty());
    candidate!(5, result.mcp_events, !result.mcp_events.is_empty());
    candidate!(6, result.hook_events, !result.hook_events.is_empty());
    candidate!(
        7,
        result.artifact_evidence,
        !result.artifact_evidence.is_empty()
    );
    candidate!(
        8,
        result.observatory.runs,
        !result.observatory.runs.is_empty()
    );
    candidate!(
        9,
        result.observatory.actors,
        !result.observatory.actors.is_empty()
    );
    candidate!(
        10,
        result.observatory.related_runs,
        !result.observatory.related_runs.is_empty()
    );
    candidate!(
        11,
        result.observatory.commits,
        !result.observatory.commits.is_empty()
    );
    candidate!(
        12,
        result.observatory.events,
        !result.observatory.events.is_empty()
    );
    candidate!(
        13,
        result.observatory.spans,
        !result.observatory.spans.is_empty()
    );
    candidate!(
        14,
        result.observatory.metrics,
        !result.observatory.metrics.is_empty()
    );
    candidate!(
        15,
        result.incident_context.error_logs,
        !result.incident_context.error_logs.is_empty()
    );
    candidate!(16, result.notifications, !result.notifications.is_empty());
    candidate!(
        17,
        result.retention_lineage,
        !result.retention_lineage.is_empty()
    );
    candidate!(
        18,
        result.related_sessions,
        !result.related_sessions.is_empty()
    );
    candidate!(
        19,
        result.external_references,
        !result.external_references.is_empty()
    );
    candidate!(
        20,
        result.observatory.parent_run,
        result.observatory.parent_run.is_some()
    );
    candidate!(
        21,
        result.observatory.previous_run,
        result.observatory.previous_run.is_some()
    );
    candidate!(
        22,
        result.observatory.repository,
        result.observatory.repository.is_some()
    );
    candidate!(
        23,
        result.observatory.worktree,
        result.observatory.worktree.is_some()
    );
    candidate!(
        24,
        result.incident_context.ai_sessions,
        !result.incident_context.ai_sessions.is_empty()
    );
    candidate!(
        25,
        result.incident_context.by_app,
        !result.incident_context.by_app.is_empty()
    );
    candidate!(26, result.session.title, result.session.title.is_some());
    candidate!(
        27,
        result.session.transcript_path,
        result.session.transcript_path.is_some()
    );
    let section = match largest.0 {
        0 => return Ok(false),
        1 => {
            result.transcript.truncate(result.transcript.len() / 2);
            result.transcript_has_more = true;
            "transcript"
        }
        2 => {
            result.correlation = None;
            "correlation"
        }
        3 => {
            result.graph_neighborhood = None;
            "graph_neighborhood"
        }
        4 => {
            result.skill_events.truncate(result.skill_events.len() / 2);
            result.skill_events_truncated = true;
            "skill_events"
        }
        5 => {
            result.mcp_events.truncate(result.mcp_events.len() / 2);
            result.mcp_events_truncated = true;
            "mcp_events"
        }
        6 => {
            result.hook_events.truncate(result.hook_events.len() / 2);
            result.hook_events_truncated = true;
            "hook_events"
        }
        7 => {
            result
                .artifact_evidence
                .truncate(result.artifact_evidence.len() / 2);
            result.artifact_evidence_truncated = true;
            "artifact_evidence"
        }
        8 => {
            result
                .observatory
                .runs
                .truncate(result.observatory.runs.len() / 2);
            "observatory_runs"
        }
        9 => {
            result
                .observatory
                .actors
                .truncate(result.observatory.actors.len() / 2);
            result.observatory.actors_truncated = true;
            "observatory_actors"
        }
        10 => {
            result
                .observatory
                .related_runs
                .truncate(result.observatory.related_runs.len() / 2);
            result.observatory.related_runs_truncated = true;
            "observatory_related_runs"
        }
        11 => {
            result
                .observatory
                .commits
                .truncate(result.observatory.commits.len() / 2);
            result.observatory.commits_truncated = true;
            "observatory_commits"
        }
        12 => {
            result
                .observatory
                .events
                .truncate(result.observatory.events.len() / 2);
            result.observatory.events_truncated = true;
            "observatory_events"
        }
        13 => {
            result
                .observatory
                .spans
                .truncate(result.observatory.spans.len() / 2);
            result.observatory.spans_truncated = true;
            "observatory_spans"
        }
        14 => {
            result
                .observatory
                .metrics
                .truncate(result.observatory.metrics.len() / 2);
            result.observatory.metrics_truncated = true;
            "observatory_metrics"
        }
        15 => {
            result
                .incident_context
                .error_logs
                .truncate(result.incident_context.error_logs.len() / 2);
            result.incident_context.error_logs_truncated = true;
            "incident_context_errors"
        }
        16 => {
            result
                .notifications
                .truncate(result.notifications.len() / 2);
            result.notifications_truncated = true;
            "notifications"
        }
        17 => {
            result
                .retention_lineage
                .truncate(result.retention_lineage.len() / 2);
            result.retention_lineage_truncated = true;
            "retention_lineage"
        }
        18 => {
            result
                .related_sessions
                .truncate(result.related_sessions.len() / 2);
            result.related_sessions_truncated = true;
            "related_sessions"
        }
        19 => {
            result
                .external_references
                .truncate(result.external_references.len() / 2);
            result.external_references_truncated = true;
            "external_references"
        }
        20 => {
            result.observatory.parent_run = None;
            "parent_run"
        }
        21 => {
            result.observatory.previous_run = None;
            "previous_run"
        }
        22 => {
            result.observatory.repository = None;
            "repository"
        }
        23 => {
            result.observatory.worktree = None;
            "worktree"
        }
        24 => {
            result
                .incident_context
                .ai_sessions
                .truncate(result.incident_context.ai_sessions.len() / 2);
            "incident_ai_sessions"
        }
        25 => {
            result
                .incident_context
                .by_app
                .truncate(result.incident_context.by_app.len() / 2);
            "incident_apps"
        }
        26 => {
            result.session.title = None;
            "session_title"
        }
        _ => {
            result.session.transcript_path = None;
            "transcript_path"
        }
    };
    let reason = format!("{section}_payload_truncated");
    if !result.partial_reasons.contains(&reason) {
        result.partial_reasons.push(reason);
    }
    Ok(true)
}

#[cfg(test)]
#[path = "session_investigation_budget_tests.rs"]
mod tests;
