//! Background scan job for error signature detection.
//!
//! `run_error_scan` processes log rows in ≤200-row chunks, advancing the
//! cursor atomically after each chunk.  Between chunks it yields for 100ms to
//! avoid monopolising the SQLite write lock (ingest batch writer has a 375ms
//! retry budget: 25/100/250ms).
//!
//! The caller (runtime.rs) is responsible for:
//! - Acquiring `maintenance_permit` per chunk (passed in via the closure).
//! - Calling this on a cadence controlled by `ErrorDetectionConfig`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Semaphore;

use super::normalize::{NORMALIZER_VERSION, normalize_template, signature_hash};
use crate::config::ErrorDetectionConfig;
use crate::db::DbPool;
use crate::db::error_signatures::{
    UpsertSignatureParams, cursor_advance, cursor_get, insert_window, upsert_signature,
};
use crate::receiver::enrichment::scrub_ai_message;

/// A minimal log row fetched for scanning.
#[derive(Debug)]
struct ScanRow {
    id: i64,
    severity: String,
    message: String,
    timestamp: String,
    hostname: String,
    app_name: Option<String>,
}

/// Run one complete error-scan cycle.
///
/// * Reads the cursor.
/// * Processes rows in `chunk_size`-row chunks (≤200).
/// * Stops when no more rows or `max_rows` reached.
/// * Acquires `maintenance_permit` per chunk.
pub(crate) async fn run_error_scan(
    pool: Arc<DbPool>,
    permit_sem: Arc<Semaphore>,
    cfg: ErrorDetectionConfig,
) -> Result<u64> {
    let chunk_size: i64 = 200;
    let max_rows = cfg.max_rows_per_cycle as i64;
    let mut total_processed: i64 = 0;

    // Read the starting cursor (cheap read, no permit needed).
    let mut last_id = {
        let p = Arc::clone(&pool);
        tokio::task::spawn_blocking(move || cursor_get(&p)).await??
    };

    loop {
        if total_processed >= max_rows {
            break;
        }

        // Acquire maintenance permit before touching the DB.
        let Ok(permit) = Arc::clone(&permit_sem).acquire_owned().await else {
            tracing::error!("error_scan: maintenance semaphore closed, aborting scan");
            break;
        };

        let pool_chunk = Arc::clone(&pool);
        let frequency_threshold = cfg.frequency_threshold;
        let exclude_patterns = cfg.exclude_patterns.clone();
        // Notifications fire only for signatures at/above this severity floor;
        // resolve to a rank once (lower = more severe). Unknown values are
        // rejected at config validation, so a parse failure here means "no
        // floor" (notify everything) rather than silently dropping pages.
        let notify_floor_rank =
            crate::db::severity_to_num(&cfg.notify_min_severity).unwrap_or(u8::MAX);
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            process_chunk(
                &pool_chunk,
                last_id,
                chunk_size,
                frequency_threshold,
                &exclude_patterns,
                notify_floor_rank,
            )
        })
        .await??;

        if result.rows_in_chunk == 0 {
            // No more rows to scan.
            break;
        }

        last_id = result.new_cursor;
        total_processed += result.rows_in_chunk;

        // Yield between chunks to avoid starving the ingest writer.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    Ok(total_processed as u64)
}

pub(crate) struct ChunkResult {
    pub(crate) rows_in_chunk: i64,
    pub(crate) new_cursor: i64,
}

