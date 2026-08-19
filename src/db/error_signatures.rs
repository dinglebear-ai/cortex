//! Database operations for the error signature detection subsystem.
//!
//! All functions take a `&r2d2::Pool<SqliteConnectionManager>` (i.e. `&DbPool`)
//! and are intended to be called from inside `tokio::task::spawn_blocking`.
//! They use rusqlite transactions, NOT sqlx.

use anyhow::Result;
use rusqlite::params;

use super::pool::DbPool;

// ---------------------------------------------------------------------------
// Cursor

/// Return the last scanned log ID from `error_scan_cursor`.
pub(crate) fn cursor_get(pool: &DbPool) -> Result<i64> {
    let conn = pool.get()?;
    let id: i64 = conn.query_row(
        "SELECT last_scanned_log_id FROM error_scan_cursor WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Advance the cursor to `new_last_id` and record the scan completion time.
pub(crate) fn cursor_advance(conn: &rusqlite::Connection, new_last_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE error_scan_cursor
         SET last_scanned_log_id = ?1,
             last_scan_completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = 1",
        params![new_last_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Upsert signature

/// Parameters for `upsert_signature`.
pub(crate) struct UpsertSignatureParams<'a> {
    pub hash: &'a str,
    pub normalizer_version: i64,
    pub template: &'a str,
    pub sample_message: &'a str,
    pub sample_hostname: &'a str,
    pub sample_app_name: Option<&'a str>,
    pub severity: &'a str,
    pub first_seen_at: &'a str,
    pub last_seen_at: &'a str,
    pub delta: i64,
}

/// Upsert a signature into `error_signatures`.
///
/// On INSERT (first time we see this hash+version): write all sample fields.
/// On UPDATE (already exists): advance `last_seen_at` and add `delta` to
/// `total_count`. Sample fields are NEVER overwritten.
pub(crate) fn upsert_signature(
    conn: &rusqlite::Connection,
    p: UpsertSignatureParams<'_>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO error_signatures
             (signature_hash, normalizer_version, template, sample_message,
              sample_hostname, sample_app_name, severity,
              first_seen_at, last_seen_at, total_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(signature_hash, normalizer_version) DO UPDATE SET
             last_seen_at  = CASE WHEN excluded.last_seen_at > last_seen_at
                                  THEN excluded.last_seen_at ELSE last_seen_at END,
             total_count   = total_count + excluded.total_count",
        params![
            p.hash,
            p.normalizer_version,
            p.template,
            p.sample_message,
            p.sample_hostname,
            p.sample_app_name,
            p.severity,
            p.first_seen_at,
            p.last_seen_at,
            p.delta,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Window

/// Insert a window record.  Overlapping windows for the same (hash, ver,
/// start, end) are merged via `ON CONFLICT … DO UPDATE`.
pub(crate) fn insert_window(
    conn: &rusqlite::Connection,
    signature_hash: &str,
    normalizer_version: i64,
    window_start: &str,
    window_end: &str,
    count: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO error_signature_windows
             (signature_hash, normalizer_version, window_start, window_end, count_in_window)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(signature_hash, normalizer_version, window_start, window_end)
         DO UPDATE SET count_in_window = count_in_window + excluded.count_in_window",
        params![
            signature_hash,
            normalizer_version,
            window_start,
            window_end,
            count
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Ack / unack

/// Record an ack or unack audit event.
pub(crate) fn record_ack_event(
    conn: &rusqlite::Connection,
    signature_hash: &str,
    normalizer_version: i64,
    event_type: &str, // "ack" | "unack"
    actor: &str,
    notes: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO error_signature_ack_events
             (signature_hash, normalizer_version, event_type, actor, notes)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![signature_hash, normalizer_version, event_type, actor, notes],
    )?;
    Ok(())
}

/// Update the ack projection column on `error_signatures`.
/// Call this after `record_ack_event` inside the same transaction.
pub(crate) fn update_ack_projection(
    conn: &rusqlite::Connection,
    signature_hash: &str,
    normalizer_version: i64,
    acknowledged_at: Option<&str>, // Some → ack, None → clear (unack)
    acknowledged_by: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE error_signatures
         SET acknowledged_at = ?3, acknowledged_by = ?4
         WHERE signature_hash = ?1 AND normalizer_version = ?2",
        params![
            signature_hash,
            normalizer_version,
            acknowledged_at,
            acknowledged_by,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Read queries

/// A row from `error_signatures` joined with a recent-window count.
#[derive(Debug)]
pub(crate) struct SignatureRow {
    pub signature_hash: String,
    pub template: String,
    pub sample_message: String,
    pub sample_hostname: String,
    pub sample_app_name: Option<String>,
    pub severity: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub total_count: i64,
    pub count_last_1h: i64,
    pub acknowledged_at: Option<String>,
}

/// Return a page of unacknowledged (or all, if `include_acknowledged`)
/// signatures for one normalizer version, ordered by `last_seen_at DESC`.
///
/// The operator API identifies signatures by hash only, while ack/unack targets
/// the active normalizer version. Filtering here keeps historical version rows
/// durable without exposing entries the current API cannot act on.
pub(crate) fn read_unaddressed_page(
    pool: &DbPool,
    normalizer_version: i64,
    limit: i64,
    offset: i64,
    include_acknowledged: bool,
) -> Result<Vec<SignatureRow>> {
    let conn = pool.get()?;
    let cutoff_1h = chrono::Utc::now()
        .checked_sub_signed(chrono::TimeDelta::hours(1))
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default();

    let filter_clause = if include_acknowledged {
        ""
    } else {
        "AND s.acknowledged_at IS NULL"
    };

    let sql = format!(
        "SELECT
             s.signature_hash,
             s.template,
             s.sample_message,
             s.sample_hostname,
             s.sample_app_name,
             s.severity,
             s.first_seen_at,
             s.last_seen_at,
             s.total_count,
             COALESCE(w.total_1h, 0) AS count_last_1h,
             s.acknowledged_at
         FROM error_signatures s
         LEFT JOIN (
             SELECT signature_hash, normalizer_version, SUM(count_in_window) AS total_1h
             FROM error_signature_windows
             WHERE window_end >= ?1 AND normalizer_version = ?2
             GROUP BY signature_hash, normalizer_version
         ) w USING (signature_hash, normalizer_version)
         WHERE s.normalizer_version = ?2 {filter_clause}
         ORDER BY s.last_seen_at DESC
         LIMIT ?3 OFFSET ?4"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![cutoff_1h, normalizer_version, limit, offset.max(0)],
        |row| {
            Ok(SignatureRow {
                signature_hash: row.get(0)?,
                template: row.get(1)?,
                sample_message: row.get(2)?,
                sample_hostname: row.get(3)?,
                sample_app_name: row.get(4)?,
                severity: row.get(5)?,
                first_seen_at: row.get(6)?,
                last_seen_at: row.get(7)?,
                total_count: row.get(8)?,
                count_last_1h: row.get(9)?,
                acknowledged_at: row.get(10)?,
            })
        },
    )?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Look up a single signature by hash and normalizer version. Returns `None` if not found.
///
/// The table PK is `(signature_hash, normalizer_version)`, so both parameters are
/// required to uniquely identify a row.
pub(crate) fn read_signature_by_hash(
    pool: &DbPool,
    signature_hash: &str,
    normalizer_version: i64,
) -> Result<Option<SignatureRow>> {
    let conn = pool.get()?;
    let cutoff_1h = chrono::Utc::now()
        .checked_sub_signed(chrono::TimeDelta::hours(1))
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default();

    // The `USING (...)` join form is load-bearing for performance (bead q2e8):
    // it lets SQLite push the outer `s.signature_hash = ?1` equality down into
    // the materialized aggregate subquery, so only this hash's windows are summed
    // (PK index seek, sub-ms). Rewriting it to an explicit `ON w.x = s.x` defeats
    // that pushdown and degrades to a full GROUP BY over error_signature_windows
    // (~100x slower at scale). Keep the `USING` form.
    let mut stmt = conn.prepare(
        "SELECT
             s.signature_hash,
             s.template,
             s.sample_message,
             s.sample_hostname,
             s.sample_app_name,
             s.severity,
             s.first_seen_at,
             s.last_seen_at,
             s.total_count,
             COALESCE(w.total_1h, 0) AS count_last_1h,
             s.acknowledged_at
         FROM error_signatures s
         LEFT JOIN (
             SELECT signature_hash, normalizer_version, SUM(count_in_window) AS total_1h
             FROM error_signature_windows
             WHERE window_end >= ?3
             GROUP BY signature_hash, normalizer_version
         ) w USING (signature_hash, normalizer_version)
         WHERE s.signature_hash = ?1 AND s.normalizer_version = ?2
         LIMIT 1",
    )?;

    let mut rows = stmt.query_map(
        params![signature_hash, normalizer_version, cutoff_1h],
        |row| {
            Ok(SignatureRow {
                signature_hash: row.get(0)?,
                template: row.get(1)?,
                sample_message: row.get(2)?,
                sample_hostname: row.get(3)?,
                sample_app_name: row.get(4)?,
                severity: row.get(5)?,
                first_seen_at: row.get(6)?,
                last_seen_at: row.get(7)?,
                total_count: row.get(8)?,
                count_last_1h: row.get(9)?,
                acknowledged_at: row.get(10)?,
            })
        },
    )?;

    rows.next().transpose().map_err(Into::into)
}

#[cfg(test)]
#[path = "error_signatures_tests.rs"]
mod tests;
