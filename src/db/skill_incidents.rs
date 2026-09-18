//! Skill-usage incident grouping and scoring. Groups `ai_skill_events` rows
//! into `SkillIncident`s by `(skill_name, skill_plugin, tool, project,
//! session_id, hostname, window_bucket)`, scans nearby transcript logs for
//! the five deterministic anchor signals in
//! `crate::app::skill_signal_detectors`, and scores/sorts the resulting
//! groups. Mirrors the abuse-incident grouping query (`search_ai_incidents`
//! in `src/db/queries.rs`) but keyed on skill usage instead of abuse-term
//! anchors. The investigation evidence-bundle layer that expands a
//! `SkillIncident` lives in the sibling module
//! `src/db/skill_incident_evidence.rs`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::app::skill_signal_detectors::{
    detect_ignored_instruction, detect_overlong_loop, detect_scope_or_source_confusion,
    detect_tool_failure, detect_user_correction, transcript_event_is_user,
};

use super::pool::DbPool;

// ---------------------------------------------------------------------------
// Skill incident grouping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSkillIncidentParams {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Exact incident_id match. When set, filters the full computed incident
    /// set (bounded only by `SKILL_INCIDENT_CANDIDATE_CAP`, not `limit`)
    /// before the priority-ranked truncation, so a match ranked below
    /// `limit` is still found.
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
pub struct SkillSignalCounts {
    pub user_correction_after_skill: usize,
    pub tool_failure_after_skill: usize,
    pub scope_or_source_confusion: usize,
    pub ignored_skill_or_policy_instruction: usize,
    pub overlong_loop_after_skill: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIncident {
    /// Stable synthetic ID: hash of skill name/plugin + tool/project/session/
    /// hostname + sorted anchor log ids + sorted skill event ids.
    pub incident_id: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub skill_event_count: usize,
    /// Sorted `ai_skill_events.id` values in this group.
    pub skill_event_ids: Vec<i64>,
    /// Sorted `logs.id` values backing the anchor signals in this group.
    pub anchor_log_ids: Vec<i64>,
    pub signal_counts: SkillSignalCounts,
    /// Sorted distinct signal category names present in this incident.
    pub signals_present: Vec<String>,
    pub priority_score: f64,
    /// "low" | "medium" | "high" | "critical"
    pub priority_label: String,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillIncidentResult {
    pub incidents: Vec<SkillIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

const SKILL_INCIDENT_CANDIDATE_CAP: usize = 10_000;

/// Grouping key for skill incidents: `(skill_name, skill_plugin, tool,
/// project, session_id, hostname, window_bucket)`.
/// `window_bucket = unix_secs / window_secs * window_secs` (floor to window
/// boundary), mirroring `search_ai_incidents`'s abuse-incident grouping.
pub fn search_ai_skill_incidents(
    pool: &DbPool,
    params: &AiSkillIncidentParams,
) -> Result<AiSkillIncidentResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let window_secs = i64::from(params.window_minutes.unwrap_or(10).clamp(1, 120)) * 60;

    // ── Fetch candidate skill events (bounded, same capped-window pattern as
    // search_ai_incidents' FTS candidate fetch) ─────────────────────────────
    struct SkillEventRow {
        id: i64,
        timestamp: String,
        hostname: String,
        tool: String,
        project: String,
        session_id: String,
        skill_name: String,
        skill_plugin: Option<String>,
    }

    let mut sql = String::from(
        "SELECT id, timestamp, hostname, ai_tool, ai_project, ai_session_id,
                skill_name, skill_plugin
         FROM ai_skill_events
         WHERE 1 = 1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;
    if let Some(skill) = &params.skill {
        sql.push_str(&format!(" AND skill_name = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(skill.clone()));
        idx += 1;
    }
    if let Some(plugin) = &params.plugin {
        sql.push_str(&format!(" AND skill_plugin = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(plugin.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
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
        SKILL_INCIDENT_CANDIDATE_CAP + 1
    ));

    let mut stmt = conn.prepare(&sql)?;
    let candidate_events: Vec<SkillEventRow> = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(SkillEventRow {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                hostname: row.get(2)?,
                tool: row.get(3)?,
                project: row.get(4)?,
                session_id: row.get(5)?,
                skill_name: row.get(6)?,
                skill_plugin: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let candidate_window_truncated = candidate_events.len() > SKILL_INCIDENT_CANDIDATE_CAP;
    let raw_candidate_count = candidate_events.len();

    // ── Group by (skill_name, skill_plugin, tool, project, session_id,
    // hostname, window_bucket) ───────────────────────────────────────────────
    type GroupKey = (String, Option<String>, String, String, String, String, i64);
    let mut groups: HashMap<GroupKey, Vec<&SkillEventRow>> = HashMap::new();

    for row in candidate_events.iter().take(SKILL_INCIDENT_CANDIDATE_CAP) {
        let bucket = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
            .map(|dt| (dt.timestamp() / window_secs) * window_secs)
            .unwrap_or(0);
        let key = (
            row.skill_name.clone(),
            row.skill_plugin.clone(),
            row.tool.clone(),
            row.project.clone(),
            row.session_id.clone(),
            row.hostname.clone(),
            bucket,
        );
        groups.entry(key).or_default().push(row);
    }

    // ── For each group, fetch nearby transcript logs in the session/window to
    // detect anchor signals, then score ───────────────────────────────────────
    let mut incidents: Vec<SkillIncident> = Vec::with_capacity(groups.len());
    for ((skill_name, skill_plugin, tool, project, session_id, hostname, _bucket), events) in groups
    {
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

        // Window bounds for anchor detection: from first skill event to
        // window_secs after the last one (anchors that follow the skill).
        let win_from = first_seen.clone();
        let win_to = chrono::DateTime::parse_from_rfc3339(&last_seen)
            .map(|dt| {
                (dt.with_timezone(&chrono::Utc) + chrono::Duration::seconds(window_secs))
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string()
            })
            .unwrap_or_else(|_| last_seen.clone());

        let mut anchor_stmt = conn.prepare(
            "SELECT id, message, metadata_json FROM logs
             WHERE ai_session_id = ?1 AND ai_project = ?2 AND ai_tool = ?3
               AND timestamp >= ?4 AND timestamp <= ?5
             ORDER BY timestamp ASC
             LIMIT 500",
        )?;
        let anchor_rows: Vec<(i64, String, Option<String>)> = anchor_stmt
            .query_map(
                rusqlite::params![session_id, project, tool, win_from, win_to],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get(2)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut counts = SkillSignalCounts::default();
        let mut anchor_log_ids: Vec<i64> = Vec::new();
        let tool_call_rows = anchor_rows.len();
        let mut has_correction_or_frustration = false;

        for (id, message, metadata_json) in &anchor_rows {
            let mut hit = false;
            if transcript_event_is_user(metadata_json.as_deref()) && detect_user_correction(message)
            {
                counts.user_correction_after_skill += 1;
                has_correction_or_frustration = true;
                hit = true;
            }
            if detect_tool_failure(message) {
                counts.tool_failure_after_skill += 1;
                hit = true;
            }
            if detect_scope_or_source_confusion(message) {
                counts.scope_or_source_confusion += 1;
                hit = true;
            }
            if detect_ignored_instruction(message) {
                counts.ignored_skill_or_policy_instruction += 1;
                hit = true;
            }
            if hit {
                anchor_log_ids.push(*id);
            }
        }
        if detect_overlong_loop(events.len(), tool_call_rows, has_correction_or_frustration) {
            counts.overlong_loop_after_skill += 1;
        }

        anchor_log_ids.sort_unstable();
        anchor_log_ids.dedup();

        let mut signals_present: Vec<String> = Vec::new();
        if counts.user_correction_after_skill > 0 {
            signals_present.push("user_correction_after_skill".to_string());
        }
        if counts.tool_failure_after_skill > 0 {
            signals_present.push("tool_failure_after_skill".to_string());
        }
        if counts.scope_or_source_confusion > 0 {
            signals_present.push("scope_or_source_confusion".to_string());
        }
        if counts.ignored_skill_or_policy_instruction > 0 {
            signals_present.push("ignored_skill_or_policy_instruction".to_string());
        }
        if counts.overlong_loop_after_skill > 0 {
            signals_present.push("overlong_loop_after_skill".to_string());
        }
        signals_present.sort();

        // ── Locked scoring formula ──────────────────────────────────────────
        let signal_variety = signals_present.len() as f64;
        let priority_score = events.len() as f64 * 2.0
            + counts.user_correction_after_skill as f64 * 15.0
            + counts.tool_failure_after_skill as f64 * 8.0
            + counts.scope_or_source_confusion as f64 * 12.0
            + counts.ignored_skill_or_policy_instruction as f64 * 12.0
            + counts.overlong_loop_after_skill as f64 * 10.0
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

        let mut skill_event_ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        skill_event_ids.sort_unstable();

        // ── Stable incident ID: same DefaultHasher pattern as
        // search_ai_incidents (src/db/queries.rs), extended with skill
        // name/plugin and sorted skill event ids.
        let incident_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            skill_name.hash(&mut h);
            skill_plugin.hash(&mut h);
            tool.hash(&mut h);
            project.hash(&mut h);
            session_id.hash(&mut h);
            hostname.hash(&mut h);
            for id in &anchor_log_ids {
                id.hash(&mut h);
            }
            for id in &skill_event_ids {
                id.hash(&mut h);
            }
            format!("skill-inc-{:016x}", h.finish())
        };

        incidents.push(SkillIncident {
            incident_id,
            skill_name,
            skill_plugin,
            tool,
            project,
            session_id,
            hostname,
            first_seen,
            last_seen,
            duration_secs,
            skill_event_count: events.len(),
            skill_event_ids,
            anchor_log_ids,
            signal_counts: counts,
            signals_present,
            priority_score,
            priority_label,
            window_minutes: (window_secs / 60) as u32,
        });
    }

    // ── Post-grouping filters: incident_id, signals, min_score ──────────────
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

    // Sort by priority_score descending, then last_seen descending. Uses
    // total_cmp (not partial_cmp/unwrap_or(Equal)) — a total order even if a
    // NaN score ever appears.
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
    });

    let total_incidents = incidents.len();
    let truncated = total_incidents > limit || candidate_window_truncated;
    incidents.truncate(limit);

    Ok(AiSkillIncidentResult {
        incidents,
        total_incidents,
        candidate_event_rows: raw_candidate_count.min(SKILL_INCIDENT_CANDIDATE_CAP),
        candidate_cap: SKILL_INCIDENT_CANDIDATE_CAP,
        candidate_window_truncated,
        truncated,
    })
}

#[cfg(test)]
#[path = "skill_incidents_tests.rs"]
mod tests;
