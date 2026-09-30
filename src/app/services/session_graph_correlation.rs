use super::*;

/// Log fan-out cap for the graph-anchored session lane of `ai_correlate`.
/// Clamped again to `[1, 1000]` inside `db::correlate_session_graph`.
pub(super) const GRAPH_SESSION_LOG_LIMIT: usize = 500;

/// Shape the DB-layer `SessionGraphInputs` into the API response, classifying
/// each row into a source lane (`agent_command` / `shell_history` /
/// `graph:host:<host>`) and counting the agent-command and shell-history lanes.
/// Heartbeat summaries are filtered to the discovered hosts. Returns `None` when
/// the session has no rows at all (empty bounds).
pub(super) fn build_graph_session_correlation(
    session_id: String,
    inputs: db::SessionGraphInputs,
    summaries: Vec<db::HeartbeatWindowSummary>,
) -> Option<GraphSessionCorrelation> {
    let (session_start, session_end) = inputs.bounds?;

    let truncated = inputs.source_fields_truncated || inputs.logs.len() >= GRAPH_SESSION_LOG_LIMIT;
    let mut agent_command_count = 0usize;
    let mut shell_history_count = 0usize;
    let logs: Vec<CorrelatedLogRow> = inputs
        .logs
        .into_iter()
        .map(|entry| {
            let source_kind = row_source_kind(&entry);
            let discovery = if entry.source_ip.starts_with("agent-command://") {
                agent_command_count += 1;
                "agent_command".to_string()
            } else if source_kind.as_deref() == Some("shell-history") {
                shell_history_count += 1;
                "shell_history".to_string()
            } else {
                format!("graph:host:{}", entry.hostname)
            };
            CorrelatedLogRow {
                entry: entry.into(),
                source_kind,
                discovery,
            }
        })
        .collect();

    let discovered: std::collections::HashSet<&str> =
        inputs.discovered_hosts.iter().map(String::as_str).collect();
    let heartbeat_summaries: Vec<db::HeartbeatWindowSummary> = summaries
        .into_iter()
        .filter(|s| discovered.contains(s.hostname.as_str()))
        .collect();

    Some(GraphSessionCorrelation {
        session_id,
        session_start,
        session_end,
        used_graph: inputs.used_graph,
        session_entity_keys: inputs.session_entity_keys,
        discovered_hosts: inputs.discovered_hosts,
        discovered_entities: inputs.discovered_entities,
        logs,
        agent_command_count,
        shell_history_count,
        heartbeat_summaries,
        truncated,
    })
}

impl CortexService {
    pub(super) async fn session_graph_correlation(
        &self,
        session: &AiSessionEntry,
        severity_min: Option<&str>,
    ) -> ServiceResult<Option<GraphSessionCorrelation>> {
        let levels = severity_at_or_above(severity_min.unwrap_or("info"))?;
        let session_id = session.session_id.clone();
        let sid = session_id.clone();
        let scope = db::SessionGraphScope {
            project: session.project.clone(),
            tool: session.tool.clone(),
            host: session.hostname.clone(),
            severity_in: levels,
        };
        let (inputs, summaries) = self
            .run_db("session_investigation_graph", move |pool| {
                let inputs = db::correlate_session_graph_scoped(
                    pool,
                    &sid,
                    &scope,
                    GRAPH_SESSION_LOG_LIMIT,
                )?;
                let summaries = match &inputs.bounds {
                    Some((start, end)) if inputs.used_graph => {
                        db::heartbeat_window_summaries(pool, start, end, None)?
                    }
                    _ => Vec::new(),
                };
                Ok((inputs, summaries))
            })
            .await?;
        Ok(build_graph_session_correlation(
            session_id, inputs, summaries,
        ))
    }
}
