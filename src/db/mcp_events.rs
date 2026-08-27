//! `ai_mcp_events` insert + list query layer. Table/columns are defined in
//! migration 39 (`src/db/pool.rs`). Extraction happens in
//! `crate::scanner::mcp_events`; this module only persists and reads back
//! already-extracted events.
//!
//! Unlike `ai_skill_events` (one `ChunkSkillSource` per log row, at most one
//! event emitted), a single transcript line can carry multiple `tool_use`
//! blocks, and a call's paired result frequently lands on a LATER log row
//! (or a different chunk/transaction entirely — Claude's `tool_result`
//! rows are separate transcript lines from their `tool_use` call). So
//! `insert_mcp_events_in_tx` intentionally does not try to join call/result
//! rows itself: each extracted event (call or result) is inserted as its
//! own row keyed by `(ai_tool, ai_session_id, call_id, event_kind)`, and
//! `mcp_tool`/`mcp_server`/`tool_name` on a bare result row are populated
//! via a best-effort backfill-join against the earlier call row in the same
//! statement batch (see `resolve_result_tool_name_in_tx`) — a result event
//! extracted with an empty `tool_name` (see
//! `scanner::mcp_events::extract_claude_mcp_events`) looks up its sibling
//! call row by `(ai_tool, ai_session_id, call_id)` and copies
//! `tool_name`/`mcp_server`/`mcp_tool` forward so incident grouping (keyed
//! on `mcp_server`/`mcp_tool`) still works for result-only anchors.

use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::scanner::mcp_events::ExtractedMcpEvent;

use super::pool::DbPool;

#[derive(Debug, Clone)]
pub struct McpEventInsert {
    pub log_id: i64,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub event: ExtractedMcpEvent,
}

/// Resolved `(tool_name, mcp_server, mcp_tool)` for a result event copied
/// forward from its paired call row.
type ResolvedCallIdentity = (String, Option<String>, Option<String>);

