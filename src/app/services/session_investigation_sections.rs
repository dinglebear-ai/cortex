use super::*;

pub(super) async fn session_notifications(
    service: &CortexService,
    session: &AiSessionEntry,
    limit: u32,
) -> ServiceResult<Vec<db::notifications::FiringRow>> {
    let host = session.hostname.clone();
    let start = session.first_seen.clone();
    let end = session.last_seen.clone();
    service
        .run_db("session_investigate_notifications", move |pool| {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id,outbox_id,rule_id,hostname,fired_at,status_code
             FROM notification_firings
             WHERE hostname=?1 AND julianday(fired_at)>=julianday(?2)
               AND julianday(fired_at)<=julianday(?3)
             ORDER BY fired_at DESC,id DESC LIMIT ?4",
            )?;
            Ok(stmt
                .query_map(
                    rusqlite::params![host, start, end, i64::from(limit) + 1],
                    |row| {
                        Ok(db::notifications::FiringRow {
                            id: row.get(0)?,
                            outbox_id: row.get(1)?,
                            rule_id: row.get(2)?,
                            hostname: row.get(3)?,
                            fired_at: row.get(4)?,
                            status_code: row.get(5)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

pub(super) fn merge_artifact_evidence(
    correlation: models::ListArtifactEvidenceResponse,
    request: models::ListArtifactEvidenceResponse,
    limit: usize,
) -> (Vec<models::ArtifactEvidenceEntry>, bool) {
    let source_truncated = correlation.truncated || request.truncated;
    let mut events = BTreeMap::new();
    for event in correlation.events.into_iter().chain(request.events) {
        events.insert(event.cortex_log_id, event);
    }
    let truncated = source_truncated || events.len() > limit;
    (events.into_values().rev().take(limit).collect(), truncated)
}

pub(super) async fn session_observatory(
    service: &CortexService,
    session: &AiSessionEntry,
    section_limit: u32,
) -> ServiceResult<models::SessionObservatoryEvidence> {
    let observatory_session_id = session.session_id.clone();
    let observatory_tool = session.tool.clone();
    let observatory_host = session.hostname.clone();
    service
        .run_db("session_investigate_observatory", move |pool| {
            let query = db::agent_observatory::AgentRunQuery {
                tools: vec![observatory_tool.clone()],
                host: Some(observatory_host.clone()),
                native_session_id: Some(observatory_session_id.clone()),
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
                anyhow::anyhow!("Agent Observatory run disappeared during session investigation")
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
                section_limit as usize,
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
            let mut commits = db::agent_observatory::list_agent_run_attributed_commits(
                pool,
                run_id,
                section_limit as usize,
            )?;
            let commits_truncated = commits.len() > section_limit as usize;
            commits.truncate(section_limit as usize);
            let mut related_runs = match run.primary_worktree_id {
                Some(worktree_id) => db::agent_observatory::list_observatory_runs(
                    pool,
                    &db::agent_observatory::AgentRunQuery {
                        worktree_id: Some(worktree_id),
                        exclude_run_id: Some(run_id),
                        ..Default::default()
                    },
                    None,
                    section_limit as usize,
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
                section_limit as usize,
                true,
                i64::MAX,
            )?;
            let spans = db::agent_observatory::list_observatory_spans(
                pool,
                run_id,
                &identity,
                &db::agent_observatory::TelemetryQuery::default(),
                None,
                section_limit as usize,
                i64::MAX,
            )?;
            let metrics = db::agent_observatory::list_observatory_metrics(
                pool,
                run_id,
                &identity,
                &db::agent_observatory::TelemetryQuery::default(),
                None,
                section_limit as usize,
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
                commits_truncated,
                events_truncated: events.len() > section_limit as usize,
                spans_truncated: spans.len() > section_limit as usize,
                metrics_truncated: metrics.len() > section_limit as usize,
                events: events.into_iter().take(section_limit as usize).collect(),
                spans: spans.into_iter().take(section_limit as usize).collect(),
                metrics: metrics.into_iter().take(section_limit as usize).collect(),
            })
        })
        .await
}
