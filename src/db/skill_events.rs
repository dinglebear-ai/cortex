//! `ai_skill_events` insert + list query layer. Table/columns are defined in
//! migration 38 (`src/db/pool.rs`). Extraction happens in
//! `crate::scanner::skill_events`; this module only persists and reads back
//! already-extracted events.

use anyhow::Result;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use crate::scanner::skill_events::ExtractedSkillEvent;

use super::pool::DbPool;

#[derive(Debug, Clone)]
pub struct SkillEventInsert {
    pub log_id: i64,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub event: ExtractedSkillEvent,
}

/// Insert `events` inside an existing transaction with `INSERT OR IGNORE`
/// (idempotent on the `UNIQUE(log_id, skill_name, event_kind, evidence_kind)`
/// constraint). Returns the number of rows actually inserted (excludes
/// ignored duplicates) via SQLite `changes()` summed per statement.
pub(crate) fn insert_skill_events_in_tx(
    tx: &Transaction<'_>,
    events: &[SkillEventInsert],
) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO ai_skill_events (
            log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
            skill_name, skill_plugin, event_kind, evidence_kind
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    let mut inserted = 0usize;
    for item in events {
        let changed = stmt.execute(params![
            item.log_id,
            item.ai_tool,
            item.ai_project,
            item.ai_session_id,
            item.hostname,
            item.timestamp,
            item.event.skill_name,
            item.event.skill_plugin,
            item.event.event_kind.as_str(),
            item.event.evidence_kind.as_str(),
        ])?;
        inserted += changed;
    }
    Ok(inserted)
}

/// Pool-acquiring wrapper for callers outside an existing transaction (e.g.
/// the backfill service, which owns its own chunked transaction boundary).
pub fn insert_skill_events(pool: &DbPool, events: &[SkillEventInsert]) -> Result<usize> {
    let mut conn = crate::db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let inserted = insert_skill_events_in_tx(&tx, events)?;
    tx.commit()?;
    if inserted > 0 {
        super::agent_observatory::notify_projection_work();
    }
    Ok(inserted)
}

#[derive(Debug, Clone, Default)]
pub struct AiSkillEventParams {
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSkillEventEntry {
    pub id: i64,
    pub log_id: i64,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub event_kind: String,
    pub evidence_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSkillEventsResult {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<AiSkillEventEntry>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

/// List `ai_skill_events` rows newest-first, applying every non-`None`
/// filter in `params` as an `AND`-ed equality/range clause. `limit` is
/// clamped to `[1, 500]`; `truncated` is `true` when more rows matched than
/// were returned (probed via `LIMIT + 1`, mirroring `list_ai_tools`'s
/// truncation-detection pattern in `src/db/queries.rs`).
pub fn list_skill_events(
    pool: &DbPool,
    params: &AiSkillEventParams,
) -> Result<ListSkillEventsResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;

    let mut sql = String::from(
        "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                skill_name, skill_plugin, event_kind, evidence_kind
         FROM ai_skill_events WHERE 1 = 1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;

    macro_rules! bind_eq {
        ($column:literal, $value:expr) => {
            if let Some(value) = $value {
                sql.push_str(&format!(" AND {} = ?{idx}", $column));
                bindings.push(rusqlite::types::Value::Text(value.clone()));
                idx += 1;
            }
        };
    }
    bind_eq!("skill_name", &params.skill);
    bind_eq!("skill_plugin", &params.plugin);
    bind_eq!("ai_tool", &params.tool);
    bind_eq!("ai_project", &params.project);
    bind_eq!("ai_session_id", &params.session_id);
    bind_eq!("hostname", &params.hostname);
    if let Some(from) = &params.from {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(from.clone()));
        idx += 1;
    }
    if let Some(to) = &params.to {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(to.clone()));
        idx += 1;
    }
    let _ = idx;
    sql.push_str(&format!(
        " ORDER BY timestamp DESC, id DESC LIMIT {}",
        limit + 1
    ));

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
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
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let truncated = rows.len() > limit;
    rows.truncate(limit);
    Ok(ListSkillEventsResult {
        total: rows.len(),
        truncated,
        events: rows,
    })
}

#[cfg(test)]
#[path = "skill_events_tests.rs"]
mod tests;