/// Look up `tool_name`/`mcp_server`/`mcp_tool` from the paired call row for
/// a result event that didn't carry its own tool name (Claude's
/// `tool_result` shape never repeats the tool name — only Codex's
/// `function_call_output` could in principle, and it doesn't either). Falls
/// back to empty/`NULL` when no matching call row has been inserted yet
/// (e.g. backfill processing results before their call, or the call row
/// fell outside the backfill's scan window) — the row is still inserted so
/// incident detection on `is_error`/`status` isn't lost, it's just
/// unclassified until a later re-ingest or backfill pass fills it in.
fn resolve_result_tool_name_in_tx(
    tx: &Transaction<'_>,
    ai_tool: &str,
    ai_session_id: Option<&str>,
    call_id: &str,
) -> Result<Option<ResolvedCallIdentity>> {
    // Filters on COALESCE(ai_session_id, '') rather than `ai_session_id IS
    // ?2` so this lookup can use idx_ai_mcp_events_dedupe (built on the
    // same COALESCE expression) instead of falling back to the wider
    // idx_ai_mcp_events_session_time index. prepare_cached avoids
    // re-parsing this statement on every result-row insert (roughly half
    // of all ai_mcp_events rows, since calls and results are paired 1:1).
    let row: Option<ResolvedCallIdentity> = tx
        .prepare_cached(
            "SELECT tool_name, mcp_server, mcp_tool FROM ai_mcp_events
             WHERE ai_tool = ?1
               AND COALESCE(ai_session_id, '') = COALESCE(?2, '')
               AND call_id = ?3 AND event_kind = 'call'
             LIMIT 1",
        )?
        .query_row(params![ai_tool, ai_session_id, call_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    Ok(row)
}

/// Insert `events` inside an existing transaction with `INSERT OR IGNORE`
/// (idempotent on the `UNIQUE(ai_tool, ai_session_id, call_id, event_kind)`
/// constraint). Returns the number of rows actually inserted (excludes
/// ignored duplicates).
pub(crate) fn insert_mcp_events_in_tx(
    tx: &Transaction<'_>,
    events: &[McpEventInsert],
) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0usize;
    for item in events {
        let (tool_name, mcp_server, mcp_tool) = if item.event.event_kind
            == crate::scanner::mcp_events::McpEventKind::Result
            && item.event.tool_name.is_empty()
        {
            resolve_result_tool_name_in_tx(
                tx,
                &item.ai_tool,
                item.ai_session_id.as_deref(),
                &item.event.call_id,
            )?
            .unwrap_or((String::new(), None, None))
        } else {
            (
                item.event.tool_name.clone(),
                item.event.mcp_server.clone(),
                item.event.mcp_tool.clone(),
            )
        };
        let changed = tx.prepare_cached(
            "INSERT OR IGNORE INTO ai_mcp_events (
                call_log_id, result_log_id, ai_tool, ai_project, ai_session_id, hostname,
                timestamp, turn_id, call_id, tool_name, mcp_server, mcp_tool, event_kind,
                status, duration_ms, is_error, arguments_json, output_preview, error_text
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        )?.execute(params![
            (item.event.event_kind == crate::scanner::mcp_events::McpEventKind::Call)
                .then_some(item.log_id),
            (item.event.event_kind == crate::scanner::mcp_events::McpEventKind::Result)
                .then_some(item.log_id),
            item.ai_tool,
            item.ai_project,
            item.ai_session_id,
            item.hostname,
            item.timestamp,
            item.event.turn_id,
            item.event.call_id,
            tool_name,
            mcp_server,
            mcp_tool,
            item.event.event_kind.as_str(),
            item.event.status,
            Option::<i64>::None,
            item.event.is_error.map(i64::from),
            item.event.arguments_json,
            item.event.output_preview,
            item.event.error_text,
        ])?;
        inserted += changed;
    }
    Ok(inserted)
}

/// Pool-acquiring wrapper for callers outside an existing transaction (e.g.
/// the backfill service, which owns its own chunked transaction boundary).
pub fn insert_mcp_events(pool: &DbPool, events: &[McpEventInsert]) -> Result<usize> {
    let mut conn = crate::db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let inserted = insert_mcp_events_in_tx(&tx, events)?;
    tx.commit()?;
    if inserted > 0 {
        super::agent_observatory::notify_projection_work();
    }
    Ok(inserted)
}

#[derive(Debug, Clone, Default)]
pub struct AiMcpEventParams {
    pub tool_name: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: Option<String>,
    pub is_error: Option<bool>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMcpEventEntry {
    pub id: i64,
    pub call_log_id: Option<i64>,
    pub result_log_id: Option<i64>,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub turn_id: Option<String>,
    pub call_id: String,
    pub tool_name: String,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub event_kind: String,
    pub status: Option<String>,
    pub duration_ms: Option<i64>,
    pub is_error: Option<bool>,
    pub arguments_json: Option<String>,
    pub output_preview: Option<String>,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMcpEventsResult {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<AiMcpEventEntry>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

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

/// List `ai_mcp_events` rows newest-first, applying every non-`None` filter
/// in `params` as an `AND`-ed equality/range clause. `limit` is clamped to
/// `[1, 500]`; `truncated` is `true` when more rows matched than were
/// returned (probed via `LIMIT + 1`, mirroring `list_skill_events`).
pub fn list_mcp_events(pool: &DbPool, params: &AiMcpEventParams) -> Result<ListMcpEventsResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;

    let mut sql = format!("SELECT {MCP_EVENT_COLUMNS} FROM ai_mcp_events WHERE 1 = 1");
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
    bind_eq!("tool_name", &params.tool_name);
    bind_eq!("mcp_server", &params.mcp_server);
    bind_eq!("mcp_tool", &params.mcp_tool);
    bind_eq!("ai_tool", &params.ai_tool);
    bind_eq!("ai_project", &params.ai_project);
    bind_eq!("ai_session_id", &params.ai_session_id);
    bind_eq!("hostname", &params.hostname);
    if let Some(is_error) = params.is_error {
        sql.push_str(&format!(" AND is_error = ?{idx}"));
        bindings.push(rusqlite::types::Value::Integer(i64::from(is_error)));
        idx += 1;
    }
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
            map_mcp_event_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let truncated = rows.len() > limit;
    rows.truncate(limit);
    Ok(ListMcpEventsResult {
        total: rows.len(),
        truncated,
        events: rows,
    })
}

#[cfg(test)]
#[path = "mcp_events_tests.rs"]
mod tests;
