use std::time::Instant;

use super::*;

use super::session_investigation_support::{
    build_session_source_counts, extract_session_external_references,
    related_sessions_for_investigation, session_investigation_metadata,
    summarize_session_source_evidence, timestamp_inclusive_between,
};


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
                let parent_run = match run.parent_run_id {
                    Some(id) => db::agent_observatory::resolve_observatory_run_row(pool, id)?,
                    None => None,
                };
                let previous_run = match run.previous_run_id {
                    Some(id) => db::agent_observatory::resolve_observatory_run_row(pool, id)?,
                    None => None,
                };
                let mut actors = db::agent_observatory::list_observatory_run_actors(
                    pool,
                    run_id,
                    section_limit as usize + 1,
                )?;
                let actors_truncated = actors.len() > section_limit as usize;
                actors.truncate(section_limit as usize);
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
                    parent_run,
                    previous_run,
                    actors,
                    actors_truncated,
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

        let related_sessions = related_sessions_for_investigation(self, &session).await?;

        let external_references = extract_session_external_references(&transcript_page.events);
        let source_evidence = summarize_session_source_evidence(
            correlated.graph_correlation.as_ref(),
            section_limit as usize,
        );
        let graph_counts = graph_neighborhood.as_ref().map(|graph| {
            (
                graph.entities.len(),
                graph.relationships.len(),
                graph.evidence.len(),
            )
        });
        let heartbeat_summary_count = correlated
            .graph_correlation
            .as_ref()
            .map_or(0, |value| value.heartbeat_summaries.len());
        let source_counts = build_session_source_counts(
            &source_evidence,
            &observatory,
            skill_events.events.len(),
            mcp_events.events.len(),
            hook_events.events.len(),
            artifact_evidence.len(),
            graph_counts,
            heartbeat_summary_count,
            incident_context.error_logs.len(),
            notifications.len(),
            retention_lineage.len(),
        );

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
        if observatory.actors_truncated {
            partial_reasons.push("observatory_actors_truncated".to_string());
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

#[cfg(test)]
#[path = "session_investigation_tests.rs"]
mod tests;
