use std::time::Instant;

use super::*;

pub(super) async fn related_sessions_for_investigation(
    service: &CortexService,
    session: &AiSessionEntry,
) -> ServiceResult<Vec<AiSessionEntry>> {
    Ok(service
        .list_sessions(ListSessionsRequest {
            project: Some(session.project.clone()),
            tool: None,
            session_id: None,
            host: None,
            since: Some(session.first_seen.clone()),
            until: Some(session.last_seen.clone()),
            limit: Some(50),
        })
        .await?
        .sessions
        .into_iter()
        .filter(|candidate| candidate.session_key != session.session_key)
        .take(20)
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_session_source_counts(
    source_evidence: &std::collections::BTreeMap<String, models::SessionSourceEvidenceSummary>,
    observatory: &models::SessionObservatoryEvidence,
    skill_event_count: usize,
    mcp_event_count: usize,
    hook_event_count: usize,
    artifact_evidence_count: usize,
    graph_counts: Option<(usize, usize, usize)>,
    heartbeat_summary_count: usize,
    incident_error_log_count: usize,
    notification_count: usize,
    retention_lineage_count: usize,
) -> std::collections::BTreeMap<String, usize> {
    let mut source_counts = source_evidence
        .iter()
        .map(|(kind, summary)| (kind.clone(), summary.count))
        .collect::<std::collections::BTreeMap<_, _>>();
    for event in &observatory.events {
        *source_counts
            .entry(format!("observatory:{}", event.source_kind))
            .or_default() += 1;
    }
    if skill_event_count > 0 {
        source_counts.insert("skill_event".to_string(), skill_event_count);
    }
    if mcp_event_count > 0 {
        source_counts.insert("mcp_event".to_string(), mcp_event_count);
    }
    if hook_event_count > 0 {
        source_counts.insert("hook_event".to_string(), hook_event_count);
    }
    if artifact_evidence_count > 0 {
        source_counts.insert("artifact_evidence".to_string(), artifact_evidence_count);
    }
    if !observatory.spans.is_empty() {
        source_counts.insert("otlp_span".to_string(), observatory.spans.len());
    }
    if !observatory.metrics.is_empty() {
        source_counts.insert("otlp_metric".to_string(), observatory.metrics.len());
    }
    if let Some((entities, relationships, evidence)) = graph_counts {
        source_counts.insert("graph_entity".to_string(), entities);
        source_counts.insert("graph_relationship".to_string(), relationships);
        source_counts.insert("graph_evidence".to_string(), evidence);
    }
    if heartbeat_summary_count > 0 {
        source_counts.insert("heartbeat_summary".to_string(), heartbeat_summary_count);
    }
    if incident_error_log_count > 0 {
        source_counts.insert("incident_error_log".to_string(), incident_error_log_count);
    }
    if notification_count > 0 {
        source_counts.insert("notification".to_string(), notification_count);
    }
    if !observatory.actors.is_empty() {
        source_counts.insert("observatory_actor".to_string(), observatory.actors.len());
    }
    if observatory.parent_run.is_some() {
        source_counts.insert("parent_run".to_string(), 1);
    }
    if observatory.previous_run.is_some() {
        source_counts.insert("previous_run".to_string(), 1);
    }
    if !observatory.related_runs.is_empty() {
        source_counts.insert(
            "same_worktree_run".to_string(),
            observatory.related_runs.len(),
        );
    }
    if retention_lineage_count > 0 {
        source_counts.insert("retention_lineage".to_string(), retention_lineage_count);
    }
    source_counts
}

pub(super) fn session_investigation_metadata(
    budget: &InvestigationBudget,
    started: Instant,
    correlation: Option<&GraphSessionCorrelation>,
    has_graph_neighborhood: bool,
    partial_reasons: &[String],
    transcript_rows: usize,
    evidence_rows: usize,
) -> InvestigationMetadata {
    let graph_calls = u32::from(correlation.is_some()) + u32::from(has_graph_neighborhood);
    let log_rows = correlation.map_or(0, |value| value.logs.len() as u32);
    let partial = !partial_reasons.is_empty();
    InvestigationMetadata {
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: INVESTIGATION_UI_VERSION.to_string(),
        graph_projection_status: correlation
            .map(|value| if value.used_graph { "used" } else { "fallback" }.to_string()),
        source_watermark: None,
        degraded_reasons: correlation
            .filter(|value| !value.used_graph)
            .map(|_| vec!["session_graph_entity_unavailable".to_string()])
            .unwrap_or_default(),
        truncated: partial,
        truncation_reasons: partial_reasons.to_vec(),
        partial,
        partial_reasons: partial_reasons.to_vec(),
        auth_state: "bearer".to_string(),
        budget: budget.clone(),
        budget_used: InvestigationBudgetUsed {
            graph_calls,
            log_rows,
            evidence_rows: evidence_rows.min(u32::MAX as usize) as u32,
            candidate_explanations: 0,
            wall_time_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            payload_bytes: 0,
        },
        payload_limit_bytes: budget.max_payload_bytes,
        version_skew: (transcript_rows > 200).then(|| "transcript_limit_exceeded".to_string()),
    }
}

pub(super) fn summarize_session_source_evidence(
    correlation: Option<&GraphSessionCorrelation>,
    requested_limit: usize,
) -> BTreeMap<String, models::SessionSourceEvidenceSummary> {
    let id_limit = requested_limit.clamp(1, 50);
    let mut summaries = BTreeMap::<String, models::SessionSourceEvidenceSummary>::new();
    let Some(correlation) = correlation else {
        return summaries;
    };

    for row in &correlation.logs {
        let kind = row
            .source_kind
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let summary = summaries.entry(kind).or_default();
        summary.count += 1;
        if summary.log_ids.len() < id_limit {
            summary.log_ids.push(row.entry.id);
        } else {
            summary.truncated = true;
        }
    }

    summaries
}

pub(super) fn timestamp_inclusive_between(value: &str, start: &str, end: &str) -> bool {
    let Ok(value) = chrono::DateTime::parse_from_rfc3339(value) else {
        return false;
    };
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(start) else {
        return false;
    };
    let Ok(end) = chrono::DateTime::parse_from_rfc3339(end) else {
        return false;
    };
    value >= start && value <= end
}

pub(super) fn extract_session_external_references(
    events: &[models::RenderedSessionEvent],
) -> Vec<models::SessionExternalReference> {
    let mut references = std::collections::BTreeSet::new();
    for event in events {
        for raw in event.text.split_whitespace() {
            let token = raw.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.' | ';' | ':' | ')' | '(' | ']' | '[' | '}' | '{' | '"' | '\''
                )
            });
            if token.is_empty() {
                continue;
            }
            let kind = if token.starts_with("http://") || token.starts_with("https://") {
                if token.contains("github.com/") && token.contains("/pull/") {
                    models::SessionExternalReferenceKind::GithubPullRequest
                } else if token.contains("github.com/") && token.contains("/issues/") {
                    models::SessionExternalReferenceKind::GithubIssue
                } else {
                    models::SessionExternalReferenceKind::Url
                }
            } else if looks_like_linear_identifier(token) {
                models::SessionExternalReferenceKind::LinearIssue
            } else if token.strip_prefix('#').is_some_and(|number| {
                !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
            }) {
                models::SessionExternalReferenceKind::GithubReference
            } else if looks_like_commit_sha(token) {
                models::SessionExternalReferenceKind::CommitSha
            } else {
                continue;
            };
            references.insert(models::SessionExternalReference {
                kind,
                value: safe_passive_text(token, 500),
                source_position: Some(event.position),
                evidence_kind: "transcript_text".to_string(),
                trust_level: "claimed".to_string(),
                verified: false,
            });
        }
    }
    references.into_iter().collect()
}

pub(super) fn looks_like_linear_identifier(token: &str) -> bool {
    let Some((prefix, suffix)) = token.split_once('-') else {
        return false;
    };
    (2..=12).contains(&prefix.len())
        && prefix
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        && !suffix.is_empty()
        && suffix.chars().all(|ch| ch.is_ascii_digit())
}

pub(super) fn looks_like_commit_sha(token: &str) -> bool {
    (7..=40).contains(&token.len())
        && token.chars().all(|ch| ch.is_ascii_hexdigit())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}
