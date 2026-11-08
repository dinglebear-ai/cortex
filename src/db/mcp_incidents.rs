//! MCP-incident grouping and scoring. Groups `ai_mcp_events` rows into
//! `McpIncident`s by `(mcp_server, mcp_tool, ai_tool, ai_project,
//! ai_session_id, hostname, window_bucket)` per GH #94's "MCP grouping key"
//! section, scans nearby transcript logs for the six deterministic anchor
//! signals in `crate::app::mcp_signal_detectors`, and scores/sorts the
//! resulting groups. Mirrors `search_ai_skill_incidents` in
//! `src/db/skill_incidents.rs` but keyed on MCP tool-call usage instead of
//! skill-attribution events. Only MCP-classified rows (`mcp_server IS NOT
//! NULL`) participate in grouping — general/builtin tool calls are excluded
//! per the schema note in GH #94 ("`cortex assess mcp` filters to
//! MCP-classified rows").

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::app::mcp_signal_detectors::{
    detect_auth_or_permission_failure, detect_repeated_call_failure,
    detect_schema_or_validation_error, detect_timeout_or_rate_limit, detect_unknown_tool_or_server,
    detect_user_correction_after_tool_call,
};

use super::incident_anchors::{AnchorWindow, load_anchors};
use super::pool::DbPool;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiMcpIncidentParams {
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub tool_name: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Exact incident_id match. When set, filters the full computed incident
    /// set (bounded only by `MCP_INCIDENT_CANDIDATE_CAP`, not `limit`) before
    /// the priority-ranked truncation, so a match ranked below `limit` is
    /// still found.
    pub incident_id: Option<String>,
    /// Max incidents to return. Default 20, clamp 1..=100.
    pub limit: Option<u32>,
    /// Grouping window in minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    /// Restrict to incidents containing at least one of these signal
    /// categories. Empty = no filter (all incidents).
    pub signals: Vec<String>,
    /// Minimum `priority_score` (inclusive). `None` = no filter.
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpSignalCounts {
    pub repeated_call_failure: usize,
    pub timeout_or_rate_limit: usize,
    pub auth_or_permission_failure: usize,
    pub schema_or_validation_error: usize,
    pub unknown_tool_or_server: usize,
    pub user_correction_after_tool_call: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIncident {
    /// Stable synthetic ID: hash of server/tool + tool/project/session/
    /// hostname + sorted mcp event ids.
    pub incident_id: String,
    pub mcp_server: String,
    pub mcp_tool: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub event_count: usize,
    pub error_count: usize,
    /// Sorted `ai_mcp_events.id` values in this group.
    pub mcp_event_ids: Vec<i64>,
    /// Sorted `logs.id` values backing the anchor signals in this group.
    pub anchor_log_ids: Vec<i64>,
    /// Anchor sampling hit the per-window cap or shared request budget.
    #[serde(default)]
    pub anchor_window_truncated: bool,
    pub signal_counts: McpSignalCounts,
    /// Sorted distinct signal category names present in this incident.
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    /// "low" | "medium" | "high" | "critical"
    pub priority_label: String,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMcpIncidentResult {
    pub incidents: Vec<McpIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

const MCP_INCIDENT_CANDIDATE_CAP: usize = 10_000;

/// Grouping key for MCP incidents: `(mcp_server, mcp_tool, ai_tool,
/// ai_project, ai_session_id, hostname, window_bucket)`.
/// `window_bucket = unix_secs / window_secs * window_secs` (floor to window
/// boundary), mirroring `search_ai_skill_incidents`'s grouping.
pub fn search_ai_mcp_incidents(
    pool: &DbPool,
    params: &AiMcpIncidentParams,
) -> Result<AiMcpIncidentResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let window_secs = i64::from(params.window_minutes.unwrap_or(10).clamp(1, 120)) * 60;

    struct McpEventRow {
        id: i64,
        timestamp: String,
        hostname: String,
        tool: String,
        project: String,
        session_id: String,
        mcp_server: String,
        mcp_tool: Option<String>,
        is_error: Option<bool>,
    }

    // Only MCP-classified rows participate in incident grouping — general
    // tool calls (mcp_server IS NULL) are excluded here (GH #94: "cortex
    // assess mcp filters to MCP-classified rows").
    let mut sql = String::from(
        "SELECT id, timestamp, hostname, ai_tool, ai_project, ai_session_id,
                mcp_server, mcp_tool, is_error
         FROM ai_mcp_events
         WHERE mcp_server IS NOT NULL",
    );
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;
    if let Some(server) = &params.mcp_server {
        sql.push_str(&format!(" AND mcp_server = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(server.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.mcp_tool {
        sql.push_str(&format!(" AND mcp_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
        idx += 1;
    }
    if let Some(tool_name) = &params.tool_name {
        sql.push_str(&format!(" AND tool_name = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool_name.clone()));
        idx += 1;
    }
    if let Some(ai_tool) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(ai_tool.clone()));
        idx += 1;
    }
    if let Some(project) = &params.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(project.clone()));
        idx += 1;
    }
    if let Some(session_id) = &params.ai_session_id {
        sql.push_str(&format!(" AND ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(session_id.clone()));
        idx += 1;
    }
    if let Some(hostname) = &params.hostname {
        sql.push_str(&format!(" AND hostname = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(hostname.clone()));
        idx += 1;
    }
    if let Some(from) = &params.since {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(from.clone()));
        idx += 1;
    }
    if let Some(to) = &params.until {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(to.clone()));
    }
    let _ = idx;
    sql.push_str(&format!(
        " ORDER BY timestamp ASC LIMIT {}",
        MCP_INCIDENT_CANDIDATE_CAP + 1
    ));

    let mut stmt = conn.prepare(&sql)?;
    let candidate_events: Vec<McpEventRow> = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(McpEventRow {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                hostname: row.get(2)?,
                tool: row.get(3)?,
                project: row.get(4)?,
                session_id: row.get(5)?,
                mcp_server: row.get(6)?,
                mcp_tool: row.get(7)?,
                is_error: row.get::<_, Option<i64>>(8)?.map(|v| v != 0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let candidate_window_truncated = candidate_events.len() > MCP_INCIDENT_CANDIDATE_CAP;
    let raw_candidate_count = candidate_events.len();

    // ── Group by (mcp_server, mcp_tool, ai_tool, ai_project, ai_session_id,
    // hostname, window_bucket) ───────────────────────────────────────────────
    type GroupKey = (String, Option<String>, String, String, String, String, i64);
    let mut groups: HashMap<GroupKey, Vec<&McpEventRow>> = HashMap::new();

    for row in candidate_events.iter().take(MCP_INCIDENT_CANDIDATE_CAP) {
        let bucket = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
            .map(|dt| (dt.timestamp() / window_secs) * window_secs)
            .unwrap_or(0);
        let key = (
            row.mcp_server.clone(),
            row.mcp_tool.clone(),
            row.tool.clone(),
            row.project.clone(),
            row.session_id.clone(),
            row.hostname.clone(),
            bucket,
        );
        groups.entry(key).or_default().push(row);
    }

    let anchors = load_anchors(
        &conn,
        groups.iter().map(|(key, events)| {
            AnchorWindow::new(
                &key.4,
                &key.3,
                &key.2,
                &key.5,
                &events.first().expect("nonempty group").timestamp,
                &events.last().expect("nonempty group").timestamp,
                window_secs,
            )
        }),
    )?;
    let anchor_windows_truncated = anchors.truncated();
    let mut incidents: Vec<McpIncident> = Vec::with_capacity(groups.len());
    for ((mcp_server, mcp_tool, tool, project, session_id, hostname, _bucket), events) in groups {
        let first_seen = events
            .first()
            .map(|e| e.timestamp.clone())
            .unwrap_or_default();
        let last_seen = events
            .last()
            .map(|e| e.timestamp.clone())
            .unwrap_or_default();
        let duration_secs = {
            let t0 = chrono::DateTime::parse_from_rfc3339(&first_seen)
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            let t1 = chrono::DateTime::parse_from_rfc3339(&last_seen)
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            (t1 - t0).max(0)
        };
        let error_count = events.iter().filter(|e| e.is_error == Some(true)).count();

        let window = AnchorWindow::new(
            &session_id,
            &project,
            &tool,
            &hostname,
            &first_seen,
            &last_seen,
            window_secs,
        );
        let window_anchors = anchors.get(&window);
        let anchor_rows = &window_anchors.rows;
        let anchor_window_truncated = window_anchors.truncated;

        let mut counts = McpSignalCounts::default();
        let mut anchor_log_ids: Vec<i64> = Vec::new();

        for row in anchor_rows {
            let (id, message) = (&row.id, &row.message);
            let mut hit = false;
            if detect_timeout_or_rate_limit(message) {
                counts.timeout_or_rate_limit += 1;
                hit = true;
            }
            if detect_auth_or_permission_failure(message) {
                counts.auth_or_permission_failure += 1;
                hit = true;
            }
            if detect_schema_or_validation_error(message) {
                counts.schema_or_validation_error += 1;
                hit = true;
            }
            if detect_unknown_tool_or_server(message) {
                counts.unknown_tool_or_server += 1;
                hit = true;
            }
            if detect_user_correction_after_tool_call(message) {
                counts.user_correction_after_tool_call += 1;
                hit = true;
            }
            if hit {
                anchor_log_ids.push(*id);
            }
        }
        if detect_repeated_call_failure(error_count) {
            counts.repeated_call_failure = error_count;
        }

        anchor_log_ids.sort_unstable();
        anchor_log_ids.dedup();

        let mut signals_present: Vec<String> = Vec::new();
        if counts.repeated_call_failure > 0 {
            signals_present.push("repeated_call_failure".to_string());
        }
        if counts.timeout_or_rate_limit > 0 {
            signals_present.push("timeout_or_rate_limit".to_string());
        }
        if counts.auth_or_permission_failure > 0 {
            signals_present.push("auth_or_permission_failure".to_string());
        }
        if counts.schema_or_validation_error > 0 {
            signals_present.push("schema_or_validation_error".to_string());
        }
        if counts.unknown_tool_or_server > 0 {
            signals_present.push("unknown_tool_or_server".to_string());
        }
        if counts.user_correction_after_tool_call > 0 {
            signals_present.push("user_correction_after_tool_call".to_string());
        }
        signals_present.sort();

        // ── Locked scoring formula (mirrors search_ai_skill_incidents'
        // weighting shape; weights chosen so a single repeated-failure or
        // user-correction signal already crosses into "medium") ───────────
        let signal_variety = signals_present.len() as f64;
        let priority_score = events.len() as f64 * 2.0
            + counts.repeated_call_failure as f64 * 10.0
            + counts.timeout_or_rate_limit as f64 * 8.0
            + counts.auth_or_permission_failure as f64 * 12.0
            + counts.schema_or_validation_error as f64 * 10.0
            + counts.unknown_tool_or_server as f64 * 12.0
            + counts.user_correction_after_tool_call as f64 * 15.0
            + signal_variety * 5.0;

        let priority_label = if priority_score < 15.0 {
            "low"
        } else if priority_score < 35.0 {
            "medium"
        } else if priority_score < 60.0 {
            "high"
        } else {
            "critical"
        }
        .to_string();

        let mut mcp_event_ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        mcp_event_ids.sort_unstable();

        // Identity follows the event group, not its sampled evidence. New
        // anchors or request-wide sampling cutoffs must not invalidate an ID.
        let incident_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            mcp_server.hash(&mut h);
            mcp_tool.hash(&mut h);
            tool.hash(&mut h);
            project.hash(&mut h);
            session_id.hash(&mut h);
            hostname.hash(&mut h);
            for id in &mcp_event_ids {
                id.hash(&mut h);
            }
            format!("mcp-inc-{:016x}", h.finish())
        };

        incidents.push(McpIncident {
            incident_id,
            mcp_server,
            mcp_tool,
            tool,
            project,
            session_id,
            hostname,
            first_seen,
            last_seen,
            duration_secs,
            event_count: mcp_event_ids.len(),
            error_count,
            mcp_event_ids,
            anchor_log_ids,
            anchor_window_truncated,
            signal_counts: counts,
            signals_present,
            priority_score,
            priority_label,
            window_minutes: (window_secs / 60) as u32,
        });
    }

    if let Some(incident_id) = &params.incident_id {
        incidents.retain(|inc| &inc.incident_id == incident_id);
    }
    if !params.signals.is_empty() {
        incidents.retain(|inc| {
            inc.signals_present
                .iter()
                .any(|s| params.signals.contains(s))
        });
    }
    if let Some(min_score) = params.min_score {
        incidents.retain(|inc| inc.priority_score >= min_score);
    }

    // total_cmp (not partial_cmp/unwrap_or(Equal)) — a total order even if a
    // NaN score ever appears.
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
    });

    let total_incidents = incidents.len();
    let truncated =
        total_incidents > limit || candidate_window_truncated || anchor_windows_truncated;
    incidents.truncate(limit);

    Ok(AiMcpIncidentResult {
        incidents,
        total_incidents,
        candidate_event_rows: raw_candidate_count.min(MCP_INCIDENT_CANDIDATE_CAP),
        candidate_cap: MCP_INCIDENT_CANDIDATE_CAP,
        candidate_window_truncated,
        truncated,
    })
}

#[cfg(test)]
#[path = "mcp_incidents_tests.rs"]
mod tests;
