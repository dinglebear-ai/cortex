use std::time::Instant;

use super::*;

impl CortexService {
    pub async fn session_investigate(
        &self,
        req: models::SessionInvestigateRequest,
    ) -> ServiceResult<InvestigationEnvelope<models::SessionInvestigateResponse>> {
        let started = Instant::now();
        let budget = InvestigationBudget::default();
        let session_id = req.session_id.trim().to_string();
        if session_id.is_empty() {
            return Err(ServiceError::InvalidInput(
                "session_id must not be empty".to_string(),
            ));
        }
        let section_limit = req.limit.unwrap_or(100).clamp(1, 200);

        let sessions = self
            .list_sessions(ListSessionsRequest {
                project: req.project.clone(),
                tool: req.tool.clone(),
                session_id: Some(session_id.clone()),
                host: req.host.clone(),
                since: None,
                until: None,
                limit: Some(20),
            })
            .await?
            .sessions;
        let mut exact = sessions
            .into_iter()
            .filter(|session| session.session_id == session_id)
            .collect::<Vec<_>>();
        if exact.is_empty() {
            return Err(ServiceError::NotFound(format!(
                "AI session not found: {session_id}"
            )));
        }
        if exact.len() > 1 {
            return Err(ServiceError::InvalidInput(
                "session_id is ambiguous; provide tool, project, and/or host".to_string(),
            ));
        }
        let session = exact.remove(0);

        let transcript_page = self
            .rendered_session_page(models::RenderedSessionPageRequest {
                project: session.project.clone(),
                tool: session.tool.clone(),
                session_id: session.session_id.clone(),
                host: session.hostname.clone(),
                cursor: None,
                limit: Some(section_limit),
            })
            .await?;

        let correlated = self
            .correlate_ai_logs(AiCorrelateRequest {
                project: Some(session.project.clone()),
                tool: Some(session.tool.clone()),
                session_id: Some(session.session_id.clone()),
                host: Some(session.hostname.clone()),
                window_minutes: req.window_minutes,
                severity_min: req.severity_min.clone(),
                limit: Some(10),
                events_per_anchor: Some(section_limit.min(100)),
                ..Default::default()
            })
            .await?;

        let session_entity_keys = correlated
            .graph_correlation
            .as_ref()
            .map(|value| value.session_entity_keys.clone())
            .unwrap_or_default();
        let session_graph_entity_ambiguous = session_entity_keys.len() > 1;
        let graph_neighborhood = match session_entity_keys.as_slice() {
            [session_key] => {
                let around = self
                    .graph_around(GraphAroundRequest {
                        mode: Some("around".to_string()),
                        entity_type: Some("ai_session".to_string()),
                        key: Some(session_key.clone()),
                        depth: Some(1),
                        limit: Some(section_limit.min(100)),
                        evidence_sample_limit: Some(3),
                        payload_budget: Some(32_768),
                        ..Default::default()
                    })
                    .await?;
                Some(models::app_graph_from_around_response(&around))
            }
            _ => None,
        };

        let incident_context = self
            .incident_context(models::IncidentContextRequest {
                since: Some(session.first_seen.clone()),
                until: Some(session.last_seen.clone()),
                host: Some(session.hostname.clone()),
                app: None,
                query: None,
                severity_min: req.severity_min.clone(),
                limit: Some(section_limit.min(200)),
            })
            .await?;

        let mut notifications = self
            .notifications_recent_checked(models::NotificationsRecentRequest {
                limit: Some(i64::from(section_limit.saturating_mul(5).min(500))),
                rule_id: None,
                since: Some(session.first_seen.clone()),
            })
            .await?
            .into_iter()
            .filter(|firing| {
                firing.hostname == session.hostname
                    && timestamp_inclusive_between(
                        &firing.fired_at,
                        &session.first_seen,
                        &session.last_seen,
                    )
            })
            .collect::<Vec<_>>();
        let notifications_truncated = notifications.len() > section_limit as usize;
        notifications.truncate(section_limit as usize);

        let retention_project = session.project.clone();
        let retention_tool = session.tool.clone();
        let retention_session_id = session.session_id.clone();
        let retention_host = session.hostname.clone();
        let mut retention_lineage = self
            .run_db("session_investigate_retention_lineage", move |pool| {
                let conn = pool.get()?;
                let mut stmt = conn.prepare(
                    "SELECT id, hostname, app_name, severity, ai_project, ai_tool, ai_session_id,
                            deleted_at + 900
                     FROM stream_deleted_log_lineage
                     WHERE ai_project = ?1
                       AND lower(ai_tool) = lower(?2)
                       AND ai_session_id = ?3
                       AND hostname = ?4
                     ORDER BY id DESC
                     LIMIT ?5",
                )?;
                let rows = stmt.query_map(
                    rusqlite::params![
                        retention_project,
                        retention_tool,
                        retention_session_id,
                        retention_host,
                        i64::from(section_limit) + 1
                    ],
                    |row| {
                        Ok(models::SessionRetentionLineageEntry {
                            log_id: row.get(0)?,
                            hostname: row.get(1)?,
                            app_name: row.get(2)?,
                            severity: row.get(3)?,
                            ai_project: row.get(4)?,
                            ai_tool: row.get(5)?,
                            ai_session_id: row.get(6)?,
                            retained_until_epoch: row.get(7)?,
                        })
                    },
                )?;
                Ok(rows.collect::<Result<Vec<_>, rusqlite::Error>>()?)
            })
            .await?;
        let retention_lineage_truncated = retention_lineage.len() > section_limit as usize;
        retention_lineage.truncate(section_limit as usize);

        let (skill_events, mcp_events, hook_events) = tokio::try_join!(
            self.list_skill_events(models::ListSkillEventsRequest {
                tool: Some(session.tool.clone()),
                project: Some(session.project.clone()),
                session_id: Some(session.session_id.clone()),
                hostname: Some(session.hostname.clone()),
                limit: Some(section_limit),
                ..Default::default()
            }),
            self.list_mcp_events(models::ListMcpEventsRequest {
                tool: Some(session.tool.clone()),
                project: Some(session.project.clone()),
                session_id: Some(session.session_id.clone()),
                hostname: Some(session.hostname.clone()),
                limit: Some(section_limit),
                ..Default::default()
            }),
            self.list_hook_events(models::ListHookEventsRequest {
                tool: Some(session.tool.clone()),
                project: Some(session.project.clone()),
                session_id: Some(session.session_id.clone()),
                hostname: Some(session.hostname.clone()),
                limit: Some(section_limit),
                ..Default::default()
            })
        )?;

        let (artifact_by_correlation, artifact_by_request) = tokio::try_join!(
            self.list_artifact_evidence(models::ListArtifactEvidenceRequest {
                correlation_id: Some(session.session_id.clone()),
                from: Some(session.first_seen.clone()),
                to: Some(session.last_seen.clone()),
                limit: Some(section_limit),
                ..Default::default()
            }),
            self.list_artifact_evidence(models::ListArtifactEvidenceRequest {
                request_id: Some(session.session_id.clone()),
                from: Some(session.first_seen.clone()),
                to: Some(session.last_seen.clone()),
                limit: Some(section_limit),
                ..Default::default()
            })
        )?;
        let artifact_evidence_truncated =
            artifact_by_correlation.truncated || artifact_by_request.truncated;
        let mut artifact_evidence_by_id = BTreeMap::new();
        for event in artifact_by_correlation
            .events
            .into_iter()
            .chain(artifact_by_request.events)
        {
            artifact_evidence_by_id.insert(event.cortex_log_id, event);
        }
        let artifact_evidence = artifact_evidence_by_id.into_values().collect::<Vec<_>>();

        let observatory_session_id = session.session_id.clone();
        let observatory_tool = session.tool.clone();
        let observatory_host = session.hostname.clone();
        let observatory = self
            .run_db("session_investigate_observatory", move |pool| {
                let query = db::agent_observatory::AgentRunQuery {
                    tools: vec![observatory_tool.clone()],
                    host: Some(observatory_host.clone()),
                    query: Some(observatory_session_id.clone()),
                    ..Default::default()
                };
                let mut runs =
                    db::agent_observatory::list_observatory_runs(pool, &query, None, 50, i64::MAX)?;
                runs.retain(|run| {
                    run.native_session_id == observatory_session_id
                        && run.tool.eq_ignore_ascii_case(&observatory_tool)
                        && run.hostname == observatory_host
                });
                let ambiguous_run = runs.len() > 1;
                let Some(run) = runs.first().cloned().filter(|_| !ambiguous_run) else {
                    return Ok(models::SessionObservatoryEvidence {
                        runs,
                        ambiguous_run,
                        ..Default::default()
                    });
                };
                let resolved = db::agent_observatory::resolve_observatory_run(pool, &run.run_key)?;
                let (run_id, identity) = resolved.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Agent Observatory run disappeared during session investigation"
                    )
                })?;
                let worktree = match run.primary_worktree_id {
                    Some(id) => db::agent_observatory::resolve_observatory_worktree(pool, id)?,
                    None => None,
                };
                let repository = match worktree.as_ref() {
                    Some(worktree) => db::agent_observatory::resolve_observatory_repository(
                        pool,
                        worktree.repository_id,
                    )?,
                    None => None,
                };
                let commits =
                    db::agent_observatory::list_agent_run_attributed_commits(pool, run_id)?;
                let mut related_runs = match run.primary_worktree_id {
                    Some(worktree_id) => db::agent_observatory::list_observatory_runs(
                        pool,
                        &db::agent_observatory::AgentRunQuery {
                            worktree_id: Some(worktree_id),
                            ..Default::default()
                        },
                        None,
                        section_limit as usize + 1,
                        i64::MAX,
                    )?,
                    None => Vec::new(),
                };
                related_runs.retain(|candidate| candidate.id != run_id);
                let related_runs_truncated = related_runs.len() > section_limit as usize;
                related_runs.truncate(section_limit as usize);
                let events = db::agent_observatory::list_observatory_events(
                    pool,
                    &run.run_key,
                    &db::agent_observatory::AgentEventQuery::default(),
                    None,
                    section_limit as usize + 1,
                    true,
                    i64::MAX,
                )?;
                let spans = db::agent_observatory::list_observatory_spans(
                    pool,
                    run_id,
                    &identity,
                    &db::agent_observatory::TelemetryQuery::default(),
                    None,
                    section_limit as usize + 1,
                    i64::MAX,
                )?;
                let metrics = db::agent_observatory::list_observatory_metrics(
                    pool,
                    run_id,
                    &identity,
                    &db::agent_observatory::TelemetryQuery::default(),
                    None,
                    section_limit as usize + 1,
                    i64::MAX,
                )?;
                Ok(models::SessionObservatoryEvidence {
                    runs,
                    ambiguous_run,
                    related_runs,
                    related_runs_truncated,
                    repository,
                    worktree,
                    commits,
                    events_truncated: events.len() > section_limit as usize,
                    spans_truncated: spans.len() > section_limit as usize,
                    metrics_truncated: metrics.len() > section_limit as usize,
                    events: events.into_iter().take(section_limit as usize).collect(),
                    spans: spans.into_iter().take(section_limit as usize).collect(),
                    metrics: metrics.into_iter().take(section_limit as usize).collect(),
                })
            })
            .await?;

        let related_sessions = self
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
            .collect::<Vec<_>>();

        let external_references = extract_session_external_references(&transcript_page.events);
        let source_evidence = summarize_session_source_evidence(
            correlated.graph_correlation.as_ref(),
            section_limit as usize,
        );
        let mut source_counts = source_evidence
            .iter()
            .map(|(kind, summary)| (kind.clone(), summary.count))
            .collect::<BTreeMap<_, _>>();
        for event in &observatory.events {
            *source_counts
                .entry(format!("observatory:{}", event.source_kind))
                .or_default() += 1;
        }
        if !skill_events.events.is_empty() {
            source_counts.insert("skill_event".to_string(), skill_events.events.len());
        }
        if !mcp_events.events.is_empty() {
            source_counts.insert("mcp_event".to_string(), mcp_events.events.len());
        }
        if !hook_events.events.is_empty() {
            source_counts.insert("hook_event".to_string(), hook_events.events.len());
        }
        if !artifact_evidence.is_empty() {
            source_counts.insert("artifact_evidence".to_string(), artifact_evidence.len());
        }
        if !observatory.spans.is_empty() {
            source_counts.insert("otlp_span".to_string(), observatory.spans.len());
        }
        if !observatory.metrics.is_empty() {
            source_counts.insert("otlp_metric".to_string(), observatory.metrics.len());
        }
        if let Some(graph) = graph_neighborhood.as_ref() {
            source_counts.insert("graph_entity".to_string(), graph.entities.len());
            source_counts.insert("graph_relationship".to_string(), graph.relationships.len());
            source_counts.insert("graph_evidence".to_string(), graph.evidence.len());
        }
        if let Some(correlation) = correlated.graph_correlation.as_ref()
            && !correlation.heartbeat_summaries.is_empty()
        {
            source_counts.insert(
                "heartbeat_summary".to_string(),
                correlation.heartbeat_summaries.len(),
            );
        }
        if !incident_context.error_logs.is_empty() {
            source_counts.insert(
                "incident_error_log".to_string(),
                incident_context.error_logs.len(),
            );
        }
        if !notifications.is_empty() {
            source_counts.insert("notification".to_string(), notifications.len());
        }
        if !observatory.related_runs.is_empty() {
            source_counts.insert(
                "same_worktree_run".to_string(),
                observatory.related_runs.len(),
            );
        }
        if !retention_lineage.is_empty() {
            source_counts.insert("retention_lineage".to_string(), retention_lineage.len());
        }

        let mut partial_reasons = Vec::new();
        if transcript_page.has_more {
            partial_reasons.push("transcript_truncated".to_string());
        }
        if correlated
            .graph_correlation
            .as_ref()
            .is_some_and(|value| value.truncated)
        {
            partial_reasons.push("correlation_truncated".to_string());
        }
        if session_graph_entity_ambiguous {
            partial_reasons.push("session_graph_entity_ambiguous".to_string());
        }
        if skill_events.truncated {
            partial_reasons.push("skill_events_truncated".to_string());
        }
        if mcp_events.truncated {
            partial_reasons.push("mcp_events_truncated".to_string());
        }
        if hook_events.truncated {
            partial_reasons.push("hook_events_truncated".to_string());
        }
        if artifact_evidence_truncated {
            partial_reasons.push("artifact_evidence_truncated".to_string());
        }
        if observatory.ambiguous_run {
            partial_reasons.push("observatory_run_ambiguous".to_string());
        }
        if observatory.related_runs_truncated {
            partial_reasons.push("observatory_related_runs_truncated".to_string());
        }
        if observatory.events_truncated {
            partial_reasons.push("observatory_events_truncated".to_string());
        }
        if observatory.spans_truncated {
            partial_reasons.push("observatory_spans_truncated".to_string());
        }
        if observatory.metrics_truncated {
            partial_reasons.push("observatory_metrics_truncated".to_string());
        }
        if incident_context.error_logs_truncated {
            partial_reasons.push("incident_context_errors_truncated".to_string());
        }
        if notifications_truncated {
            partial_reasons.push("notifications_truncated".to_string());
        }
        if retention_lineage_truncated {
            partial_reasons.push("retention_lineage_truncated".to_string());
        }

        let metadata = session_investigation_metadata(
            &budget,
            started,
            correlated.graph_correlation.as_ref(),
            graph_neighborhood.is_some(),
            &partial_reasons,
            transcript_page.events.len(),
            observatory.events.len(),
        );
        Ok(InvestigationEnvelope {
            metadata,
            result: models::SessionInvestigateResponse {
                session,
                transcript: transcript_page.events,
                transcript_has_more: transcript_page.has_more,
                correlation: correlated.graph_correlation,
                graph_neighborhood,
                skill_events: skill_events.events,
                skill_events_truncated: skill_events.truncated,
                mcp_events: mcp_events.events,
                mcp_events_truncated: mcp_events.truncated,
                hook_events: hook_events.events,
                hook_events_truncated: hook_events.truncated,
                artifact_evidence,
                artifact_evidence_truncated,
                observatory,
                incident_context,
                notifications,
                notifications_truncated,
                retention_lineage,
                retention_lineage_truncated,
                related_sessions,
                external_references,
                source_counts,
                source_evidence,
                partial_reasons,
            },
        })
    }
}

fn session_investigation_metadata(
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

fn summarize_session_source_evidence(
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

fn timestamp_inclusive_between(value: &str, start: &str, end: &str) -> bool {
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

fn extract_session_external_references(
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

fn looks_like_linear_identifier(token: &str) -> bool {
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

fn looks_like_commit_sha(token: &str) -> bool {
    (7..=40).contains(&token.len())
        && token.chars().all(|ch| ch.is_ascii_hexdigit())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

#[cfg(test)]
#[path = "session_investigation_tests.rs"]
mod tests;
