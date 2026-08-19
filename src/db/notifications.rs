//! Database operations for the notifications subsystem.
//!
//! All functions take a `&rusqlite::Connection` so they can be called either
//! from a plain connection or from inside a `rusqlite::Transaction`
//! (Transaction derefs to Connection).
//!
//! Call from inside `tokio::task::spawn_blocking`, never from async context.

use rusqlite::params;

// ---------------------------------------------------------------------------
// Public types (cross-bead coupling export)

/// Parameters for inserting a row into `notifications_outbox`.
pub struct OutboxInsertParams {
    pub dedup_key: String,
    pub rule_id: String,
    pub severity: String,
    pub hostname: String,
    pub title: String,
    pub body: String,
    pub apprise_urls_json: String,
    /// ISO8601 datetime for next delivery attempt.
    pub next_attempt_at: String,
}

/// A row fetched from `notifications_outbox`.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub dedup_key: String,
    pub rule_id: String,
    pub severity: String,
    pub hostname: String,
    pub title: String,
    pub body: String,
    pub apprise_urls_json: String,
    pub attempt_count: i64,
}

/// A row from `notification_firings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FiringRow {
    pub id: i64,
    pub outbox_id: i64,
    pub rule_id: String,
    pub hostname: String,
    pub fired_at: String,
    pub status_code: Option<i64>,
}

// ---------------------------------------------------------------------------
// Outbox operations

/// Insert a row into `notifications_outbox`.
///
/// Idempotent on `(dedup_key, status='pending')` via the partial unique index
/// `idx_outbox_dedup_pending` (migration 12). Uses `INSERT OR IGNORE` to
/// avoid a TOCTOU race between the SELECT COUNT(*) guard and the INSERT.
pub fn outbox_insert(
    conn: &rusqlite::Connection,
    params: &OutboxInsertParams,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO notifications_outbox
             (dedup_key, rule_id, severity, hostname, title, body, apprise_urls_json, next_attempt_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            params.dedup_key,
            params.rule_id,
            params.severity,
            params.hostname,
            params.title,
            params.body,
            params.apprise_urls_json,
            params.next_attempt_at,
        ],
    )?;
    Ok(())
}

/// Claim up to `limit` pending outbox rows whose `next_attempt_at` is in the past.
pub fn outbox_claim_pending(
    conn: &rusqlite::Connection,
    limit: i64,
) -> rusqlite::Result<Vec<OutboxRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, dedup_key, rule_id, severity, hostname, title, body,
                apprise_urls_json, attempt_count
         FROM notifications_outbox
         WHERE status = 'pending'
           AND next_attempt_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')
         ORDER BY next_attempt_at ASC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(OutboxRow {
                id: row.get(0)?,
                dedup_key: row.get(1)?,
                rule_id: row.get(2)?,
                severity: row.get(3)?,
                hostname: row.get(4)?,
                title: row.get(5)?,
                body: row.get(6)?,
                apprise_urls_json: row.get(7)?,
                attempt_count: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Mark a row as sent; increment attempt_count.
pub fn outbox_mark_sent(
    conn: &rusqlite::Connection,
    id: i64,
    status_code: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'sent',
             attempt_count = attempt_count + 1,
             last_status_code = ?2
         WHERE id = ?1",
        params![id, status_code],
    )?;
    Ok(())
}

/// Mark a row as dead (exhausted retries).
pub fn outbox_mark_dead(
    conn: &rusqlite::Connection,
    id: i64,
    status_code: Option<i64>,
    error: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'dead',
             attempt_count = attempt_count + 1,
             last_status_code = ?2,
             last_error = ?3
         WHERE id = ?1",
        params![id, status_code, error],
    )?;
    Ok(())
}

/// Mark a row as dropped (e.g. acked, deduplicated).
pub fn outbox_mark_dropped(
    conn: &rusqlite::Connection,
    id: i64,
    notes: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'dropped',
             attempt_count = attempt_count + 1,
             last_error = ?2
         WHERE id = ?1",
        params![id, notes],
    )?;
    Ok(())
}