/// Process one chunk of log rows inside a single rusqlite transaction.
/// Returns the number of rows processed and the new cursor position.
pub(crate) fn process_chunk(
    pool: &DbPool,
    last_id: i64,
    chunk_size: i64,
    frequency_threshold: u32,
    exclude_patterns: &[String],
    notify_floor_rank: u8,
) -> Result<ChunkResult> {
    // --- Fetch rows ---
    //
    // Scoped so the read connection is back in the pool before the write phase
    // queues on the process-wide write lock. Grouping below is pure CPU work on
    // the fetched rows and needs no connection at all.
    let rows: Vec<ScanRow> = {
        let conn = pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, severity, message, timestamp, hostname, app_name
             FROM logs
             WHERE id > ?1
               AND severity IN ('warning', 'err', 'crit', 'alert', 'emerg')
             ORDER BY id ASC
             LIMIT ?2",
        )?;

        stmt.query_map(rusqlite::params![last_id, chunk_size], |row| {
            Ok(ScanRow {
                id: row.get(0)?,
                severity: row.get(1)?,
                message: row.get(2)?,
                timestamp: row.get(3)?,
                hostname: row.get(4)?,
                app_name: row.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };

    if rows.is_empty() {
        return Ok(ChunkResult {
            rows_in_chunk: 0,
            new_cursor: last_id,
        });
    }

    // Safe: `rows.is_empty()` was just checked above.
    let max_id = rows.last().expect("rows non-empty").id;

    // --- Group rows by (signature_hash, normalizer_version) ---
    // Key: (hash, template, sample_message, sample_hostname, sample_app_name, severity,
    //       first_ts, last_ts, count)
    #[derive(Debug)]
    struct Group {
        template: String,
        sample_message: String,
        sample_hostname: String,
        sample_app_name: Option<String>,
        severity: String,
        first_ts: String,
        last_ts: String,
        count: i64,
    }

    let mut groups: HashMap<String, Group> = HashMap::new();

    // Lowercased exclusion needles, computed once per chunk. Rows whose message
    // matches are skipped entirely (not tracked as signatures and never
    // notified). The cursor still advances past them via `max_id` above, so the
    // skip is permanent rather than reprocessed next cycle.
    let exclude_needles: Vec<String> = exclude_patterns
        .iter()
        .map(|p| p.to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();

    for row in &rows {
        if !exclude_needles.is_empty() {
            let message_lc = row.message.to_lowercase();
            if exclude_needles.iter().any(|n| message_lc.contains(n)) {
                continue;
            }
        }

        let template = normalize_template(&row.message);
        let hash = signature_hash(&template);

        // Scrub sample message unconditionally (no is_ai_source gate per spec).
        let scrubbed = scrub_ai_message(&row.message, None);

        let entry = groups.entry(hash).or_insert_with(|| Group {
            template: template.clone(),
            sample_message: scrubbed.clone(),
            sample_hostname: row.hostname.clone(),
            sample_app_name: row.app_name.clone(),
            severity: row.severity.clone(),
            first_ts: row.timestamp.clone(),
            last_ts: row.timestamp.clone(),
            count: 0,
        });
        entry.count += 1;
        if row.timestamp < entry.first_ts {
            entry.first_ts = row.timestamp.clone();
        }
        if row.timestamp > entry.last_ts {
            entry.last_ts = row.timestamp.clone();
        }
    }

    // --- Write signatures and windows in a single transaction ---
    let mut conn = crate::db::write_conn(pool)?;
    let tx = conn.transaction()?;

    for (hash, group) in &groups {
        upsert_signature(
            &tx,
            UpsertSignatureParams {
                hash,
                normalizer_version: NORMALIZER_VERSION,
                template: &group.template,
                sample_message: &group.sample_message,
                sample_hostname: &group.sample_hostname,
                sample_app_name: group.sample_app_name.as_deref(),
                severity: &group.severity,
                first_seen_at: &group.first_ts,
                last_seen_at: &group.last_ts,
                delta: group.count,
            },
        )?;

        insert_window(
            &tx,
            hash,
            NORMALIZER_VERSION,
            &group.first_ts,
            &group.last_ts,
            group.count,
        )?;

        // Insert into outbox if the signature is unaddressed and above
        // the frequency threshold. Use the 1-hour window count (not the
        // lifetime total_count) to avoid alerting on long-ago bursts.
        // The dispatcher's dedup_window_secs prevents duplicate notifications.
        let ack_check: rusqlite::Result<(i64, Option<String>)> = tx.query_row(
            "SELECT
                 COALESCE((
                     SELECT SUM(count_in_window)
                     FROM error_signature_windows
                     WHERE signature_hash = ?1 AND normalizer_version = ?2
                       AND window_end >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 hour')
                 ), 0),
                 acknowledged_at
             FROM error_signatures
             WHERE signature_hash = ?1 AND normalizer_version = ?2",
            rusqlite::params![hash, NORMALIZER_VERSION],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        // Severity floor: warning-level recurrences are recorded but must not
        // page (lower rank = more severe, so the signature must rank at/below
        // the floor). Unknown severities (rank MAX) never meet a real floor.
        let meets_severity_floor = crate::db::severity_to_num(&group.severity)
            .map(|rank| rank <= notify_floor_rank)
            .unwrap_or(false);
        let should_notify = match ack_check {
            Ok((count_last_1h, acknowledged_at)) => {
                meets_severity_floor
                    && acknowledged_at.is_none()
                    && count_last_1h >= frequency_threshold as i64
            }
            Err(e) => {
                tracing::warn!(
                    signature_hash = %hash,
                    error = %e,
                    "error_scan: ack-check query failed; treating as unacked (fail-open)"
                );
                // fail open within the severity floor: a duplicate err+ page is
                // recoverable, a missed one is not — but never page below floor.
                meets_severity_floor
            }
        };

        if should_notify {
            let outbox_params = crate::db::notifications::OutboxInsertParams {
                dedup_key: format!("error_sig:{hash}"),
                rule_id: "unaddressed_error_signature".to_string(),
                severity: group.severity.clone(),
                hostname: group.sample_hostname.clone(),
                title: format!(
                    "[{}] Recurring error on {}",
                    group.severity.to_uppercase(),
                    group.sample_hostname
                ),
                body: format!(
                    "Signature: {}\nSample: {}\nOccurrences: {}",
                    group.template, group.sample_message, group.count
                ),
                apprise_urls_json: "[]".to_string(), // overridden by dispatcher config
                next_attempt_at: chrono::Utc::now()
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string(),
            };
            if let Err(e) = crate::db::notifications::outbox_insert(&tx, &outbox_params) {
                tracing::warn!(
                    signature_hash = %hash,
                    error = %e,
                    "error_scan: failed to insert outbox notification (non-fatal)"
                );
            }
        }
    }

    cursor_advance(&tx, max_id)?;

    tx.commit()?;

    tracing::debug!(
        rows = rows.len(),
        groups = groups.len(),
        new_cursor = max_id,
        "error_scan: chunk processed"
    );

    Ok(ChunkResult {
        rows_in_chunk: rows.len() as i64,
        new_cursor: max_id,
    })
}

#[cfg(test)]
#[path = "scanner_tests.rs"]
mod tests;
