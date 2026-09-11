//! `cortex reflect`: index local transcripts, list skill/MCP/hook incidents,
//! rank them together, and assess the top N. Each top incident is
//! investigated once (incident id plus its exact target); its findings come
//! from that evidence, and the evidence goes to the kind's existing
//! per-evidence LLM helper. Orchestration only: no new detection or prompts.

use std::collections::BTreeMap;

use super::reflect_llm::{ReflectLlmFailure, classify_llm_failure, program_on_path};
use super::*;
use crate::app::models::{
    AiHookIncidentRequest, AiHookInvestigateRequest, AiMcpIncidentRequest, AiMcpInvestigateRequest,
    AiSkillIncidentRequest, AiSkillInvestigateRequest, ReflectAssessed, ReflectIncident,
    ReflectIndexSummary, ReflectKind, ReflectKindListing, ReflectMode, ReflectReport,
    ReflectRequest, rank_reflect_incidents, summarize_reflect_incidents,
};
use crate::llm_backend::LlmBackend;

/// The incident list services clamp `limit` to 1..=100.
const REFLECT_LIST_LIMIT: u32 = 100;

pub(crate) const INCIDENT_CHANGED: &str =
    "incident changed during the run (the database was written while reflect ran); rerun reflect";

pub(super) enum AssessOutcome {
    Done {
        assessment: Option<String>,
        findings: serde_json::Value,
    },
    LlmError {
        error: ServiceError,
        findings: serde_json::Value,
    },
    Missing,
}

/// Lists one kind's incidents and returns (incidents, total, truncated).
macro_rules! list_kind {
    ($self:ident, $req:ident, $list:ident, $Req:ident) => {{
        let response = $self
            .$list($Req {
                tool: $req.tool.clone(),
                project: $req.project.clone(),
                since: $req.since.clone(),
                until: $req.until.clone(),
                limit: Some(REFLECT_LIST_LIMIT),
                ..Default::default()
            })
            .await?;
        let truncated = response.truncated || response.candidate_window_truncated;
        let total = response.total_incidents;
        let incidents: Vec<ReflectIncident> = response
            .incidents
            .into_iter()
            .map(ReflectIncident::from)
            .collect();
        (incidents, total, truncated)
    }};
}

/// Investigates one incident by id and exact target, then optionally runs
/// the per-evidence LLM helper. Evaluates to an `AssessOutcome`; returns
/// early from the enclosing fn on non-NotFound investigate errors.
macro_rules! assess_kind {
    ($self:ident, $incident:ident, $req:ident, $backend:ident, $investigate:ident, $run_one:ident,
     $Req:ident { $($field:ident : $value:expr),* $(,)? }) => {{
        let investigated = $self
            .$investigate($Req {
                incident_id: Some($incident.incident_id.clone()),
                tool: $req.tool.clone(),
                project: $req.project.clone(),
                since: $req.since.clone(),
                until: $req.until.clone(),
                limit: Some(1),
                $($field: $value,)*
                ..Default::default()
            })
            .await;
        let response = match investigated {
            Ok(response) => response,
            Err(ServiceError::NotFound(_)) => return Ok(AssessOutcome::Missing),
            Err(error) => return Err(error),
        };
        match response.evidence.into_iter().next() {
            None => AssessOutcome::Missing,
            Some(evidence) => {
                let findings = findings_json(&evidence.findings)?;
                match $backend {
                    None => AssessOutcome::Done { assessment: None, findings },
                    Some(backend) => {
                        let mut ignore = |_: &str| -> anyhow::Result<()> { Ok(()) };
                        match $self.$run_one(&evidence, backend, &mut ignore).await {
                            Ok(result) => AssessOutcome::Done { assessment: result.assessment, findings },
                            Err(error) => AssessOutcome::LlmError { error, findings },
                        }
                    }
                }
            }
        }
    }};
}

