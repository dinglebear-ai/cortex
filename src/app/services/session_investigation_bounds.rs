use super::*;

/// Bound the serialized envelope, including metadata and JSON escaping.
/// Keep session identity intact; every removed evidence section is explicit.
pub(super) fn bound_session_payload(
    envelope: &mut InvestigationEnvelope<models::SessionInvestigateResponse>,
) -> ServiceResult<()> {
    loop {
        refresh_counts(envelope);
        let bytes = serde_json::to_vec(envelope)
            .map_err(anyhow::Error::from)?
            .len();
        envelope.metadata.budget_used.payload_bytes = bytes.min(u32::MAX as usize) as u32;
        let final_bytes = serde_json::to_vec(envelope)
            .map_err(anyhow::Error::from)?
            .len();
        if final_bytes <= envelope.metadata.payload_limit_bytes as usize {
            // The byte-count field can grow by a digit when populated.
            if final_bytes == bytes {
                return Ok(());
            }
            continue;
        }
        if !shrink_largest_section(&mut envelope.result)? {
            return Err(ServiceError::InvalidInput(
                "session identity exceeds the investigation payload budget".to_string(),
            ));
        }
        envelope.metadata.partial = true;
        envelope.metadata.truncated = true;
        for reasons in [
            &mut envelope.metadata.partial_reasons,
            &mut envelope.metadata.truncation_reasons,
            &mut envelope.result.partial_reasons,
        ] {
            if !reasons
                .iter()
                .any(|reason| reason == "payload_budget_exceeded")
            {
                reasons.push("payload_budget_exceeded".to_string());
            }
        }
    }
}

