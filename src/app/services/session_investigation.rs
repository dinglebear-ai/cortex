use std::time::Instant;

use super::*;

use super::session_investigation_support::{
    build_session_source_counts, extract_session_external_references,
    related_sessions_for_investigation, session_investigation_metadata,
    summarize_session_source_evidence,
};

impl CortexService {
    pub async fn session_investigate(
        &self,
        req: models::SessionInvestigateRequest,
    ) -> ServiceResult<InvestigationEnvelope<models::SessionInvestigateResponse>> {
        let budget = InvestigationBudget::default();
        super::session_investigation_budget::within_session_budget(
            budget.max_wall_time_ms,
            self.session_investigate_inner(req),
        )
        .await
    }

    async fn session_investigate_inner(
        &self,
        req: models::SessionInvestigateRequest,
    ) -> ServiceResult<InvestigationEnvelope<models::SessionInvestigateResponse>> {
        let started = Instant::now();
        let mut budget = InvestigationBudget::default();
        let session_id = req.session_id.trim().to_string();
        if session_id.is_empty() {
            return Err(ServiceError::InvalidInput(
                "session_id must not be empty".to_string(),
            ));
        }
        let section_limit = req.limit.unwrap_or(100).clamp(1, 200);
        // The composition has multiple independently bounded evidence lanes.
        budget.max_log_rows = 1_000 + section_limit;
        budget.max_evidence_rows = section_limit * 18 + 512;

        let sessions = self
            .list_sessions(ListSessionsRequest {
                project: req.project.clone(),
                tool: req.tool.clone(),
                session_id: Some(session_id.clone()),
                host: req.host.clone(),
                since: None,
                until: None,
                limit: Some(20),
                offset: None,
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

        let correlation = self
            .session_graph_correlation(&session, req.severity_min.as_deref())
            .await?;
        let session_entity_keys = correlation
            .as_ref()
            .map(|value| value.session_entity_keys.clone())
            .unwrap_or_default();
        let session_graph_entity_ambiguous = session_entity_keys.len() > 1;
        let mut graph_neighborhood_truncated = false;
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
                graph_neighborhood_truncated = around.metadata.truncated;
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

        let mut notifications = super::session_investigation_sections::session_notifications(
            self,
            &session,
            section_limit,
        )
        .await?;
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
        let (artifact_evidence, artifact_evidence_truncated) =
            super::session_investigation_sections::merge_artifact_evidence(
                artifact_by_correlation,
                artifact_by_request,
                section_limit as usize,
            );

        let observatory = super::session_investigation_sections::session_observatory(
            self,
            &session,
            section_limit,
        )
        .await?;

        let (related_sessions, related_sessions_truncated) =
            related_sessions_for_investigation(self, &session).await?;

        let (external_references, external_references_truncated) =
            extract_session_external_references(&transcript_page.events, section_limit as usize);
        let source_evidence =
            summarize_session_source_evidence(correlation.as_ref(), section_limit as usize);
        let graph_counts = graph_neighborhood.as_ref().map(|graph| {
            (
                graph.entities.len(),
                graph.relationships.len(),
                graph.evidence.len(),
            )
        });
        let heartbeat_summary_count = correlation
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
        if external_references_truncated {
            partial_reasons.push("external_references_truncated".to_string());
        }
        if transcript_page.has_more {
            partial_reasons.push("transcript_truncated".to_string());
        }
        if correlation.as_ref().is_some_and(|value| value.truncated) {
            partial_reasons.push("correlation_truncated".to_string());
        }
        if graph_neighborhood_truncated {
            partial_reasons.push("graph_neighborhood_truncated".to_string());
        }
        if related_sessions_truncated {
            partial_reasons.push("related_sessions_truncated".to_string());
        }
        if observatory.commits_truncated {
            partial_reasons.push("observatory_commits_truncated".to_string());
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
            correlation.as_ref(),
            graph_neighborhood.is_some(),
            &partial_reasons,
            transcript_page.events.len(),
            observatory.events.len(),
        );
        let envelope = InvestigationEnvelope {
            metadata,
            result: models::SessionInvestigateResponse {
                session,
                transcript: transcript_page.events,
                transcript_has_more: transcript_page.has_more,
                correlation,
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
                related_sessions_truncated,
                external_references,
                external_references_truncated,
                source_counts,
                source_evidence,
                partial_reasons,
            },
        };
        self.run_db("session_investigate_finalize", move |_| {
            let mut envelope =
                super::session_investigation_budget::finalize_session_envelope(envelope)
                    .map_err(anyhow::Error::from)?;
            envelope.metadata.budget_used.wall_time_ms =
                started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            if started.elapsed()
                > std::time::Duration::from_millis(u64::from(
                    envelope.metadata.budget.max_wall_time_ms,
                ))
            {
                return Err(anyhow::Error::from(ServiceError::Busy(
                    "session_investigation_wall_time_budget_exceeded".into(),
                )));
            }
            // Updating elapsed usage can change the escaped envelope byte count.
            super::session_investigation_budget::finalize_session_envelope(envelope)
                .map_err(anyhow::Error::from)
        })
        .await
    }
}

#[cfg(test)]
#[path = "session_investigation_tests.rs"]
mod tests;
