//! `ai_hook_events` insert + list query layer. Table/columns are defined in
//! migration 40 (`src/db/pool.rs`). Extraction happens in
//! `crate::scanner::hook_events` (Claude runtime attachments) and
//! `crate::hook_config` (Claude/Codex config-inventory collectors); this
//! module only persists and reads back already-extracted events. Mirrors
//! `src/db/skill_events.rs` one-for-one.

use anyhow::Result;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use crate::scanner::hook_events::ExtractedHookEvent;

use super::pool::DbPool;

#[derive(Debug, Clone)]
pub struct HookEventInsert {
    pub log_id: Option<i64>,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub event: ExtractedHookEvent,
}

/// Insert `events` inside an existing transaction with `INSERT OR IGNORE`
/// (idempotent on the `UNIQUE(ai_tool, ai_session_id, hook_event, hook_name,
/// timestamp, evidence_kind)` constraint). Returns the number of rows
/// actually inserted (excludes ignored duplicates) via SQLite `changes()`
/// summed per statement.
pub(crate) fn insert_hook_events_in_tx(
    tx: &Transaction<'_>,
    events: &[HookEventInsert],
) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO ai_hook_events (
            log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
            hook_event, hook_name, hook_source, hook_command, status,
            exit_code, duration_ms, stdout_preview, stderr_preview,
            persisted_output_path, trusted_hash, evidence_kind, metadata_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
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
            item.event.hook_event,
            item.event.hook_name,
            item.event.hook_source,
            item.event.hook_command,
            item.event.status.as_str(),
            item.event.exit_code,
            item.event.duration_ms,
            item.event.stdout_preview,
            item.event.stderr_preview,
            item.event.persisted_output_path,
            item.event.trusted_hash,
            item.event.evidence_kind.as_str(),
            item.event.metadata_json,
        ])?;
        inserted += changed;
    }
    Ok(inserted)
}

/// Pool-acquiring wrapper for callers outside an existing transaction (e.g.
/// the backfill service and the config-inventory collector CLI path, both of
/// which own their own transaction boundary).
pub fn insert_hook_events(pool: &DbPool, events: &[HookEventInsert]) -> Result<usize> {
    let mut conn = crate::db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let inserted = insert_hook_events_in_tx(&tx, events)?;
    tx.commit()?;
    if inserted > 0 {
        super::agent_observatory::notify_projection_work();
    }
    Ok(inserted)
}

#[derive(Debug, Clone, Default)]
pub struct AiHookEventParams {
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub status: Option<String>,
    pub evidence_kind: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiHookEventEntry {
    pub id: i64,
    pub log_id: Option<i64>,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub hook_event: String,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub hook_command: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub persisted_output_path: Option<String>,
    pub trusted_hash: Option<String>,
    pub evidence_kind: String,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHookEventsResult {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<AiHookEventEntry>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

const HOOK_EVENT_COLUMNS: &str =
    "id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
     hook_event, hook_name, hook_source, hook_command, status, exit_code,
     duration_ms, stdout_preview, stderr_preview, persisted_output_path,
     trusted_hash, evidence_kind, metadata_json";

pub(crate) fn map_hook_event_row(row: &rusqlite::Row) -> rusqlite::Result<AiHookEventEntry> {
    Ok(AiHookEventEntry {
        id: row.get(0)?,
        log_id: row.get(1)?,
        ai_tool: row.get(2)?,
        ai_project: row.get(3)?,
        ai_session_id: row.get(4)?,
        hostname: row.get(5)?,
        timestamp: row.get(6)?,
        hook_event: row.get(7)?,
        hook_name: row.get(8)?,
        hook_source: row.get(9)?,
        hook_command: row.get(10)?,
        status: row.get(11)?,
        exit_code: row.get(12)?,
        duration_ms: row.get(13)?,
        stdout_preview: row.get(14)?,
        stderr_preview: row.get(15)?,
        persisted_output_path: row.get(16)?,
        trusted_hash: row.get(17)?,
        evidence_kind: row.get(18)?,
        metadata_json: row.get(19)?,
    })
}

/// List `ai_hook_events` rows newest-first, applying every non-`None`
/// filter in `params` as an `AND`-ed equality/range clause. `limit` is
/// clamped to `[1, 500]`; `truncated` is `true` when more rows matched than
/// were returned (probed via `LIMIT + 1`, mirroring `list_skill_events`).
pub fn list_hook_events(pool: &DbPool, params: &AiHookEventParams) -> Result<ListHookEventsResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;

    let mut sql = format!("SELECT {HOOK_EVENT_COLUMNS} FROM ai_hook_events WHERE 1 = 1");
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
    bind_eq!("hook_event", &params.hook_event);
    bind_eq!("hook_name", &params.hook_name);
    bind_eq!("hook_source", &params.hook_source);
    bind_eq!("status", &params.status);
    bind_eq!("evidence_kind", &params.evidence_kind);
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
        .query_map(
            rusqlite::params_from_iter(bindings.iter()),
            map_hook_event_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let truncated = rows.len() > limit;
    rows.truncate(limit);
    Ok(ListHookEventsResult {
        total: rows.len(),
        truncated,
        events: rows,
    })
}

#[cfg(test)]
#[path = "hook_events_tests.rs"]
mod tests;
