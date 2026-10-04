//! Service-layer methods for `skill_incidents`/`skill_investigate`. Split out
//! of `ai.rs` (which was pushed over the 500-line module-size gate by these
//! additions) following this repo's convention of splitting `impl
//! CortexService` blocks across sibling files under `src/app/services/` (see
//! `incidents.rs`).

use super::*;

impl CortexService {
    pub async fn list_ai_skill_incidents(
        &self,
        req: AiSkillIncidentRequest,
    ) -> ServiceResult<AiSkillIncidentResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_heavy_db("list_ai_skill_incidents", move |pool| {
                db::search_ai_skill_incidents(
                    pool,
                    &db::AiSkillIncidentParams {
                        skill: req.skill,
                        plugin: req.plugin,
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
        Ok(AiSkillIncidentResponse {
            incidents: result.incidents.into_iter().map(Into::into).collect(),
            total_incidents: result.total_incidents,
            candidate_event_rows: result.candidate_event_rows,
            candidate_cap: result.candidate_cap,
            candidate_window_truncated: result.candidate_window_truncated,
            truncated: result.truncated,
        })
    }

    /// Investigate skill-usage incidents. When the caller filters by `skill`
    /// or `plugin` (and does not pin an exact `incident_id`), this resolves
    /// "skill-first": it looks up all matching incidents (uncapped up to the
    /// incident-list cap), returns only the top-priority one(s) in
    /// `evidence` (count controlled by `limit`, default 1), and summarizes
    /// the remainder into `other_matching_incidents`. A single zero-signal
    /// bundle is still returned (never an error) but flagged via
    /// `no_incident_low_severity_summary`. For the plain `incident_id`-only
    /// or unfiltered path, `limit` means "how many incidents to
    /// investigate", matching `investigate_ai_incidents`.
    pub async fn investigate_ai_skill_incidents(
        &self,
        req: AiSkillInvestigateRequest,
    ) -> ServiceResult<AiSkillInvestigateResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let skill_first =
            req.incident_id.is_none() && (req.skill.is_some() || req.plugin.is_some());
        let requested_limit = req.limit;

        // Skill-first path: look up ALL matching incidents first (uncapped up
        // to the incident-list cap) so we can rank by priority and report
        // the ones we are not returning as `other_matching_incidents`.
        let lookup_limit = if skill_first {
            Some(100)
        } else {
            requested_limit
        };

        let result = self
            .run_heavy_db("investigate_ai_skill_incidents", {
                let req_incident_id = req.incident_id.clone();
                let req_skill = req.skill.clone();
                let req_plugin = req.plugin.clone();
                let req_tool = req.tool.clone();
                let req_project = req.project.clone();
                let window_minutes = req.window_minutes;
                let correlation_window_minutes = req.correlation_window_minutes;
                move |pool| {
                    db::investigate_ai_skill_incidents(
                        pool,
                        &db::AiSkillInvestigateParams {
                            incident_id: req_incident_id,
                            skill: req_skill,
                            plugin: req_plugin,
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
                "drop --plugin and filter by --skill only".to_string(),
                "run `cortex sessions skillincidents` with no filters to see what skills have \
                 events"
                    .to_string(),
            ];
            return Ok(AiSkillInvestigateResponse {
                evidence: Vec::new(),
                total_incidents: 0,
                truncated: result.truncated,
                other_matching_incidents: Vec::new(),
                no_incident_low_severity_summary: false,
                no_data: true,
                suggested_filters,
            });
        }

        let mut evidence: Vec<SkillIncidentEvidence> =
            result.evidence.into_iter().map(Into::into).collect();

        // Already sorted by priority_score desc, last_seen desc
        // (search_ai_skill_incidents guarantees this via total_cmp). For the
        // skill-first path, slice to the requested count (default 1) and
        // summarize the rest into other_matching_incidents.
        let mut other_matching_incidents = Vec::new();
        let mut no_incident_low_severity_summary = false;

        if skill_first {
            let keep = requested_limit.unwrap_or(1).max(1) as usize;
            // A bundle with zero signals present is "low signal" — still
            // return it (never an error), but flag it.
            if evidence.len() == 1 {
                let inc = &evidence[0].incident;
                if inc.signals_present.is_empty() {
                    no_incident_low_severity_summary = true;
                }
            }
            if evidence.len() > keep {
                other_matching_incidents = evidence[keep..]
                    .iter()
                    .map(|bundle| SkillIncidentSummary {
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

        Ok(AiSkillInvestigateResponse {
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

#[cfg(test)]
#[path = "incident_cutoff_tests.rs"]
mod tests;
