//! Investigation evidence-bundle layer for MCP incidents. Expands an
//! `McpIncident` (grouped/scored in `src/db/mcp_incidents.rs`) into a
//! bounded, truncation-flagged evidence bundle: the underlying MCP events,
//! the transcript rows that triggered anchor signals, transcript context
//! before/after, and nearby non-AI logs split into error/user-correction
//! subsets. Mirrors `investigate_ai_skill_incidents` in
//! `src/db/skill_incident_evidence.rs` but keyed on MCP tool-call usage
//! instead of skill-attribution events.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::app::mcp_signal_detectors::detect_user_correction_after_tool_call;

use super::mcp_events::AiMcpEventEntry;
use super::mcp_incidents::{AiMcpIncidentParams, McpIncident, search_ai_mcp_incidents};
use super::models::LogEntry;
use super::pool::DbPool;
use super::queries::map_row;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiMcpInvestigateParams {
    pub incident_id: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
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
pub struct McpIncidentEvidence {
    pub incident: McpIncident,
    /// The `ai_mcp_events` rows in this group, capped at 25.
    pub mcp_events: Vec<AiMcpEventEntry>,
    pub mcp_events_truncated: bool,
    /// Transcript rows that triggered an anchor signal, capped at 50.
    pub signal_anchors: Vec<LogEntry>,
    pub signal_anchors_truncated: bool,
    /// Same-session transcript entries before the first event, capped 20.
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    /// Same-session transcript entries after the last event, capped 20.
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
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
pub struct AiMcpInvestigateResult {
    pub evidence: Vec<McpIncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
}

fn map_mcp_event_row(row: &rusqlite::Row) -> rusqlite::Result<AiMcpEventEntry> {
    Ok(AiMcpEventEntry {
        id: row.get(0)?,
        call_log_id: row.get(1)?,
        result_log_id: row.get(2)?,
        ai_tool: row.get(3)?,
        ai_project: row.get(4)?,
        ai_session_id: row.get(5)?,
        hostname: row.get(6)?,
        timestamp: row.get(7)?,
        turn_id: row.get(8)?,
        call_id: row.get(9)?,
        tool_name: row.get(10)?,
        mcp_server: row.get(11)?,
        mcp_tool: row.get(12)?,
        event_kind: row.get(13)?,
        status: row.get(14)?,
        duration_ms: row.get(15)?,
        is_error: row.get::<_, Option<i64>>(16)?.map(|v| v != 0),
        arguments_json: row.get(17)?,
        output_preview: row.get(18)?,
        error_text: row.get(19)?,
    })
}

const MCP_EVENT_COLUMNS: &str = "id, call_log_id, result_log_id, ai_tool, ai_project, \
    ai_session_id, hostname, timestamp, turn_id, call_id, tool_name, mcp_server, mcp_tool, \
    event_kind, status, duration_ms, is_error, arguments_json, output_preview, error_text";

pub fn investigate_ai_mcp_incidents(
    pool: &DbPool,
    params: &AiMcpInvestigateParams,
) -> Result<AiMcpInvestigateResult> {
    const MCP_EVENTS_CAP: usize = 25;
    const SIGNAL_ANCHORS_CAP: usize = 50;
    const TRANSCRIPT_CAP: usize = 20;
    const NEARBY_CAP: usize = 50;
    const NEARBY_SUBSET_CAP: usize = 25;

    let limit = params.limit.unwrap_or(3).clamp(1, 10) as usize;
    let corr_mins = i64::from(params.correlation_window_minutes.unwrap_or(5).clamp(1, 120));

    // `incident_id` is passed straight through to `AiMcpIncidentParams`,
    // which filters the full computed incident set (bounded only by
    // `MCP_INCIDENT_CANDIDATE_CAP` events, not an incident-count cap) before
    // its own priority-ranked truncation. This guarantees an exact
    // incident_id lookup finds its target regardless of priority rank —
    // routing it through a fixed-size top-N candidate window (as a prior
    // version of this code did) could silently miss incidents ranked below
    // that window.
    let incident_result = search_ai_mcp_incidents(
        pool,
        &AiMcpIncidentParams {
            mcp_server: params.mcp_server.clone(),
            mcp_tool: params.mcp_tool.clone(),
            tool_name: params.tool_name.clone(),
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
        // ── MCP events for this group ────────────────────────────────────
        let (mcp_events, mcp_events_truncated) = if incident.mcp_event_ids.is_empty() {
            (Vec::new(), false)
        } else {
            let placeholders: Vec<String> = (1..=incident.mcp_event_ids.len())
                .map(|i| format!("?{i}"))
                .collect();
            let sql = format!(
                "SELECT {MCP_EVENT_COLUMNS} FROM ai_mcp_events WHERE id IN ({}) ORDER BY timestamp ASC",
                placeholders.join(",")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows: Vec<AiMcpEventEntry> = stmt
                .query_map(
                    rusqlite::params_from_iter(
                        incident
                            .mcp_event_ids
                            .iter()
                            .map(|id| rusqlite::types::Value::Integer(*id)),
                    ),
                    map_mcp_event_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > MCP_EVENTS_CAP;
            let mut out = rows;
            out.truncate(MCP_EVENTS_CAP);
            (out, truncated)
        };

        // ── Signal anchor log rows ──────────────────────────────────────
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

        // ── Derived subsets: user corrections, errors ────────────────────
        let mut nearby_user_corrections: Vec<LogEntry> = nearby_logs
            .iter()
            .filter(|e| detect_user_correction_after_tool_call(&e.message))
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

        evidence.push(McpIncidentEvidence {
            incident,
            mcp_events,
            mcp_events_truncated,
            signal_anchors,
            signal_anchors_truncated,
            transcript_before,
            transcript_before_truncated,
            transcript_after,
            transcript_after_truncated,
            nearby_user_corrections,
            nearby_user_corrections_truncated,
            nearby_logs,
            nearby_logs_truncated,
            nearby_errors,
            nearby_errors_truncated,
        });
    }

    Ok(AiMcpInvestigateResult {
        evidence,
        total_incidents,
        truncated,
    })
}

#[cfg(test)]
#[path = "mcp_incident_evidence_tests.rs"]
mod tests;
