//! Bounded source-table pages for Agent Observatory projection.

use crate::db::DbPool;
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};

const MAX_PAGE: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSourceKind {
    Mcp,
    Hook,
    Skill,
    Llm,
}

impl AgentSourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp_events",
            Self::Hook => "hook_events",
            Self::Skill => "skill_events",
            Self::Llm => "llm_invocations",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMcpSourceRow {
    pub cursor_id: i64,
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
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHookSourceRow {
    pub cursor_id: i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillSourceRow {
    pub cursor_id: i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLlmSourceRow {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub caller_surface: String,
    pub action: String,
    pub provider: String,
    pub model: Option<String>,
    pub program: Option<String>,
    pub incident_id: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub evidence_counts_json: Option<String>,
    pub prompt_bytes: Option<i64>,
    pub output_bytes: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSourceRecord {
    Mcp(AgentMcpSourceRow),
    Hook(AgentHookSourceRow),
    Skill(AgentSkillSourceRow),
    Llm(AgentLlmSourceRow),
}

impl AgentSourceRecord {
    pub(crate) fn next_cursor(&self) -> String {
        match self {
            Self::Mcp(row) => row.cursor_id.to_string(),
            Self::Hook(row) => row.cursor_id.to_string(),
            Self::Skill(row) => row.cursor_id.to_string(),
            Self::Llm(row) => serde_json::to_string(&LlmCursor {
                ready_at: row
                    .finished_at
                    .clone()
                    .unwrap_or_else(|| row.started_at.clone()),
                id: row.id.clone(),
            })
            .expect("LLM cursor serialization cannot fail"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSourcePage {
    pub records: Vec<AgentSourceRecord>,
    pub next_cursor: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LlmCursor {
    #[serde(alias = "started_at")]
    ready_at: String,
    id: String,
}

fn validate_page(limit: usize) -> Result<i64> {
    if limit == 0 || limit > MAX_PAGE {
        bail!("limit must be between 1 and {MAX_PAGE}");
    }
    i64::try_from(limit + 1).context("page limit exceeds SQLite integer range")
}

fn numeric_cursor(cursor: &str) -> Result<i64> {
    if cursor.is_empty() {
        return Ok(0);
    }
    let value = cursor
        .parse::<i64>()
        .with_context(|| format!("invalid numeric source cursor: {cursor}"))?;
    if value < 0 {
        bail!("numeric source cursor must be non-negative");
    }
    Ok(value)
}

fn llm_cursor(cursor: &str) -> Result<Option<LlmCursor>> {
    if cursor.is_empty() {
        return Ok(None);
    }
    let cursor: LlmCursor = serde_json::from_str(cursor).context("invalid LLM source cursor")?;
    if cursor.id.is_empty() || chrono::DateTime::parse_from_rfc3339(&cursor.ready_at).is_err() {
        bail!("invalid LLM source cursor fields");
    }
    Ok(Some(cursor))
}

fn mcp_page(conn: &rusqlite::Connection, after: i64, limit: i64) -> Result<Vec<AgentSourceRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, call_log_id, result_log_id, ai_tool, ai_project, ai_session_id,
                hostname, timestamp, turn_id, call_id, tool_name, mcp_server, mcp_tool,
                event_kind, status, duration_ms, is_error, arguments_json, output_preview,
                error_text, metadata_json
           FROM ai_mcp_events WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    Ok(stmt
        .query_map(params![after, limit], |row| {
            Ok(AgentSourceRecord::Mcp(AgentMcpSourceRow {
                cursor_id: row.get(0)?,
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
                is_error: row.get(16)?,
                arguments_json: row.get(17)?,
                output_preview: row.get(18)?,
                error_text: row.get(19)?,
                metadata_json: row.get(20)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn hook_page(
    conn: &rusqlite::Connection,
    after: i64,
    limit: i64,
) -> Result<Vec<AgentSourceRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                hook_event, hook_name, hook_source, hook_command, status, exit_code,
                duration_ms, stdout_preview, stderr_preview, persisted_output_path,
                trusted_hash, evidence_kind, metadata_json
           FROM ai_hook_events WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    Ok(stmt
        .query_map(params![after, limit], |row| {
            Ok(AgentSourceRecord::Hook(AgentHookSourceRow {
                cursor_id: row.get(0)?,
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
            }))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn skill_page(
    conn: &rusqlite::Connection,
    after: i64,
    limit: i64,
) -> Result<Vec<AgentSourceRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                skill_name, skill_plugin, event_kind, evidence_kind
           FROM ai_skill_events WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    Ok(stmt
        .query_map(params![after, limit], |row| {
            Ok(AgentSourceRecord::Skill(AgentSkillSourceRow {
                cursor_id: row.get(0)?,
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
            }))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn llm_page(
    conn: &rusqlite::Connection,
    after: Option<&LlmCursor>,
    limit: i64,
) -> Result<Vec<AgentSourceRecord>> {
    let mut sql = String::from(
        "SELECT id, started_at, finished_at, duration_ms, caller_surface, action,
                provider, model, program, incident_id, ai_tool, ai_project, ai_session_id,
                evidence_counts_json, prompt_bytes, output_bytes, status, error, metadata_json
           FROM llm_invocations
          WHERE finished_at IS NOT NULL",
    );
    let mut bindings = Vec::<rusqlite::types::Value>::new();
    if let Some(after) = after {
        sql.push_str(" AND (finished_at, id) > (?1, ?2)");
        bindings.push(rusqlite::types::Value::Text(after.ready_at.clone()));
        bindings.push(rusqlite::types::Value::Text(after.id.clone()));
    }
    let limit_parameter = bindings.len() + 1;
    sql.push_str(&format!(
        " ORDER BY finished_at, id LIMIT ?{limit_parameter}"
    ));
    bindings.push(rusqlite::types::Value::Integer(limit));

    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(AgentSourceRecord::Llm(AgentLlmSourceRow {
                id: row.get(0)?,
                started_at: row.get(1)?,
                finished_at: row.get(2)?,
                duration_ms: row.get(3)?,
                caller_surface: row.get(4)?,
                action: row.get(5)?,
                provider: row.get(6)?,
                model: row.get(7)?,
                program: row.get(8)?,
                incident_id: row.get(9)?,
                ai_tool: row.get(10)?,
                ai_project: row.get(11)?,
                ai_session_id: row.get(12)?,
                evidence_counts_json: row.get(13)?,
                prompt_bytes: row.get(14)?,
                output_bytes: row.get(15)?,
                status: row.get(16)?,
                error: row.get(17)?,
                metadata_json: row.get(18)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn page_agent_sources(
    pool: &DbPool,
    kind: AgentSourceKind,
    after_cursor: &str,
    limit: usize,
) -> Result<AgentSourcePage> {
    let probe_limit = validate_page(limit)?;
    let conn = pool.get().context("acquire database connection")?;
    let mut records = match kind {
        AgentSourceKind::Mcp => mcp_page(&conn, numeric_cursor(after_cursor)?, probe_limit)?,
        AgentSourceKind::Hook => hook_page(&conn, numeric_cursor(after_cursor)?, probe_limit)?,
        AgentSourceKind::Skill => skill_page(&conn, numeric_cursor(after_cursor)?, probe_limit)?,
        AgentSourceKind::Llm => {
            let after = llm_cursor(after_cursor)?;
            llm_page(&conn, after.as_ref(), probe_limit)?
        }
    };
    let truncated = records.len() > limit;
    records.truncate(limit);
    let next_cursor = records
        .last()
        .map_or_else(|| after_cursor.to_string(), AgentSourceRecord::next_cursor);
    Ok(AgentSourcePage {
        records,
        next_cursor,
        truncated,
    })
}

#[cfg(test)]
#[path = "agent_observatory_sources_tests.rs"]
mod tests;