/// Set next_attempt_at for exponential backoff retry; increment attempt_count.
pub fn outbox_schedule_retry(
    conn: &rusqlite::Connection,
    id: i64,
    next_attempt_at: &str,
    last_error: &str,
    status_code: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET attempt_count = attempt_count + 1,
             next_attempt_at = ?2,
             last_error = ?3,
             last_status_code = ?4
         WHERE id = ?1",
        params![id, next_attempt_at, last_error, status_code],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Firings

/// Parameters for inserting a row into `notification_firings`.
pub struct FiringInsertParams<'a> {
    pub outbox_id: i64,
    pub rule_id: &'a str,
    pub severity: &'a str,
    pub hostname: &'a str,
    pub status_code: Option<i64>,
    pub notes: Option<&'a str>,
    /// Mirrors the outbox row's dedup_key so that dedup checks are scoped to
    /// a specific error signature rather than all firings for (rule_id, hostname).
    pub dedup_key: &'a str,
}

/// Insert a row into `notification_firings`.
pub fn firings_insert(
    conn: &rusqlite::Connection,
    p: FiringInsertParams<'_>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO notification_firings
             (outbox_id, rule_id, severity, hostname, status_code, notes, dedup_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            p.outbox_id,
            p.rule_id,
            p.severity,
            p.hostname,
            p.status_code,
            p.notes,
            p.dedup_key
        ],
    )?;
    Ok(())
}

/// Check if there is a recent firing for the given rule+hostname+dedup_key
/// within the dedup window (seconds). Returns true if a firing already exists
/// (suppress).
///
/// The `dedup_key` parameter is essential for rules that share a `rule_id`
/// (e.g. `unaddressed_error_signature` fires once per distinct error hash).
/// Without it, the first firing would suppress all subsequent ones regardless
/// of which signature they belong to.
pub fn firings_recent_dedup_check(
    conn: &rusqlite::Connection,
    rule_id: &str,
    hostname: &str,
    dedup_key: &str,
    dedup_window_secs: u64,
) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM notification_firings
         WHERE rule_id = ?1
           AND hostname = ?2
           AND dedup_key = ?3
           AND fired_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', printf('-%d seconds', ?4))",
        params![rule_id, hostname, dedup_key, dedup_window_secs as i64],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Check whether a firing has ever been recorded for the exact
/// rule+hostname+dedup_key tuple.
///
/// This is used by once-per-outage rules whose dedup key includes the
/// observation timestamp that identifies the outage. A new observation gets
/// a new key; an unchanged outage remains suppressed regardless of age.
pub fn firings_any_dedup_check(
    conn: &rusqlite::Connection,
    rule_id: &str,
    hostname: &str,
    dedup_key: &str,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM notification_firings
             WHERE rule_id = ?1
               AND hostname = ?2
               AND dedup_key = ?3
         )",
        params![rule_id, hostname, dedup_key],
        |row| row.get(0),
    )
}

/// Fetch recent firings for a given rule_id (optional) since a given time.
pub fn firings_recent(
    conn: &rusqlite::Connection,
    limit: i64,
    rule_id: Option<&str>,
    since: Option<&str>,
) -> rusqlite::Result<Vec<FiringRow>> {
    let clamped_limit = limit.clamp(1, 500);
    let mut stmt = conn.prepare(
        "SELECT id, outbox_id, rule_id, hostname, fired_at, status_code
         FROM notification_firings
         WHERE (?1 IS NULL OR rule_id = ?1)
           AND (?2 IS NULL OR fired_at >= ?2)
         ORDER BY fired_at DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![rule_id, since, clamped_limit], |row| {
            Ok(FiringRow {
                id: row.get(0)?,
                outbox_id: row.get(1)?,
                rule_id: row.get(2)?,
                hostname: row.get(3)?,
                fired_at: row.get(4)?,
                status_code: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Backoff helper

/// Compute `next_attempt_at` as an ISO8601 string given `attempt_count`.
///
/// Backoff schedule (capped at 30 minutes):
///   attempt 0 → now+1s
///   attempt 1 → now+5s
///   attempt 2 → now+30s
///   attempt 3 → now+5min
///   attempt 4+ → now+30min
pub fn backoff_next_attempt_at(attempt_count: u8) -> String {
    let delay_secs: u64 = match attempt_count {
        0 => 1,
        1 => 5,
        2 => 30,
        3 => 300,
        _ => 1800,
    };
    let next = chrono::Utc::now() + chrono::Duration::seconds(delay_secs as i64);
    next.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
