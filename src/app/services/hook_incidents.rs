//! Service-layer methods for `hook_incidents`/`hook_investigate`. Mirrors
//! `super::skill_incidents`, keyed on hook usage instead of skill usage.

use super::*;

impl CortexService {
    pub async fn list_ai_hook_incidents(
        &self,
        req: AiHookIncidentRequest,
    ) -> ServiceResult<AiHookIncidentResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_heavy_db("list_ai_hook_incidents", move |pool| {
                db::search_ai_hook_incidents(
                    pool,
                    &db::AiHookIncidentParams {
                        hook_event: req.hook_event,
                        hook_name: req.hook_name,
                        hook_source: req.hook_source,
                        ai_tool: req.tool,
                        ai_project: req.project,
                        ai_session_id: req.session_id,
                        hostname: req.hostname,
                        evidence_kind: req.evidence_kind,
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
        Ok(AiHookIncidentResponse {
            incidents: result.incidents.into_iter().map(Into::into).collect(),
            total_incidents: result.total_incidents,
            candidate_event_rows: result.candidate_event_rows,
            candidate_cap: result.candidate_cap,
            candidate_window_truncated: result.candidate_window_truncated,
            truncated: result.truncated,
        })
    }

    /// Investigate hook-usage incidents. When the caller filters by
    /// `hook_event`/`hook_name` (and does not pin an exact `incident_id`),
    /// this resolves "hook-first": it looks up all matching incidents
    /// (uncapped up to the incident-list cap), returns only the top-priority
    /// one(s) in `evidence` (count controlled by `limit`, default 1), and
    /// summarizes the remainder into `other_matching_incidents`. A single
    /// zero-signal bundle is still returned (never an error) but flagged via
    /// `no_incident_low_severity_summary`. Mirrors
    /// `investigate_ai_skill_incidents`.
    pub async fn investigate_ai_hook_incidents(
        &self,
        req: AiHookInvestigateRequest,
    ) -> ServiceResult<AiHookInvestigateResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let hook_first = req.incident_id.is_none()
            && (req.hook_event.is_some() || req.hook_name.is_some() || req.hook_source.is_some());
        let requested_limit = req.limit;

        let lookup_limit = if hook_first {
            Some(100)
        } else {
            requested_limit
        };

        let result = self
            .run_heavy_db("investigate_ai_hook_incidents", {
                let req_incident_id = req.incident_id.clone();
                let req_hook_event = req.hook_event.clone();
                let req_hook_name = req.hook_name.clone();
                let req_hook_source = req.hook_source.clone();
                let req_tool = req.tool.clone();
                let req_project = req.project.clone();
                let window_minutes = req.window_minutes;
                let correlation_window_minutes = req.correlation_window_minutes;
                move |pool| {
                    db::investigate_ai_hook_incidents(
                        pool,
                        &db::AiHookInvestigateParams {
                            incident_id: req_incident_id,
                            hook_event: req_hook_event,
                            hook_name: req_hook_name,
                            hook_source: req_hook_source,
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
                "drop --hook and list all hook incidents".to_string(),
                "run `cortex sessions hookevents` with no filters to see what hooks have events"
                    .to_string(),
            ];
            return Ok(AiHookInvestigateResponse {
                evidence: Vec::new(),
                total_incidents: 0,
                truncated: result.truncated,
                other_matching_incidents: Vec::new(),
                no_incident_low_severity_summary: false,
                no_data: true,
                suggested_filters,
            });
        }

        let mut evidence: Vec<HookIncidentEvidence> =
            result.evidence.into_iter().map(Into::into).collect();

        let mut other_matching_incidents = Vec::new();
        let mut no_incident_low_severity_summary = false;

        if hook_first {
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
                    .map(|bundle| HookIncidentSummary {
                        incident_id: bundle.incident.incident_id.clone(),
                        first_seen: bundle.incident.first_seen.clone(),
                        last_seen: bundle.incident.last_seen.clone(),
                        priority_score: bundle.incident.priority_score,
                        priority_label: bundle.incident.priority_label.clone(),
                        has_runtime_evidence: bundle.incident.has_runtime_evidence,
                    })
                    .collect();
                evidence.truncate(keep);
            }
        }

        Ok(AiHookInvestigateResponse {
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
