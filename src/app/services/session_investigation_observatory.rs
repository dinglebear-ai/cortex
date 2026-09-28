use super::*;

impl CortexService {
    pub(super) async fn session_observatory_evidence(
        &self,
        session: &AiSessionEntry,
        section_limit: u32,
    ) -> ServiceResult<models::SessionObservatoryEvidence> {
        let observatory_session_id = session.session_id.clone();
        let observatory_tool = session.tool.clone();
        let observatory_host = session.hostname.clone();
        self.run_db("session_investigate_observatory", move |pool| {
            let mut runs = db::agent_observatory::list_observatory_session_runs(
                pool,
                &observatory_session_id,
                &observatory_tool,
                &observatory_host,
                section_limit as usize,
            )?;
            let runs_truncated = runs.len() > section_limit as usize;
            let ambiguous_run = runs.len() > 1;
            runs.truncate(section_limit as usize);
            let Some(run) = runs.first().cloned().filter(|_| !ambiguous_run) else {
                return Ok(models::SessionObservatoryEvidence {
                    runs,
                    ambiguous_run,
                    runs_truncated,
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
                runs_truncated,
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
}
