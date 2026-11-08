//! Investigation evidence-bundle layer for hook incidents. Expands a
//! `HookIncident` (grouped/scored in `src/db/hook_incidents.rs`) into a
//! bounded, truncation-flagged evidence bundle: the underlying hook events,
//! the transcript rows that triggered the `user_correction_after_hook`
//! anchor, transcript context before/after, and nearby non-AI logs split
//! into tool-call/user-correction/error subsets. Mirrors
//! `src/db/skill_incident_evidence.rs` but keyed on hook usage instead of
//! skill usage.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::app::skill_signal_detectors::detect_tool_failure;

use super::hook_events::{AiHookEventEntry, map_hook_event_row};
use super::hook_incidents::{AiHookIncidentParams, HookIncident, search_ai_hook_incidents};
use super::models::LogEntry;
use super::pool::DbPool;
use super::queries::map_row;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiHookInvestigateParams {
    pub incident_id: Option<String>,
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Max incidents to investigate. Default 3, clamp 1..=10.
    pub limit: Option<u32>,
    /// Incident grouping window minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    /// Correlation window minutes around incident. Default 5, clamp 1..=120.
    pub correlation_window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIncidentEvidence {
    pub incident: HookIncident,
    /// The `ai_hook_events` rows in this group, capped at 25.
    pub hook_events: Vec<AiHookEventEntry>,
    pub hook_events_truncated: bool,
    /// Transcript rows that triggered the `user_correction_after_hook`
    /// anchor, capped at 50.
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    /// Same-session transcript entries before the first hook event, capped 20.
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    /// Same-session transcript entries after the last hook event, capped 20.
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    /// Subset of nearby_logs matching tool-failure phrases, capped 25.
    pub nearby_tool_calls: Vec<LogEntry>,
    pub nearby_tool_calls_truncated: bool,
    /// Non-AI syslog/Docker logs in the correlation window, capped 50.
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    /// Subset of nearby_logs with severity warning or above, capped 25.
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiHookInvestigateResult {
    pub evidence: Vec<HookIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
}

pub fn investigate_ai_hook_incidents(
    pool: &DbPool,
    params: &AiHookInvestigateParams,
) -> Result<AiHookInvestigateResult> {
    const HOOK_EVENTS_CAP: usize = 25;
    const SIGNAL_ANCHORS_CAP: usize = 50;
    const TRANSCRIPT_CAP: usize = 20;
    const NEARBY_CAP: usize = 50;
    const NEARBY_SUBSET_CAP: usize = 25;

    let limit = params.limit.unwrap_or(3).clamp(1, 10) as usize;
    let corr_mins = i64::from(params.correlation_window_minutes.unwrap_or(5).clamp(1, 120));

    // `incident_id` is passed straight through to `AiHookIncidentParams`,
    // which filters the full computed incident set (bounded only by
    // `HOOK_INCIDENT_CANDIDATE_CAP` events, not an incident-count cap)
    // before its own priority-ranked truncation. This guarantees an exact
    // incident_id lookup finds its target regardless of priority rank —
    // routing it through a fixed-size top-N candidate window (as a prior
    // version of this code did) could silently miss incidents ranked
    // below that window.
    let incident_result = search_ai_hook_incidents(
        pool,
        &AiHookIncidentParams {
            hook_event: params.hook_event.clone(),
            hook_name: params.hook_name.clone(),
            hook_source: params.hook_source.clone(),
            ai_tool: params.ai_tool.clone(),
            ai_project: params.ai_project.clone(),
            ai_session_id: None,
            hostname: None,
            evidence_kind: None,
            since: params.since.clone(),
            until: params.until.clone(),
            incident_id: params.incident_id.clone(),
            limit: Some(limit as u32),
            window_minutes: params.window_minutes,
            signals: Vec::new(),
            min_score: None,
        },
    )?;
    let total_incidents = incident_result.total_incidents;
    let truncated = incident_result.truncated;
    let mut incidents = incident_result.incidents;
    incidents.truncate(limit);

    let conn = pool.get()?;
    let mut evidence = Vec::with_capacity(incidents.len());

    for incident in incidents {
        // ── Hook events for this group ──────────────────────────────────
        let (hook_events, hook_events_truncated) = if incident.hook_event_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.hook_event_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            let sql = format!(
                "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                        hook_event, hook_name, hook_source, hook_command, status, exit_code,
                        duration_ms, stdout_preview, stderr_preview, persisted_output_path,
                        trusted_hash, evidence_kind, metadata_json
                 FROM ai_hook_events WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows: Vec<AiHookEventEntry> = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .hook_event_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_hook_event_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > HOOK_EVENTS_CAP;
            let mut out = rows;
            out.truncate(HOOK_EVENTS_CAP);
            (out, truncated)
        };

        // ── Signal anchor log rows (user_correction_after_hook) ─────────
        let (signal_anchors, signal_anchors_truncated) = if incident.anchor_log_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.anchor_log_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            let sql = format!(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .anchor_log_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > SIGNAL_ANCHORS_CAP;
            let mut out = rows;
            out.truncate(SIGNAL_ANCHORS_CAP);
            (out, truncated)
        };

        let signal_anchors_truncated = signal_anchors_truncated || incident.anchor_window_truncated;

        // ── Transcript before/after ──────────────────────────────────────
        let (transcript_before, transcript_before_truncated) = {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
                   AND hostname = ?5 AND timestamp < ?4
                 ORDER BY timestamp DESC
                 LIMIT 21",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![
                        &incident.session_id,
                        &incident.project,
                        &incident.tool,
                        &incident.first_seen,
                        &incident.hostname,
                    ],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > TRANSCRIPT_CAP;
            let mut out = rows;
            out.truncate(TRANSCRIPT_CAP);
            out.reverse();
            (out, truncated)
        };

        let (transcript_after, transcript_after_truncated) = {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
                   AND hostname = ?5 AND timestamp > ?4
                 ORDER BY timestamp ASC
                 LIMIT 21",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![
                        &incident.session_id,
                        &incident.project,
                        &incident.tool,
                        &incident.last_seen,
                        &incident.hostname,
                    ],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > TRANSCRIPT_CAP;
            let mut out = rows;
            out.truncate(TRANSCRIPT_CAP);
            (out, truncated)
        };

        // ── Nearby non-AI logs in the correlation window ────────────────
        let (nearby_logs, nearby_logs_truncated) = {
            let win_from = chrono::DateTime::parse_from_rfc3339(&incident.first_seen)
                .map(|dt| {
                    (dt.with_timezone(&chrono::Utc) - chrono::Duration::minutes(corr_mins))
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                })
                .unwrap_or_else(|_| incident.first_seen.clone());
            let win_to = chrono::DateTime::parse_from_rfc3339(&incident.last_seen)
                .map(|dt| {
                    (dt.with_timezone(&chrono::Utc) + chrono::Duration::minutes(corr_mins))
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                })
                .unwrap_or_else(|_| incident.last_seen.clone());

            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE timestamp >= ?1 AND timestamp <= ?2 AND hostname = ?3
                 ORDER BY timestamp ASC
                 LIMIT 51",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![win_from, win_to, &incident.hostname],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > NEARBY_CAP;
            let mut out = rows;
            out.truncate(NEARBY_CAP);
            (out, truncated)
        };

        // ── Derived subsets: tool calls (failure phrases), errors ───────
        let mut nearby_tool_calls: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| detect_tool_failure(&e.message))
            .cloned()
            .collect();
        let nearby_tool_calls_truncated = nearby_tool_calls.len() > NEARBY_SUBSET_CAP;
        nearby_tool_calls.truncate(NEARBY_SUBSET_CAP);

        let error_sevs = ["emergency", "alert", "critical", "error", "warning"];
        let mut nearby_errors: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| error_sevs.contains(&e.severity.as_str()))
            .cloned()
            .collect();
        let nearby_errors_truncated = nearby_errors.len() > NEARBY_SUBSET_CAP;
        nearby_errors.truncate(NEARBY_SUBSET_CAP);

        // Signal anchors already restricted to user_correction_after_hook
        // rows in `search_ai_hook_incidents`; nothing else to derive here
        // beyond the two subsets above. (No separate `nearby_user_corrections`
        // subset like the skill-incident evidence bundle has — the anchor
        // rows already *are* the correction evidence for hooks.)
        evidence.push(HookIncidentEvidence {
            incident,
            hook_events,
            hook_events_truncated,
            signal_anchors,
            signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated,
            transcript_after,
            transcript_after_truncated,
            nearby_tool_calls,
            nearby_tool_calls_truncated,
            nearby_logs,
            nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated,
        });
    }

    Ok(AiHookInvestigateResult {
        evidence,
        total_incidents,
        truncated,
    })
}

#[cfg(test)]
#[path = "hook_incident_evidence_tests.rs"]
mod tests;