impl CortexService {
    pub async fn run_reflect<P>(
        &self,
        mut req: ReflectRequest,
        mut progress: P,
    ) -> ServiceResult<ReflectReport>
    where
        P: FnMut(&str) + Send,
    {
        if req.kinds.is_empty() {
            return Err(ServiceError::InvalidInput(
                "reflect requires at least one kind: skill, mcp, or hook".to_string(),
            ));
        }
        // Pin the window end so listing and assessment see the same incidents.
        if req.until.is_none() {
            req.until =
                Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }

        let index = if req.index {
            progress(&format!(
                "indexing AI transcripts modified since {} (the first run can take a while)",
                req.since.as_deref().unwrap_or("the beginning")
            ));
            let result = self.index_ai_roots(None, false, req.since.clone()).await?;
            let summary = ReflectIndexSummary::from(&result);
            progress(&format!(
                "indexed {} new records from {} files ({} parse errors)",
                summary.ingested, summary.discovered_files, summary.parse_errors
            ));
            Some(summary)
        } else {
            None
        };

        let mut incidents = Vec::new();
        let mut listings = Vec::with_capacity(req.kinds.len());
        for &kind in &req.kinds {
            progress(&format!("detecting {} incidents", kind.as_str()));
            let (found, total, truncated) = match kind {
                ReflectKind::Skill => {
                    list_kind!(self, req, list_ai_skill_incidents, AiSkillIncidentRequest)
                }
                ReflectKind::Mcp => {
                    list_kind!(self, req, list_ai_mcp_incidents, AiMcpIncidentRequest)
                }
                ReflectKind::Hook => {
                    list_kind!(self, req, list_ai_hook_incidents, AiHookIncidentRequest)
                }
            };
            listings.push(ReflectKindListing {
                kind,
                total,
                truncated,
            });
            incidents.extend(found);
        }
        let mut ranked = rank_reflect_incidents(incidents);
        let summary = summarize_reflect_incidents(&listings, &ranked);
        let take = (req.max_assess as usize).min(ranked.len());
        let unassessed = ranked.split_off(take);

        let mut mode = if req.run_llm {
            ReflectMode::ReportAndLlm
        } else {
            ReflectMode::ReportOnly
        };
        let mut llm_fallback_reason = None;
        let mut backend = None;
        if mode == ReflectMode::ReportAndLlm && take > 0 {
            match self.reflect_llm_backend() {
                Ok(resolved) => backend = Some(resolved),
                Err(reason) => {
                    mode = ReflectMode::ReportOnly;
                    llm_fallback_reason = Some(reason);
                }
            }
        }

        let mut blocked: BTreeMap<ReflectKind, String> = BTreeMap::new();
        let mut assessed = Vec::with_capacity(take);
        for (position, incident) in ranked.into_iter().enumerate() {
            progress(&format!(
                "assessing {}/{take}: {} {}",
                position + 1,
                incident.kind.as_str(),
                incident.target
            ));
            let kind_blocked = blocked.get(&incident.kind).cloned();
            let use_backend = if kind_blocked.is_none() {
                backend.as_ref()
            } else {
                None
            };
            let entry = match self
                .assess_reflect_incident(&incident, &req, use_backend)
                .await?
            {
                AssessOutcome::Done {
                    assessment,
                    findings,
                } => ReflectAssessed {
                    incident,
                    assessment,
                    findings,
                    failure: kind_blocked,
                },
                AssessOutcome::Missing => ReflectAssessed {
                    incident,
                    assessment: None,
                    findings: serde_json::Value::Null,
                    failure: Some(INCIDENT_CHANGED.to_string()),
                },
                AssessOutcome::LlmError { error, findings } => {
                    let failure = match classify_llm_failure(&error) {
                        ReflectLlmFailure::NotLlm => return Err(error),
                        ReflectLlmFailure::Unavailable(reason) => {
                            backend = None;
                            llm_fallback_reason = Some(reason.clone());
                            reason
                        }
                        ReflectLlmFailure::KindBlocked(reason) => {
                            blocked.insert(incident.kind, reason.clone());
                            reason
                        }
                        ReflectLlmFailure::Failed(reason) => reason,
                    };
                    ReflectAssessed {
                        incident,
                        assessment: None,
                        findings,
                        failure: Some(failure),
                    }
                }
            };
            assessed.push(entry);
        }
        if llm_fallback_reason.is_some() && assessed.iter().all(|entry| entry.assessment.is_none())
        {
            mode = ReflectMode::ReportOnly;
        }

        Ok(ReflectReport {
            since: req.since.clone(),
            until: req.until.clone(),
            project: req.project.clone(),
            tool: req.tool.clone(),
            kinds: req.kinds.clone(),
            db_path: self.storage.db_path.display().to_string(),
            mode,
            llm_fallback_reason,
            index,
            summary,
            assessed,
            unassessed,
        })
    }

    /// Resolves the backend and checks its exact program string exists.
    fn reflect_llm_backend(&self) -> Result<LlmBackend, String> {
        let backend = self.llm().backend(None).map_err(|error| {
            format!(
                "LLM backend could not be resolved ({error}); set CORTEX_LLM to codex or gemini, or pass --no-llm"
            )
        })?;
        let program = backend.program();
        if program_on_path(&program) {
            Ok(backend)
        } else {
            Err(format!(
                "LLM backend program '{program}' was not found; install it, set CORTEX_CODEX_CMD or \
                 CORTEX_HEADLESS_GEMINI_CMD, or pass --no-llm"
            ))
        }
    }

    pub(super) async fn assess_reflect_incident(
        &self,
        incident: &ReflectIncident,
        req: &ReflectRequest,
        backend: Option<&LlmBackend>,
    ) -> ServiceResult<AssessOutcome> {
        // The target filters are exact matches on each kind's grouping key,
        // so they shrink the scan without changing the incident id.
        Ok(match incident.kind {
            ReflectKind::Skill => assess_kind!(
                self,
                incident,
                req,
                backend,
                investigate_ai_skill_incidents,
                run_one_skill_assessment,
                AiSkillInvestigateRequest {
                    skill: Some(incident.target_key.clone()),
                    plugin: incident.target_detail.clone(),
                }
            ),
            ReflectKind::Mcp => assess_kind!(
                self,
                incident,
                req,
                backend,
                investigate_ai_mcp_incidents,
                run_one_mcp_assessment,
                AiMcpInvestigateRequest {
                    mcp_server: Some(incident.target_key.clone()),
                    mcp_tool: incident.target_detail.clone(),
                }
            ),
            ReflectKind::Hook => assess_kind!(
                self,
                incident,
                req,
                backend,
                investigate_ai_hook_incidents,
                run_one_hook_assessment,
                AiHookInvestigateRequest {
                    hook_event: Some(incident.target_key.clone()),
                    hook_name: incident.target_detail.clone(),
                }
            ),
        })
    }
}

fn findings_json<T: serde::Serialize>(findings: &T) -> ServiceResult<serde_json::Value> {
    serde_json::to_value(findings)
        .map_err(|error| ServiceError::Internal(anyhow::anyhow!("serialize findings: {error}")))
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
