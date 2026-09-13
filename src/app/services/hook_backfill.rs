//! `sessions hooks backfill` — scans existing `logs` rows with `ai_tool =
//! 'claude'` and extracts/persists `ai_hook_events` (runtime_transcript
//! evidence) for rows that predate this phase's ingest-time wiring
//! (`src/scanner.rs::flush_chunk`). Mirrors `super::skill_backfill`
//! one-for-one; see that module's rustdoc for the chunked scan-and-release,
//! WAL-snapshot read (no write lock on the SELECT), hard limit clamp, and
//! single-flight guard rationale.
//!
//! Scope: ONLY the Claude runtime-transcript hook path is backfilled here.
//! Config-inventory / trusted-hash-state evidence is a point-in-time read of
//! local host config files (see `crate::hook_config`), not something derivable
//! from transcript history, so it is collected live by `cortex assess hooks`
//! rather than backfilled. Codex has no runtime hook shape, so Codex rows are
//! skipped entirely.

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use tokio::sync::Semaphore;

use crate::db::{DbPool, HookEventInsert, insert_hook_events};
use crate::scanner::hook_events::extract_claude_hook_events;

use super::super::models::{HookBackfillRequest, HookBackfillResult};
use super::super::time::parse_optional_timestamp;
use super::{CortexService, ServiceError, ServiceResult};

const CHUNK_SIZE: i64 = 2_000;

/// Hard upper bound on `HookBackfillRequest.limit` — same rationale and value
/// as `skill_backfill::MAX_BACKFILL_LIMIT`.
const MAX_BACKFILL_LIMIT: u64 = 1_000_000;

/// Process-wide single-flight gate for `backfill_hook_events`. Separate guard
/// from the skill backfill so a hook backfill and a skill backfill can run
/// concurrently, but two hook backfills cannot.
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
    message: String,
}

impl CortexService {
    pub async fn backfill_hook_events(
        &self,
        req: HookBackfillRequest,
    ) -> ServiceResult<HookBackfillResult> {
        let since = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let limit = req.limit.unwrap_or(10_000).clamp(1, MAX_BACKFILL_LIMIT);
        let dry_run = req.dry_run;

        let _permit = backfill_guard()
            .try_acquire_owned()
            .map_err(|_| ServiceError::Busy("hook event backfill already running".into()))?;

        self.run_db("backfill_hook_events", move |pool| {
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
) -> Result<HookBackfillResult> {
    let mut result = HookBackfillResult {
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
            let message = if row.ai_transcript_path.is_some() {
                let Some(record) = recovered.get(&row.id) else {
                    result.source_unavailable += 1;
                    continue;
                };
                record
            } else {
                &row.message
            };
            let extracted = match row.ai_tool.as_str() {
                "claude" if message.contains("hook_") => {
                    match serde_json::from_str::<serde_json::Value>(message) {
                        Ok(value) => extract_claude_hook_events(&value),
                        Err(_) => {
                            result.parse_errors += 1;
                            continue;
                        }
                    }
                }
                _ => continue,
            };
            for event in extracted {
                inserts.push(HookEventInsert {
                    log_id: Some(row.id),
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
            let inserted = insert_hook_events(pool, &inserts)? as u64;
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

fn fetch_candidate_chunk(
    conn: &rusqlite::Connection,
    since: Option<&str>,
    last_id: i64,
    chunk_limit: i64,
) -> Result<Vec<CandidateRow>> {
    let (sql, bindings): (&str, Vec<rusqlite::types::Value>) = match since {
        Some(since) => (
            "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, message, ai_transcript_path, metadata_json
             FROM logs
             WHERE ai_tool = 'claude'
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
            "SELECT id, ai_tool, ai_project, ai_session_id, hostname, timestamp, message, ai_transcript_path, metadata_json
             FROM logs
             WHERE ai_tool = 'claude'
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
                message: row.get(6)?,
                ai_transcript_path: row.get(7)?,
                metadata_json: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
#[path = "hook_backfill_tests.rs"]
mod tests;
