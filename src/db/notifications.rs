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

/// How long a claimed row stays leased before another cycle may reclaim it.
///
/// Sized well above the worst case for a single in-flight row
/// (5s Apprise timeout + the pool's 6s `connection_timeout` on the write-back),
/// so a transient pool-exhaustion episode has cleared before the reclaim, while
/// still redelivering within a useful window if the process dies mid-flight.
pub const CLAIM_LEASE_SECS: i64 = 300;

/// Claim up to `limit` pending outbox rows whose `next_attempt_at` is in the past.
///
/// This is a write, not a read. It stamps a lease into `next_attempt_at` and
/// consumes one attempt *before* the caller delivers, so a failure between
/// delivery and the outbox write-back cannot redeliver on the next 30s cycle.
/// The guarantee is bounded, not absolute: once [`CLAIM_LEASE_SECS`] elapses
/// the row is reclaimed, so a delivery abandoned for longer than the lease
/// *will* be redelivered. That is the deliberate trade — an abandoned row is
/// reclaimed rather than lost — and because the claim already consumed an
/// attempt, repeated reclaims still walk the row toward the dead-letter
/// threshold instead of redelivering forever.
///
/// The lease lives in `next_attempt_at` rather than a dedicated `'claimed'`
/// status for two reasons. `status` carries a `CHECK (status IN
/// ('pending','sent','dead','dropped'))` constraint that SQLite can only widen
/// by rebuilding the table; and a row moved out of `'pending'` would leave the
/// `idx_outbox_dedup_pending` unique partial index, letting the evaluator
/// re-enqueue the same `dedup_key` while delivery is still in flight.
///
/// `OutboxRow::attempt_count` is the count *before* this claim — the value the
/// dispatcher's backoff tier and dead-letter threshold are defined in terms of.
/// The write-back helpers below therefore do not increment it again.
pub fn outbox_claim_pending(
    conn: &rusqlite::Connection,
    limit: i64,
) -> rusqlite::Result<Vec<OutboxRow>> {
    // RETURNING requires SQLite >= 3.35 and makes select-and-lease a single
    // atomic statement.
    let mut stmt = conn.prepare(
        "UPDATE notifications_outbox
            SET attempt_count = attempt_count + 1,
                next_attempt_at =
                    strftime('%Y-%m-%dT%H:%M:%fZ','now', printf('+%d seconds', ?2))
          WHERE id IN (
                SELECT id FROM notifications_outbox
                 WHERE status = 'pending'
                   AND next_attempt_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 ORDER BY next_attempt_at ASC
                 LIMIT ?1)
      RETURNING id, dedup_key, rule_id, severity, hostname, title, body,
                apprise_urls_json, attempt_count - 1",
    )?;
    let mut rows = stmt
        .query_map(params![limit, CLAIM_LEASE_SECS], |row| {
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
    // RETURNING emits rows in update order, which SQLite does not define; the
    // subquery already picked the oldest `limit` rows, so sort by id to give
    // the caller a stable, roughly enqueue-ordered batch.
    rows.sort_by_key(|row| row.id);
    Ok(rows)
}

/// Mark a row as sent.
///
/// Does not touch `attempt_count`: [`outbox_claim_pending`] already consumed
/// the attempt this write-back is reporting on.
pub fn outbox_mark_sent(
    conn: &rusqlite::Connection,
    id: i64,
    status_code: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'sent',
             last_status_code = ?2
         WHERE id = ?1",
        params![id, status_code],
    )?;
    Ok(())
}

/// Mark a row as dead (exhausted retries).
///
/// Does not touch `attempt_count`: [`outbox_claim_pending`] already consumed
/// the attempt this write-back is reporting on.
pub fn outbox_mark_dead(
    conn: &rusqlite::Connection,
    id: i64,
    status_code: Option<i64>,
    error: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'dead',
             last_status_code = ?2,
             last_error = ?3
         WHERE id = ?1",
        params![id, status_code, error],
    )?;
    Ok(())
}

/// Mark a row as dropped (e.g. acked, deduplicated).
///
/// Does not touch `attempt_count`: [`outbox_claim_pending`] already consumed
/// the attempt this write-back is reporting on.
pub fn outbox_mark_dropped(
    conn: &rusqlite::Connection,
    id: i64,
    notes: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET status = 'dropped',
             last_error = ?2
         WHERE id = ?1",
        params![id, notes],
    )?;
    Ok(())
}

/// Set next_attempt_at for an exponential-backoff retry.
///
/// Overwrites the claim lease with the backoff deadline — which for the later
/// tiers is longer than the lease, not shorter — and does not
/// touch `attempt_count`: [`outbox_claim_pending`] already consumed the attempt
/// this write-back is reporting on.
pub fn outbox_schedule_retry(
    conn: &rusqlite::Connection,
    id: i64,
    next_attempt_at: &str,
    last_error: &str,
    status_code: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE notifications_outbox
         SET next_attempt_at = ?2,
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
