//! Investigation evidence-bundle layer for skill incidents. Expands a
//! `SkillIncident` (grouped/scored in `src/db/skill_incidents.rs`) into a
//! bounded, truncation-flagged evidence bundle: the underlying skill events,
//! the transcript rows that triggered anchor signals, transcript context
//! before/after, and nearby non-AI logs split into tool-failure/
//! user-correction/error subsets. Mirrors `investigate_ai_incidents` in
//! `src/db/queries.rs` but keyed on skill usage instead of abuse-term
//! anchors.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::app::skill_signal_detectors::{
    detect_tool_failure, detect_user_correction, transcript_event_is_user,
};

use super::models::LogEntry;
use super::pool::DbPool;
use super::queries::map_row;
use super::skill_events::AiSkillEventEntry;
use super::skill_incidents::{AiSkillIncidentParams, SkillIncident, search_ai_skill_incidents};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillInvestigateParams {
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
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
pub struct SkillIncidentEvidence {
    pub incident: SkillIncident,
    /// The `ai_skill_events` rows in this group, capped at 25.
    pub skill_events: Vec<AiSkillEventEntry>,
    pub skill_events_truncated: bool,
    /// Transcript rows that triggered an anchor signal, capped at 50.
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    /// Same-session transcript entries before the first skill event, capped 20.
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    /// Same-session transcript entries after the last skill event, capped 20.
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    /// Subset of nearby_logs matching tool-failure phrases, capped 25.
    pub nearby_tool_failures: Vec<LogEntry>,
    pub nearby_tool_failures_truncated: bool,
    /// Subset of nearby_logs matching user-correction phrases, capped 25.
    pub nearby_user_corrections: Vec<LogEntry>,
    pub nearby_user_corrections_truncated: bool,
    /// Non-AI syslog/Docker logs in the correlation window, capped 50.
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    /// Subset of nearby_logs with severity warning or above, capped 25.
    pub nearby_errors: Vec<LogEntry>,
    pub nearby_errors_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillInvestigateResult {
    pub evidence: Vec<SkillIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
}

/// Row mapper for `ai_skill_events` rows, matching the 11-column SELECT used
/// throughout this module. Kept as a standalone fn (rather than duplicating
/// the inline closure in `skill_events::list_skill_events`) so the ordering
/// only needs to be maintained in one place.
fn map_skill_event_row(row: &rusqlite::Row) -> rusqlite::Result<AiSkillEventEntry> {
    Ok(AiSkillEventEntry {
        id: row.get(0)?,
        log_id: row.get(1)?,
        ai_tool: row.get(2)?,
        ai_project: row.get(3)?,
        ai_session_id: row.get(4)?,
        hostname: row.get(5)?,
        timestamp: row.get(6)?,
        skill_name: row.get(7)?,
        skill_plugin: row.get(8)?,
        event_kind: row.get(9)?,
        evidence_kind: row.get(10)?,
    })
}

pub fn investigate_ai_skill_incidents(
    pool: &DbPool,
    params: &AiSkillInvestigateParams,
) -> Result<AiSkillInvestigateResult> {
    const SKILL_EVENTS_CAP: usize = 25;
    const SIGNAL_ANCHORS_CAP: usize = 50;
    const TRANSCRIPT_CAP: usize = 20;
    const NEARBY_CAP: usize = 50;
    const NEARBY_SUBSET_CAP: usize = 25;

    let limit = params.limit.unwrap_or(3).clamp(1, 10) as usize;
    let corr_mins = i64::from(params.correlation_window_minutes.unwrap_or(5).clamp(1, 120));

    // `incident_id` is passed straight through to `AiSkillIncidentParams`,
    // which filters the full computed incident set (bounded only by
    // `SKILL_INCIDENT_CANDIDATE_CAP` events, not an incident-count cap)
    // before its own priority-ranked truncation. This guarantees an exact
    // incident_id lookup finds its target regardless of priority rank —
    // routing it through a fixed-size top-N candidate window (as a prior
    // version of this code did) could silently miss incidents ranked
    // below that window.
    let incident_result = search_ai_skill_incidents(
        pool,
        &AiSkillIncidentParams {
            skill: params.skill.clone(),
            plugin: params.plugin.clone(),
            ai_tool: params.ai_tool.clone(),
            ai_project: params.ai_project.clone(),
            ai_session_id: None,
            hostname: None,
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
        // ── Skill events for this group ─────────────────────────────────────
        let (skill_events, skill_events_truncated) = if incident.skill_event_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.skill_event_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            let sql = format!(
                "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                        skill_name, skill_plugin, event_kind, evidence_kind
                 FROM ai_skill_events WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows: Vec<AiSkillEventEntry> = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .skill_event_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_skill_event_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > SKILL_EVENTS_CAP;
            let mut out = rows;
            out.truncate(SKILL_EVENTS_CAP);
            (out, truncated)
        };

        // ── Signal anchor log rows ──────────────────────────────────────────
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

        // ── Transcript before/after (same pattern as investigate_ai_incidents) ──
        let (transcript_before, transcript_before_truncated) = {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, hostname, facility, severity, app_name,
                        process_id, message, received_at, source_ip,
                        ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
                 FROM logs
                 WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
                   AND timestamp < ?4
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
                   AND timestamp > ?4
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
                    ],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > TRANSCRIPT_CAP;
            let mut out = rows;
            out.truncate(TRANSCRIPT_CAP);
            (out, truncated)
        };

        // ── Nearby non-AI logs in the correlation window ────────────────────
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

        // ── Derived subsets: tool failures, user corrections, errors ────────
        let mut nearby_tool_failures: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| detect_tool_failure(&e.message))
            .cloned()
            .collect();
        let nearby_tool_failures_truncated = nearby_tool_failures.len() > NEARBY_SUBSET_CAP;
        nearby_tool_failures.truncate(NEARBY_SUBSET_CAP);

        let mut nearby_user_corrections: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| {
                transcript_event_is_user(e.metadata_json.as_deref())
                    && detect_user_correction(&e.message)
            })
            .cloned()
            .collect();
        let nearby_user_corrections_truncated = nearby_user_corrections.len() > NEARBY_SUBSET_CAP;
        nearby_user_corrections.truncate(NEARBY_SUBSET_CAP);

        let error_sevs = ["emergency", "alert", "critical", "error", "warning"];
        let mut nearby_errors: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| error_sevs.contains(&e.severity.as_str()))
            .cloned()
            .collect();
        let nearby_errors_truncated = nearby_errors.len() > NEARBY_SUBSET_CAP;
        nearby_errors.truncate(NEARBY_SUBSET_CAP);

        evidence.push(SkillIncidentEvidence {
            incident,
            skill_events,
            skill_events_truncated,
            signal_anchors,
            signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated,
            transcript_after,
            transcript_after_truncated,
            nearby_tool_failures,
            nearby_tool_failures_truncated,
            nearby_user_corrections,
            nearby_user_corrections_truncated,
            nearby_logs,
            nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated,
        });
    }

    Ok(AiSkillInvestigateResult {
        evidence,
        total_incidents,
        truncated,
    })
}

#[cfg(test)]
#[path = "skill_incident_evidence_tests.rs"]
mod tests;
