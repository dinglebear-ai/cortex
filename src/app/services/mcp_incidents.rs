//! Service-layer methods for `mcp_incidents`/`mcp_investigate`. Mirrors
//! `src/app/services/skill_incidents.rs` (same skill/plugin-first ->
//! server/tool-first shape), keyed on MCP server/tool instead of skill
//! name/plugin.

use super::*;

impl CortexService {
    pub async fn list_ai_mcp_incidents(
        &self,
        req: AiMcpIncidentRequest,
    ) -> ServiceResult<AiMcpIncidentResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_heavy_db("list_ai_mcp_incidents", move |pool| {
                db::search_ai_mcp_incidents(
                    pool,
                    &db::AiMcpIncidentParams {
                        mcp_server: req.mcp_server,
                        mcp_tool: req.mcp_tool,
                        tool_name: req.tool_name,
                        ai_tool: req.tool,
                        ai_project: req.project,
                        ai_session_id: req.session_id,
                        hostname: req.hostname,
                        since: from,
                        until: to,
                        incident_id: None,
                        limit: req.limit,
                        window_minutes: req.window_minutes,
                        signals: req.signals,
                        min_score: req.min_score,
                    },
                )
            })
            .await?;
        Ok(AiMcpIncidentResponse {
            incidents: result.incidents.into_iter().map(Into::into).collect(),
            total_incidents: result.total_incidents,
            candidate_event_rows: result.candidate_event_rows,
            candidate_cap: result.candidate_cap,
            candidate_window_truncated: result.candidate_window_truncated,
            truncated: result.truncated,
        })
    }

    /// Investigate MCP incidents. When the caller filters by `mcp_server`,
    /// `mcp_tool`, or `tool_name` (and does not pin an exact
    /// `incident_id`), this resolves "server/tool-first": it looks up all
    /// matching incidents (uncapped up to the incident-list cap), returns
    /// only the top-priority one(s) in `evidence` (count controlled by
    /// `limit`, default 1), and summarizes the remainder into
    /// `other_matching_incidents`. A single zero-signal bundle is still
    /// returned (never an error) but flagged via
    /// `no_incident_low_severity_summary`.
    pub async fn investigate_ai_mcp_incidents(
        &self,
        req: AiMcpInvestigateRequest,
    ) -> ServiceResult<AiMcpInvestigateResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let target_first = req.incident_id.is_none()
            && (req.mcp_server.is_some() || req.mcp_tool.is_some() || req.tool_name.is_some());
        let requested_limit = req.limit;

        let lookup_limit = if target_first {
            Some(100)
        } else {
            requested_limit
        };

        let result = self
            .run_heavy_db("investigate_ai_mcp_incidents", {
                let req_incident_id = req.incident_id.clone();
                let req_mcp_server = req.mcp_server.clone();
                let req_mcp_tool = req.mcp_tool.clone();
                let req_tool_name = req.tool_name.clone();
                let req_tool = req.tool.clone();
                let req_project = req.project.clone();
                let window_minutes = req.window_minutes;
                let correlation_window_minutes = req.correlation_window_minutes;
                move |pool| {
                    db::investigate_ai_mcp_incidents(
                        pool,
                        &db::AiMcpInvestigateParams {
                            incident_id: req_incident_id,
                            mcp_server: req_mcp_server,
                            mcp_tool: req_mcp_tool,
                            tool_name: req_tool_name,
                            ai_tool: req_tool,
                            ai_project: req_project,
                            since: from,
                            until: to,
                            limit: lookup_limit,
                            window_minutes,
                            correlation_window_minutes,
                        },
                    )
                }
            })
            .await?;

        let no_data = result.evidence.is_empty() && result.total_incidents == 0;
        if no_data {
            let suggested_filters = vec![
                "widen --since (e.g. --since 30d)".to_string(),
                "drop --mcp-tool and filter by --mcp-server only".to_string(),
                "run `cortex sessions mcpincidents` with no filters to see what MCP servers \
                 have events"
                    .to_string(),
            ];
            return Ok(AiMcpInvestigateResponse {
                evidence: Vec::new(),
                total_incidents: 0,
                truncated: result.truncated,
                other_matching_incidents: Vec::new(),
                no_incident_low_severity_summary: false,
                no_data: true,
                suggested_filters,
            });
        }

        let mut evidence: Vec<McpIncidentEvidence> =
            result.evidence.into_iter().map(Into::into).collect();

        let mut other_matching_incidents = Vec::new();
        let mut no_incident_low_severity_summary = false;

        if target_first {
            let keep = requested_limit.unwrap_or(1).max(1) as usize;
            if evidence.len() == 1 {
                let inc = &evidence[0].incident;
                if inc.signals_present.is_empty() {
                    no_incident_low_severity_summary = true;
                }
            }
            if evidence.len() > keep {
                other_matching_incidents = evidence[keep..]
                    .iter()
                    .map(|bundle| McpIncidentSummary {
                        incident_id: bundle.incident.incident_id.clone(),
                        first_seen: bundle.incident.first_seen.clone(),
                        last_seen: bundle.incident.last_seen.clone(),
                        priority_score: bundle.incident.priority_score,
                        priority_label: bundle.incident.priority_label.clone(),
                    })
                    .collect();
                evidence.truncate(keep);
            }
        }

        Ok(AiMcpInvestigateResponse {
            total_incidents: result.total_incidents,
            truncated: result.truncated,
            evidence,
            other_matching_incidents,
            no_incident_low_severity_summary,
            no_data: false,
            suggested_filters: Vec::new(),
        })
    }
}
