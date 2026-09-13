//! `sessions mcp-events backfill` — scans existing `logs` rows with `ai_tool
//! IN ('claude','codex')` and extracts/persists `ai_mcp_events` for rows
//! that predate this phase's ingest-time wiring
//! (`src/scanner.rs::flush_chunk`). Mirrors
//! `src/app/services/skill_backfill.rs` exactly: chunked scan-and-release,
//! no write lock around the pure SELECT, hard-clamped limit, and a
//! process-wide single-flight guard.

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use tokio::sync::Semaphore;

use crate::db::{DbPool, McpEventInsert, insert_mcp_events};
use crate::scanner::mcp_events::{extract_claude_mcp_events, extract_codex_mcp_events};

use super::super::models::{McpBackfillRequest, McpBackfillResult};
use super::super::time::parse_optional_timestamp;
use super::{CortexService, ServiceError, ServiceResult};

const CHUNK_SIZE: i64 = 2_000;

/// Same rationale as `skill_backfill::MAX_BACKFILL_LIMIT` (eng review Fix
/// 7): hard upper bound so a caller cannot drive an unbounded scan.
const MAX_BACKFILL_LIMIT: u64 = 1_000_000;

/// Process-wide single-flight gate for `backfill_mcp_events`, mirroring
/// `skill_backfill::backfill_guard` — a separate guard so an MCP backfill
/// and a skill-events backfill can run concurrently (they scan/write
/// disjoint tables) while still serializing against themselves.
fn backfill_guard() -> Arc<Semaphore> {
    static GUARD: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(GUARD.get_or_init(|| Arc::new(Semaphore::new(1))))
}

struct CandidateRow {
    id: i64,
    ai_tool: String,
    ai_project: Option<String>,
    ai_session_id: Option<String>,
    hostname: String,
    timestamp: String,
    ai_transcript_path: Option<String>,
    metadata_json: Option<String>,
    raw_json: Option<String>,
}

impl CortexService {
    pub async fn backfill_mcp_events(
        &self,
        req: McpBackfillRequest,
    ) -> ServiceResult<McpBackfillResult> {
        let since = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let limit = req.limit.unwrap_or(10_000).clamp(1, MAX_BACKFILL_LIMIT);
        let dry_run = req.dry_run;

        let _permit = backfill_guard()
            .try_acquire_owned()
            .map_err(|_| ServiceError::Busy("mcp event backfill already running".into()))?;

        self.run_db("backfill_mcp_events", move |pool| {
            run_backfill(pool, since.as_deref(), limit, dry_run)
        })
        .await
    }
}

fn run_backfill(
    pool: &DbPool,
    since: Option<&str>,
    limit: u64,
    dry_run: bool,
) -> Result<McpBackfillResult> {
    let mut result = McpBackfillResult {
        dry_run,
        ..Default::default()
    };
    let mut last_id = 0i64;
    let mut remaining = limit;

    loop {
        if remaining == 0 {
            result.truncated = true;
            break;
        }
        let chunk_limit = CHUNK_SIZE.min(remaining as i64).max(1);
        // No write_lock() here — pure SELECT, WAL gives a consistent
        // snapshot (mirrors skill_backfill's eng review Fix 6).
        let conn = pool.get()?;
        let rows = fetch_candidate_chunk(&conn, since, last_id, chunk_limit)?;
        drop(conn);

        if rows.is_empty() {
            break;
        }
        last_id = rows.last().map(|r| r.id).unwrap_or(last_id);
        result.scanned += rows.len() as u64;
        remaining = remaining.saturating_sub(rows.len() as u64);

        let recovered = super::transcript_recovery::recover(rows.iter().map(|row| {
            (
                row.id,
                row.ai_transcript_path.as_deref(),
                row.metadata_json.as_deref(),
            )
        }));
        let mut inserts = Vec::new();
        for row in &rows {
            let raw_json = if row.ai_transcript_path.is_some() {
                recovered.get(&row.id)
            } else {
                row.raw_json.as_ref()
            };
            let Some(raw_json) = raw_json else {
                result.source_unavailable += 1;
                continue;
            };
            let extracted = match serde_json::from_str::<serde_json::Value>(raw_json) {
                Ok(value) => match row.ai_tool.as_str() {
                    "claude" => extract_claude_mcp_events(&value),
                    "codex" => extract_codex_mcp_events(&value),
                    _ => continue,
                },
                Err(_) => {
                    result.parse_errors += 1;
                    continue;
                }
            };
            for event in extracted {
                inserts.push(McpEventInsert {
                    log_id: row.id,
                    ai_tool: row.ai_tool.clone(),
                    ai_project: row.ai_project.clone(),
                    ai_session_id: row.ai_session_id.clone(),
                    hostname: row.hostname.clone(),
                    timestamp: row.timestamp.clone(),
                    event,
                });
            }
        }

        if !dry_run && !inserts.is_empty() {
            let attempted = inserts.len() as u64;
            let inserted = insert_mcp_events(pool, &inserts)? as u64;
            result.inserted += inserted;
            result.skipped_duplicates += attempted - inserted;
        }

        if (rows.len() as i64) < chunk_limit {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    Ok(result)
}

/// Fetch display rows plus provenance for recovery of the original transcript.
fn fetch_candidate_chunk(
    conn: &rusqlite::Connection,
    since: Option<&str>,
    last_id: i64,
    chunk_limit: i64,
) -> Result<Vec<CandidateRow>> {
    let (sql, bindings): (&str, Vec<rusqlite::types::Value>) = match since {
        Some(since) => (
            "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, raw, ai_transcript_path, metadata_json
             FROM logs
             WHERE ai_tool IN ('claude', 'codex')
               AND id > ?1
               AND timestamp >= ?2
             ORDER BY id ASC
             LIMIT ?3",
            vec![
                rusqlite::types::Value::Integer(last_id),
                rusqlite::types::Value::Text(since.to_string()),
                rusqlite::types::Value::Integer(chunk_limit),
            ],
        ),
        None => (
            "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, raw, ai_transcript_path, metadata_json
             FROM logs
             WHERE ai_tool IN ('claude', 'codex')
               AND id > ?1
             ORDER BY id ASC
             LIMIT ?2",
            vec![
                rusqlite::types::Value::Integer(last_id),
                rusqlite::types::Value::Integer(chunk_limit),
            ],
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(CandidateRow {
                id: row.get(0)?,
                ai_tool: row.get(1)?,
                ai_project: row.get(2)?,
                ai_session_id: row.get(3)?,
                hostname: row.get(4)?,
                timestamp: row.get(5)?,
                raw_json: row.get(6)?,
                ai_transcript_path: row.get(7)?,
                metadata_json: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
#[path = "mcp_backfill_tests.rs"]
mod tests;
