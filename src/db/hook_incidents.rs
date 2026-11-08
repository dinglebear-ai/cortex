//! Hook-usage incident grouping and scoring. Groups `ai_hook_events` rows
//! into `HookIncident`s by `(hook_event, hook_name, hook_source, ai_tool,
//! ai_project, ai_session_id, hostname, window_bucket)`, scans nearby
//! transcript logs for the `user_correction_after_hook` anchor and derives
//! the remaining anchors directly from the hook event rows themselves
//! (failure status, high duration, output-parse-error phrases, invocation
//! frequency). Mirrors `src/db/skill_incidents.rs`'s grouping query but keyed
//! on hook usage instead of skill usage.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::app::hook_signal_detectors::{
    detect_hook_invoked_too_often, detect_hook_output_parse_error, detect_user_correction,
    is_hook_failure_status, is_hook_timeout,
};

use super::incident_anchors::{AnchorWindow, load_anchors};
use super::pool::DbPool;

// ---------------------------------------------------------------------------
// Hook incident grouping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiHookIncidentParams {
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    /// Restrict candidate hook events to a single `evidence_kind` (e.g.
    /// `"runtime_transcript"` to only consider proven-executed hooks).
    /// `None` = no filter (all evidence kinds considered).
    pub evidence_kind: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Exact incident_id match. When set, filters the full computed incident
    /// set (bounded only by `HOOK_INCIDENT_CANDIDATE_CAP`, not `limit`)
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
pub struct HookSignalCounts {
    pub hook_failed: usize,
    pub hook_timed_out: usize,
    pub hook_output_parse_error: usize,
    pub hook_invoked_too_often: usize,
    pub user_correction_after_hook: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIncident {
    /// Stable synthetic ID: hash of hook event/name/source + tool/project/
    /// session/hostname + sorted hook event ids.
    pub incident_id: String,
    pub hook_event: String,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub hook_event_count: usize,
    /// Sorted `ai_hook_events.id` values in this group.
    pub hook_event_ids: Vec<i64>,
    /// Sorted `logs.id` values backing the `user_correction_after_hook`
    /// anchor in this group.
    pub anchor_log_ids: Vec<i64>,
    /// Anchor sampling hit the per-window cap or shared request budget.
    #[serde(default)]
    pub anchor_window_truncated: bool,
    pub signal_counts: HookSignalCounts,
    /// Sorted distinct signal category names present in this incident.
    pub signals_present: Vec<String>,
    /// True when every hook event backing this incident has
    /// `evidence_kind = 'runtime_transcript'` — i.e. this incident is
    /// backed by proven execution evidence, not just configuration
    /// inventory. Callers (deterministic findings, `cortex assess hooks`)
    /// must use this to avoid claiming a hook executed when only
    /// config/trust-state evidence exists.
    pub has_runtime_evidence: bool,
    pub priority_score: f64,
    /// "low" | "medium" | "high" | "critical"
    pub priority_label: String,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiHookIncidentResult {
    pub incidents: Vec<HookIncident>,
    pub total_incidents: usize,
    pub candidate_event_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

const HOOK_INCIDENT_CANDIDATE_CAP: usize = 10_000;

/// Grouping key for hook incidents: `(hook_event, hook_name, hook_source,
/// tool, project, session_id, hostname, window_bucket)`.
/// `window_bucket = unix_secs / window_secs * window_secs` (floor to window
/// boundary), mirroring `search_ai_skill_incidents`'s grouping.
pub fn search_ai_hook_incidents(
    pool: &DbPool,
    params: &AiHookIncidentParams,
) -> Result<AiHookIncidentResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let window_secs = i64::from(params.window_minutes.unwrap_or(10).clamp(1, 120)) * 60;

    struct HookEventRow {
        id: i64,
        timestamp: String,
        hostname: String,
        tool: String,
        project: String,
        session_id: String,
        hook_event: String,
        hook_name: Option<String>,
        hook_source: Option<String>,
        status: String,
        duration_ms: Option<i64>,
        stdout_preview: Option<String>,
        stderr_preview: Option<String>,
        evidence_kind: String,
    }

    let mut sql = String::from(
        "SELECT id, timestamp, hostname, ai_tool, ai_project, ai_session_id,
                hook_event, hook_name, hook_source, status, duration_ms,
                stdout_preview, stderr_preview, evidence_kind
         FROM ai_hook_events
         WHERE 1 = 1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;
    if let Some(v) = &params.hook_event {
        sql.push_str(&format!(" AND hook_event = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.hook_name {
        sql.push_str(&format!(" AND hook_name = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.hook_source {
        sql.push_str(&format!(" AND hook_source = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.ai_session_id {
        sql.push_str(&format!(" AND ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.hostname {
        sql.push_str(&format!(" AND hostname = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
        idx += 1;
    }
    if let Some(v) = &params.evidence_kind {
        sql.push_str(&format!(" AND evidence_kind = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(v.clone()));
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
        HOOK_INCIDENT_CANDIDATE_CAP + 1
    ));

    let mut stmt = conn.prepare(&sql)?;
    let candidate_events: Vec<HookEventRow> = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(HookEventRow {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                hostname: row.get(2)?,
                tool: row.get(3)?,
                project: row.get(4)?,
                session_id: row.get(5)?,
                hook_event: row.get(6)?,
                hook_name: row.get(7)?,
                hook_source: row.get(8)?,
                status: row.get(9)?,
                duration_ms: row.get(10)?,
                stdout_preview: row.get(11)?,
                stderr_preview: row.get(12)?,
                evidence_kind: row.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let candidate_window_truncated = candidate_events.len() > HOOK_INCIDENT_CANDIDATE_CAP;
    let raw_candidate_count = candidate_events.len();

    // ── Group by (hook_event, hook_name, hook_source, tool, project,
    // session_id, hostname, window_bucket) ─────────────────────────────────
    type GroupKey = (
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        String,
        i64,
    );
    let mut groups: HashMap<GroupKey, Vec<&HookEventRow>> = HashMap::new();

    for row in candidate_events.iter().take(HOOK_INCIDENT_CANDIDATE_CAP) {
        let bucket = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
            .map(|dt| (dt.timestamp() / window_secs) * window_secs)
            .unwrap_or(0);
        let key = (
            row.hook_event.clone(),
            row.hook_name.clone(),
            row.hook_source.clone(),
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
                &key.5,
                &key.4,
                &key.3,
                &key.6,
                &events.first().expect("nonempty group").timestamp,
                &events.last().expect("nonempty group").timestamp,
                window_secs,
            )
        }),
    )?;
    let anchor_windows_truncated = anchors.truncated();
    let mut incidents: Vec<HookIncident> = Vec::with_capacity(groups.len());
    for (
        (hook_event, hook_name, hook_source, tool, project, session_id, hostname, _bucket),
        events,
    ) in groups
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

        let mut counts = HookSignalCounts::default();
        let has_runtime_evidence = events
            .iter()
            .all(|e| e.evidence_kind == "runtime_transcript");

        for event in &events {
            if is_hook_failure_status(&event.status) {
                counts.hook_failed += 1;
            }
            if is_hook_timeout(&event.status, event.duration_ms) {
                counts.hook_timed_out += 1;
            }
            let preview_hit = event
                .stdout_preview
                .as_deref()
                .is_some_and(detect_hook_output_parse_error)
                || event
                    .stderr_preview
                    .as_deref()
                    .is_some_and(detect_hook_output_parse_error);
            if preview_hit {
                counts.hook_output_parse_error += 1;
            }
        }
        if detect_hook_invoked_too_often(events.len()) {
            counts.hook_invoked_too_often = events.len();
        }

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

        let mut anchor_log_ids: Vec<i64> = Vec::new();
        for row in anchor_rows {
            let (id, message) = (&row.id, &row.message);
            if detect_user_correction(message) {
                counts.user_correction_after_hook += 1;
                anchor_log_ids.push(*id);
            }
        }
        anchor_log_ids.sort_unstable();
        anchor_log_ids.dedup();

        let mut signals_present: Vec<String> = Vec::new();
        if counts.hook_failed > 0 {
            signals_present.push("hook_failed".to_string());
        }
        if counts.hook_timed_out > 0 {
            signals_present.push("hook_timed_out".to_string());
        }
        if counts.hook_output_parse_error > 0 {
            signals_present.push("hook_output_parse_error".to_string());
        }
        if counts.hook_invoked_too_often > 0 {
            signals_present.push("hook_invoked_too_often".to_string());
        }
        if counts.user_correction_after_hook > 0 {
            signals_present.push("user_correction_after_hook".to_string());
        }
        signals_present.sort();

        // ── Locked scoring formula (mirrors skill-incident scoring shape,
        // weighted for hook-specific signal severity) ──────────────────────
        let signal_variety = signals_present.len() as f64;
        let priority_score = events.len() as f64 * 2.0
            + counts.hook_failed as f64 * 15.0
            + counts.hook_timed_out as f64 * 10.0
            + counts.hook_output_parse_error as f64 * 10.0
            + counts.hook_invoked_too_often as f64 * 8.0
            + counts.user_correction_after_hook as f64 * 15.0
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

        let mut hook_event_ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        hook_event_ids.sort_unstable();

        // Identity follows the event group, not its sampled evidence. New
        // anchors or request-wide sampling cutoffs must not invalidate an ID.
        let incident_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            hook_event.hash(&mut h);
            hook_name.hash(&mut h);
            hook_source.hash(&mut h);
            tool.hash(&mut h);
            project.hash(&mut h);
            session_id.hash(&mut h);
            hostname.hash(&mut h);
            for id in &hook_event_ids {
                id.hash(&mut h);
            }
            format!("hook-inc-{:016x}", h.finish())
        };

        incidents.push(HookIncident {
            incident_id,
            hook_event,
            hook_name,
            hook_source,
            tool,
            project,
            session_id,
            hostname,
            first_seen,
            last_seen,
            duration_secs,
            hook_event_count: events.len(),
            hook_event_ids,
            anchor_log_ids,
            anchor_window_truncated,
            signal_counts: counts,
            signals_present,
            has_runtime_evidence,
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

    // Sort by priority_score descending, then last_seen descending. Uses
    // total_cmp (never partial_cmp/unwrap_or(Equal)) for a total order even
    // if a NaN score ever appears.
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
    });

    let total_incidents = incidents.len();
    let truncated =
        total_incidents > limit || candidate_window_truncated || anchor_windows_truncated;
    incidents.truncate(limit);

    Ok(AiHookIncidentResult {
        incidents,
        total_incidents,
        candidate_event_rows: raw_candidate_count.min(HOOK_INCIDENT_CANDIDATE_CAP),
        candidate_cap: HOOK_INCIDENT_CANDIDATE_CAP,
        candidate_window_truncated,
        truncated,
    })
}

#[cfg(test)]
#[path = "hook_incidents_tests.rs"]
mod tests;
