use super::*;

/// Log fan-out cap for the graph-anchored session lane of `ai_correlate`.
/// Clamped again to `[1, 1000]` inside `db::correlate_session_graph`.
const GRAPH_SESSION_LOG_LIMIT: usize = 500;

/// Shape the DB-layer `SessionGraphInputs` into the API response, classifying
/// each row into a source lane (`agent_command` / `shell_history` /
/// `graph:host:<host>`) and counting the agent-command and shell-history lanes.
/// Heartbeat summaries are filtered to the discovered hosts. Returns `None` when
/// the session has no rows at all (empty bounds).
fn build_graph_session_correlation(
    session_id: String,
    inputs: db::SessionGraphInputs,
    summaries: Vec<db::HeartbeatWindowSummary>,
) -> Option<GraphSessionCorrelation> {
    let (session_start, session_end) = inputs.bounds?;

    let truncated = inputs.logs.len() >= GRAPH_SESSION_LOG_LIMIT;
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
    pub async fn list_sessions(
        &self,
        req: ListSessionsRequest,
    ) -> ServiceResult<ListSessionsResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        // The unbounded (no time-window) path reads from the periodically
        // refreshed rollup; expose its staleness so callers know the `as_of`.
        // Time-windowed queries run live, so no staleness applies.
        let unbounded = from.is_none() && to.is_none();
        let params = db::ListAiSessionsParams {
            ai_project: req.project,
            ai_tool: req.tool,
            host: req.host,
            since: from,
            until: to,
            limit: req.limit,
        };
        let (rows, rollup_as_of) = self
            .run_db("list_sessions", move |pool| {
                let rows = db::list_ai_sessions(pool, &params)?;
                // Only attach staleness when the rollup path was actually used
                // (unbounded query AND rollup populated). If unbounded but the
                // rollup was empty, list_ai_sessions fell back to live, so
                // report no staleness.
                let as_of = if unbounded {
                    db::ai_session_rollup_status(pool)?.refreshed_at
                } else {
                    None
                };
                Ok((rows, as_of))
            })
            .await?;
        let sessions: Vec<AiSessionEntry> = rows.into_iter().map(Into::into).collect();
        Ok(ListSessionsResponse {
            count: sessions.len(),
            sessions,
            rollup_as_of,
        })
    }

    pub async fn search_sessions(
        &self,
        req: SearchSessionsRequest,
    ) -> ServiceResult<SearchSessionsResponse> {
        self.search_sessions_with_limit_policy(req, None).await
    }

    pub async fn search_sessions_with_limit_policy(
        &self,
        mut req: SearchSessionsRequest,
        policy: Option<AiLimitPolicy>,
    ) -> ServiceResult<SearchSessionsResponse> {
        let limit_clamped_to = policy.and_then(|policy| {
            req.limit
                .filter(|limit| *limit > policy.limit_cap)
                .map(|_| policy)
        });
        if let Some(policy) = limit_clamped_to {
            req.limit = Some(policy.limit_cap);
        }
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let params = db::SearchAiSessionsParams {
            query: req.query,
            ai_project: req.project,
            ai_tool: req.tool,
            host: None,
            app: None,
            since: from,
            until: to,
            limit: req.limit,
        };
        let result = self
            .run_db("search_sessions", move |pool| {
                db::search_ai_sessions(pool, &params)
            })
            .await?;
        let mut response: SearchSessionsResponse = result.into();
        if let Some(policy) = limit_clamped_to.filter(|policy| policy.report_limit_clamp) {
            response.limit_clamped_to = Some(policy.limit_cap);
            response.truncated = true;
        }
        Ok(response)
    }

    pub async fn search_abuse(
        &self,
        req: AbuseSearchRequest,
    ) -> ServiceResult<AbuseSearchResponse> {
        self.search_abuse_with_limit_policy(req, None).await
    }

    pub async fn search_abuse_with_limit_policy(
        &self,
        mut req: AbuseSearchRequest,
        policy: Option<AiLimitPolicy>,
    ) -> ServiceResult<AbuseSearchResponse> {
        let limit_clamped_to = policy.and_then(|policy| {
            req.limit
                .filter(|limit| *limit > policy.limit_cap)
                .map(|_| policy)
        });
        if let Some(policy) = limit_clamped_to {
            req.limit = Some(policy.limit_cap);
        }
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let params = db::AiAbuseParams {
            ai_project: req.project,
            ai_tool: req.tool,
            since: from,
            until: to,
            limit: req.limit,
            before: req.before,
            after: req.after,
            terms: req.terms,
        };
        let result = self
            .run_db("search_abuse", move |pool| {
                db::search_ai_abuse(pool, &params)
            })
            .await?;
        let mut response: AbuseSearchResponse = result.into();
        if let Some(policy) = limit_clamped_to.filter(|policy| policy.report_limit_clamp) {
            response.limit_clamped_to = Some(policy.limit_cap);
            response.truncated = true;
        }
        Ok(response)
    }

    pub async fn list_ai_incidents(
        &self,
        req: AiIncidentRequest,
    ) -> ServiceResult<AiIncidentResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_db("list_ai_incidents", move |pool| {
                db::search_ai_incidents(
                    pool,
                    &db::AiIncidentParams {
                        ai_project: req.project,
                        ai_tool: req.tool,
                        since: from,
                        until: to,
                        limit: req.limit,
                        window_minutes: req.window_minutes,
                        terms: req.terms,
                    },
                )
            })
            .await?;
        Ok(AiIncidentResponse {
            incidents: result.incidents.into_iter().map(Into::into).collect(),
            total_incidents: result.total_incidents,
            candidate_rows: result.candidate_rows,
            candidate_cap: result.candidate_cap,
            candidate_window_truncated: result.candidate_window_truncated,
            truncated: result.truncated,
        })
    }

    pub async fn investigate_ai_incidents(
        &self,
        req: AiInvestigateRequest,
    ) -> ServiceResult<AiInvestigateResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_heavy_db("investigate_ai_incidents", move |pool| {
                db::investigate_ai_incidents(
                    pool,
                    &db::AiInvestigateParams {
                        incident_id: req.incident_id,
                        ai_project: req.project,
                        ai_tool: req.tool,
                        since: from,
                        until: to,
                        limit: req.limit,
                        window_minutes: req.window_minutes,
                        correlation_window_minutes: req.correlation_window_minutes,
                        terms: req.terms,
                    },
                )
            })
            .await?;
        Ok(AiInvestigateResponse {
            evidence: result.evidence.into_iter().map(Into::into).collect(),
            total_incidents: result.total_incidents,
            truncated: result.truncated,
        })
    }

    pub async fn correlate_ai_logs(
        &self,
        req: AiCorrelateRequest,
    ) -> ServiceResult<AiCorrelateResponse> {
        self.correlate_ai_logs_with_limit_policy(req, AiCorrelateLimitPolicy::MCP)
            .await
    }

    pub async fn correlate_ai_logs_with_limit_policy(
        &self,
        req: AiCorrelateRequest,
        policy: AiCorrelateLimitPolicy,
    ) -> ServiceResult<AiCorrelateResponse> {
        let (req, events_per_anchor_clamped_to) = req.normalize_limits(policy);
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let window = req.window_minutes.unwrap_or(5).clamp(1, 120);
        let related_limit = req
            .events_per_anchor
            .unwrap_or(25)
            .clamp(1, policy.events_per_anchor_cap);
        let anchor_limit = req.limit.unwrap_or(10).clamp(1, 50);
        let severity_min = req.severity_min.unwrap_or_else(|| "info".into());
        let severity_levels = severity_at_or_above(&severity_min)?;

        // Both DB calls share one connection snapshot inside a single spawn_blocking
        // boundary, eliminating the double cross-thread wakeup and the intermediate
        // allocation that was required to transfer anchor rows back to the async task
        // just to build window params and re-enter spawn_blocking.
        // Session-anchored graph correlation runs when the caller targets a
        // specific session; capture the id before `req` is consumed below.
        let session_for_graph = req.session_id.clone();

        let anchor_params = db::AiCorrelateParams {
            ai_project: req.project,
            ai_tool: req.tool,
            ai_session_id: req.session_id,
            ai_query: req.ai_query,
            since: from,
            until: to,
            limit: Some(anchor_limit),
        };
        let log_query = req.log_query;
        let hostname = req.host;
        let source_ip = req.source;
        let app_name = req.app;

        type CorrelateDbResult = (
            bool,
            Vec<(db::LogEntry, String, String)>,
            Vec<db::AiRelatedLogsForAnchor>,
        );
        let (anchors_truncated, anchor_entries, related_by_anchor) = self
            .run_db(
                "correlate_ai_logs",
                move |pool| -> anyhow::Result<CorrelateDbResult> {
                    let mut anchors = db::search_ai_anchors(pool, &anchor_params)?;
                    let anchors_truncated = anchors.len() > anchor_limit as usize;
                    anchors.truncate(anchor_limit as usize);

                    let delta = TimeDelta::try_minutes(i64::from(window))
                        .ok_or_else(|| anyhow::anyhow!("window_minutes overflow"))?;

                    let mut anchor_entries = Vec::with_capacity(anchors.len());
                    let mut windows = Vec::with_capacity(anchors.len());
                    for (anchor_index, anchor) in anchors.into_iter().enumerate() {
                        let ref_dt =
                            parse_required_timestamp(&anchor.timestamp, "anchor.timestamp")
                                .map_err(anyhow::Error::from)?;
                        let window_from = rfc3339_z(ref_dt - delta);
                        let window_to = rfc3339_z(ref_dt + delta);
                        windows.push(db::AiRelatedWindow {
                            anchor_index,
                            anchor_time: anchor.timestamp.clone(),
                            window_from: window_from.clone(),
                            window_to: window_to.clone(),
                        });
                        anchor_entries.push((anchor, window_from, window_to));
                    }

                    let related_params = db::AiRelatedLogsParams {
                        windows,
                        query: log_query,
                        host: hostname,
                        source: source_ip,
                        severity_in: severity_levels,
                        app: app_name,
                        limit_per_anchor: related_limit,
                    };
                    let related_by_anchor = db::search_ai_related_logs(pool, &related_params)?;
                    Ok((anchors_truncated, anchor_entries, related_by_anchor))
                },
            )
            .await?;

        let mut by_anchor: std::collections::HashMap<usize, db::AiRelatedLogsForAnchor> =
            related_by_anchor
                .into_iter()
                .map(|group| (group.anchor_index, group))
                .collect();

        let mut correlated = Vec::with_capacity(anchor_entries.len());
        let mut total_related_events = 0usize;
        for (anchor_index, (anchor, window_from, window_to)) in
            anchor_entries.into_iter().enumerate()
        {
            let (related_logs, related_truncated) = by_anchor
                .remove(&anchor_index)
                .map(|group| (group.logs, group.truncated))
                .unwrap_or((Vec::new(), false));
            total_related_events += related_logs.len();
            correlated.push(AiCorrelationAnchor {
                entry: anchor.into(),
                window_from,
                window_to,
                related: related_logs.into_iter().map(Into::into).collect(),
                related_truncated,
            });
        }

        // Graph-anchored lane: traverse from the session entity, fan logs out
        // across all source kinds within the session window, and attach
        // heartbeat pressure for the discovered hosts. Additive — only when a
        // concrete session id was supplied.
        let graph_correlation = match session_for_graph {
            Some(session_id) => {
                let sid = session_id.clone();
                let (inputs, summaries) = self
                    .run_db(
                        "correlate_session_graph",
                        move |pool| -> anyhow::Result<(
                            db::SessionGraphInputs,
                            Vec<db::HeartbeatWindowSummary>,
                        )> {
                            let inputs =
                                db::correlate_session_graph(pool, &sid, GRAPH_SESSION_LOG_LIMIT)?;
                            let summaries = match &inputs.bounds {
                                Some((start, end)) if !inputs.discovered_hosts.is_empty() => {
                                    db::heartbeat_window_summaries(pool, start, end, None)?
                                }
                                _ => Vec::new(),
                            };
                            Ok((inputs, summaries))
                        },
                    )
                    .await?;
                build_graph_session_correlation(session_id, inputs, summaries)
            }
            None => None,
        };

        Ok(AiCorrelateResponse {
            window_minutes: window,
            severity_min,
            total_anchors: correlated.len(),
            anchor_rows: correlated.len(),
            anchor_limit: anchor_limit as usize,
            anchors_truncated,
            related_limit_per_anchor: related_limit as usize,
            total_related_events,
            anchors: correlated,
            events_per_anchor_clamped_to,
            graph_correlation,
        })
    }

    pub async fn usage_blocks(
        &self,
        req: UsageBlocksRequest,
    ) -> ServiceResult<UsageBlocksResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let params = db::AiUsageBlocksParams {
            ai_project: req.project,
            ai_tool: req.tool,
            since: from,
            until: to,
            limit: req.limit,
        };
        let result = self
            .run_db("usage_blocks", move |pool| {
                db::get_ai_usage_blocks(pool, &params)
            })
            .await?;
        Ok(result.into())
    }

    pub async fn project_context(
        &self,
        req: ProjectContextRequest,
    ) -> ServiceResult<ProjectContextResponse> {
        let params = db::AiProjectContextParams {
            project: req.project,
            ai_tool: req.tool,
            limit: req.limit,
        };
        let result = self
            .run_heavy_db("project_context", move |pool| {
                db::get_ai_project_context(pool, &params)
            })
            .await?;
        Ok(result.into())
    }

    pub async fn list_ai_tools(
        &self,
        req: ListAiToolsRequest,
    ) -> ServiceResult<ListAiToolsResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let params = db::ListAiToolsParams {
            ai_project: req.project,
            since: from,
            until: to,
        };
        let result = self
            .run_db("list_ai_tools", move |pool| {
                db::list_ai_tools(pool, &params)
            })
            .await?;
        Ok(result.into())
    }

    pub async fn list_ai_projects(
        &self,
        req: ListAiProjectsRequest,
    ) -> ServiceResult<ListAiProjectsResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let params = db::ListAiProjectsParams {
            ai_tool: req.tool,
            since: from,
            until: to,
        };
        let result = self
            .run_db("list_ai_projects", move |pool| {
                db::list_ai_projects(pool, &params)
            })
            .await?;
        Ok(result.into())
    }
}

#[cfg(test)]
#[path = "ai_correlate_tests.rs"]
mod ai_correlate_tests;