fn shrink_largest_section(r: &mut models::SessionInvestigateResponse) -> ServiceResult<bool> {
    let mut largest = (0usize, None);
    macro_rules! candidate {
        ($field:expr, $id:expr) => {
            if !$field.is_empty() {
                let bytes = serde_json::to_vec(&$field)
                    .map_err(anyhow::Error::from)?
                    .len();
                if bytes > largest.0 {
                    largest = (bytes, Some($id));
                }
            }
        };
    }
    candidate!(r.transcript, 0);
    candidate!(r.observatory.events, 1);
    candidate!(r.observatory.spans, 2);
    candidate!(r.observatory.metrics, 3);
    candidate!(r.observatory.commits, 4);
    candidate!(r.observatory.actors, 5);
    candidate!(r.observatory.related_runs, 6);
    candidate!(r.observatory.runs, 7);
    candidate!(r.artifact_evidence, 8);
    candidate!(r.skill_events, 9);
    candidate!(r.mcp_events, 10);
    candidate!(r.hook_events, 11);
    candidate!(r.notifications, 12);
    candidate!(r.retention_lineage, 13);
    candidate!(r.related_sessions, 14);
    candidate!(r.external_references, 15);
    candidate!(r.incident_context.error_logs, 16);
    candidate!(r.incident_context.by_app, 17);
    candidate!(r.incident_context.ai_sessions, 18);
    macro_rules! optional {
        ($field:expr, $id:expr) => {
            if let Some(value) = &$field {
                let bytes = serde_json::to_vec(value)
                    .map_err(anyhow::Error::from)?
                    .len();
                if bytes > largest.0 {
                    largest = (bytes, Some($id));
                }
            }
        };
    }
    optional!(r.correlation, 19);
    optional!(r.graph_neighborhood, 20);
    optional!(r.observatory.repository, 21);
    optional!(r.observatory.worktree, 22);
    optional!(r.observatory.parent_run, 23);
    optional!(r.observatory.previous_run, 24);
    macro_rules! halve {
        ($field:expr, $flag:expr) => {{
            $field.truncate($field.len() / 2);
            $flag = true;
        }};
    }
    let reason = match largest.1 {
        Some(0) => {
            halve!(r.transcript, r.transcript_has_more);
            "transcript"
        }
        Some(1) => {
            halve!(r.observatory.events, r.observatory.events_truncated);
            "observatory_events"
        }
        Some(2) => {
            halve!(r.observatory.spans, r.observatory.spans_truncated);
            "observatory_spans"
        }
        Some(3) => {
            halve!(r.observatory.metrics, r.observatory.metrics_truncated);
            "observatory_metrics"
        }
        Some(4) => {
            halve!(r.observatory.commits, r.observatory.commits_truncated);
            "observatory_commits"
        }
        Some(5) => {
            halve!(r.observatory.actors, r.observatory.actors_truncated);
            "observatory_actors"
        }
        Some(6) => {
            halve!(
                r.observatory.related_runs,
                r.observatory.related_runs_truncated
            );
            "observatory_related_runs"
        }
        Some(7) => {
            halve!(r.observatory.runs, r.observatory.runs_truncated);
            "observatory_runs"
        }
        Some(8) => {
            halve!(r.artifact_evidence, r.artifact_evidence_truncated);
            "artifact_evidence"
        }
        Some(9) => {
            halve!(r.skill_events, r.skill_events_truncated);
            "skill_events"
        }
        Some(10) => {
            halve!(r.mcp_events, r.mcp_events_truncated);
            "mcp_events"
        }
        Some(11) => {
            halve!(r.hook_events, r.hook_events_truncated);
            "hook_events"
        }
        Some(12) => {
            halve!(r.notifications, r.notifications_truncated);
            "notifications"
        }
        Some(13) => {
            halve!(r.retention_lineage, r.retention_lineage_truncated);
            "retention_lineage"
        }
        Some(14) => {
            halve!(r.related_sessions, r.related_sessions_truncated);
            "related_sessions"
        }
        Some(15) => {
            halve!(r.external_references, r.external_references_truncated);
            "external_references"
        }
        Some(16) => {
            halve!(
                r.incident_context.error_logs,
                r.incident_context.error_logs_truncated
            );
            "incident_errors"
        }
        Some(17) => {
            r.incident_context
                .by_app
                .truncate(r.incident_context.by_app.len() / 2);
            "incident_apps"
        }
        Some(18) => {
            r.incident_context
                .ai_sessions
                .truncate(r.incident_context.ai_sessions.len() / 2);
            "incident_sessions"
        }
        Some(19) => {
            r.correlation = None;
            "correlation"
        }
        Some(20) => {
            r.graph_neighborhood = None;
            "graph_neighborhood"
        }
        Some(21) => {
            r.observatory.repository = None;
            "repository"
        }
        Some(22) => {
            r.observatory.worktree = None;
            "worktree"
        }
        Some(23) => {
            r.observatory.parent_run = None;
            "parent_run"
        }
        Some(24) => {
            r.observatory.previous_run = None;
            "previous_run"
        }
        _ => return Ok(false),
    };
    let reason = format!("{reason}_payload_truncated");
    if !r.partial_reasons.contains(&reason) {
        r.partial_reasons.push(reason);
    }
    Ok(true)
}

fn refresh_counts(envelope: &mut InvestigationEnvelope<models::SessionInvestigateResponse>) {
    let r = &mut envelope.result;
    if r.correlation.is_none() {
        r.source_evidence.clear();
    }
    r.source_counts = super::session_investigation_support::build_session_source_counts(
        &r.source_evidence,
        &r.observatory,
        r.skill_events.len(),
        r.mcp_events.len(),
        r.hook_events.len(),
        r.artifact_evidence.len(),
        r.graph_neighborhood
            .as_ref()
            .map(|g| (g.entities.len(), g.relationships.len(), g.evidence.len())),
        r.correlation
            .as_ref()
            .map_or(0, |c| c.heartbeat_summaries.len()),
        r.incident_context.error_logs.len(),
        r.notifications.len(),
        r.retention_lineage.len(),
    );
    envelope.metadata.budget_used.log_rows =
        r.correlation.as_ref().map_or(0, |c| c.logs.len() as u32);
    envelope.metadata.budget_used.evidence_rows = r
        .source_counts
        .values()
        .sum::<usize>()
        .min(u32::MAX as usize) as u32;
    for reason in &r.partial_reasons {
        if !envelope.metadata.partial_reasons.contains(reason) {
            envelope.metadata.partial_reasons.push(reason.clone());
            envelope.metadata.truncation_reasons.push(reason.clone());
        }
    }
}
