//! Service-layer skill assessment: calls PR 3's
//! `CortexService::investigate_ai_skill_incidents` to resolve a skill (or
//! plugin) name to its highest-priority (or all, with `--all`) matching
//! `SkillIncidentEvidence` bundle(s), and optionally runs the guarded
//! Gemini assessment through PR 1's `LlmRunner` using the
//! `skill-improvement-assessment` skill prompt
//! (`crate::skill_assessment::build_skill_assessment_prompt`).
//!
//! This module does NOT reimplement Gemini process spawning, an audit
//! table, or a skill-incident schema — all three already exist upstream
//! (PR 1's `LlmRunner`, PR 3's `investigate_ai_skill_incidents`). It also
//! does NOT fall back to the AI-transcript abuse-incident pipeline for
//! skill evidence; that was an earlier-draft workaround made obsolete by
//! PR 3 landing.
use super::*;
use crate::app::llm_runner::{LlmCallerSurface, LlmEvidenceCounts, LlmInvocationSpec};
use crate::app::models::{
    AiSkillInvestigateRequest, SkillAssessRequest, SkillAssessResponse, SkillAssessResult,
    SkillIncidentEvidence,
};
use crate::assessment::GeminiAssessConfig;
use crate::skill_assessment::build_skill_assessment_prompt;

impl CortexService {
    pub async fn run_skill_assessment(
        &self,
        req: SkillAssessRequest,
    ) -> ServiceResult<SkillAssessResponse> {
        self.run_skill_assessment_with_delta(req, true, |_| Ok(()))
            .await
    }

    /// `run_llm = false` skips the `LlmRunner::run` call entirely and
    /// returns only deterministic findings — this is the path MCP/REST
    /// callers MUST use (see Task 9's MCP-safety test) and the path the
    /// CLI uses when `--no-llm` is passed (Task 6).
    ///
    /// Both `skill` and `plugin` forward directly into
    /// `AiSkillInvestigateRequest` — PR 3's `investigate_ai_skill_incidents`
    /// natively supports plugin-level (all skills under a plugin) lookup, so
    /// no synthetic identifier encoding is needed here (see the regression
    /// test locking this contract in,
    /// `plugin_only_request_forwards_plugin_to_investigate_ai_skill_incidents`).
    pub async fn run_skill_assessment_with_delta<F>(
        &self,
        req: SkillAssessRequest,
        run_llm: bool,
        mut on_delta: F,
    ) -> ServiceResult<SkillAssessResponse>
    where
        F: FnMut(&str) -> anyhow::Result<()> + Send,
    {
        if req.skill.is_none() && req.plugin.is_none() {
            return Err(ServiceError::InvalidInput(
                "assess skill requires either a skill name or --plugin".to_string(),
            ));
        }

        let keep_limit = if req.all {
            req.limit
        } else {
            Some(req.limit.unwrap_or(1).max(1))
        };
        let invest_req = AiSkillInvestigateRequest {
            incident_id: None,
            skill: req.skill.clone(),
            plugin: req.plugin.clone(),
            tool: req.tool.clone(),
            project: req.project.clone(),
            since: req.since.clone(),
            until: req.until.clone(),
            limit: keep_limit,
            window_minutes: req.window_minutes,
            correlation_window_minutes: req.correlation_window_minutes,
        };
        let invest_resp = self.investigate_ai_skill_incidents(invest_req).await?;

        if invest_resp.no_data || invest_resp.evidence.is_empty() {
            let skill_desc = req
                .skill
                .clone()
                .or_else(|| req.plugin.clone().map(|p| format!("plugin:{p}")))
                .unwrap_or_default();
            return Err(ServiceError::InvalidInput(format!(
                "no skill incident found for '{skill_desc}'; try a wider --since/--until window or verify the skill/plugin name"
            )));
        }

        let gemini_config =
            GeminiAssessConfig::from_env(req.model.clone(), self.llm().timeout_secs());
        let mut results = Vec::with_capacity(invest_resp.evidence.len());
        for evidence in &invest_resp.evidence {
            let mut result = SkillAssessResult {
                incident_id: evidence.incident.incident_id.clone(),
                findings: evidence.findings.clone(),
                assessment: None,
                prompt_preview: None,
            };
            if run_llm {
                result = self
                    .run_one_skill_assessment(evidence, &gemini_config, &mut on_delta)
                    .await?;
            }
            results.push(result);
        }

        Ok(SkillAssessResponse {
            skill: req.skill,
            plugin: req.plugin,
            results,
            total_incidents: invest_resp.total_incidents,
            other_matching_incidents: invest_resp.other_matching_incidents,
            no_incident_low_severity_summary: invest_resp.no_incident_low_severity_summary,
        })
    }

    /// Runs one guarded Gemini assessment for a single `SkillIncidentEvidence`
    /// bundle via `LlmRunner::run`, forwarding deltas directly through the
    /// borrowed callback without an intermediate allocation or channel.
    async fn run_one_skill_assessment<F>(
        &self,
        evidence: &SkillIncidentEvidence,
        gemini_config: &GeminiAssessConfig,
        on_delta: &mut F,
    ) -> ServiceResult<SkillAssessResult>
    where
        F: FnMut(&str) -> anyhow::Result<()> + Send,
    {
        let evidence_json = serde_json::to_string_pretty(evidence)
            .map_err(|e| ServiceError::Internal(anyhow::anyhow!("json serialize failed: {e}")))?;
        let prompt = build_skill_assessment_prompt(&evidence_json);
        let prompt_preview: String = prompt.chars().take(500).collect();

        let spec = LlmInvocationSpec {
            caller_surface: LlmCallerSurface::Cli, // skill assessment is CLI-only (safety invariant)
            action: "skill_assess".to_string(),
            incident_id: Some(evidence.incident.incident_id.clone()),
            ai_tool: Some(evidence.incident.tool.clone()),
            ai_project: Some(evidence.incident.project.clone()),
            ai_session_id: Some(evidence.incident.session_id.clone()),
            evidence_counts: LlmEvidenceCounts {
                total_incidents: 1,
                evidence_bundle_count: 1,
                total_anchors: evidence.signal_anchors.len(),
                truncated: evidence.signal_anchors_truncated
                    || evidence.transcript_before_truncated
                    || evidence.transcript_after_truncated,
            },
            prompt,
            provider: "gemini-cli".to_string(),
            model: gemini_config.model.clone(),
            program: gemini_config.program.clone(),
            extra_metadata: serde_json::json!({ "skill_name": evidence.incident.skill_name }),
        };
        let output =
            super::run_gemini_with_delta(self.llm(), spec, gemini_config, on_delta).await?;

        Ok(SkillAssessResult {
            incident_id: evidence.incident.incident_id.clone(),
            findings: evidence.findings.clone(),
            assessment: Some(output),
            prompt_preview: Some(prompt_preview),
        })
    }
}

#[cfg(test)]
#[path = "skill_assessment_tests.rs"]
mod tests;
