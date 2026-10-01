//! Additive attribution of forwarded logs to their claimed subject device.
//! Raw log/auth identities remain immutable. Counts cover retained rows;
//! first/last seen retain the observed time range until a bucket becomes empty.

use anyhow::Result;
use rusqlite::{Connection, Transaction, params};

use super::{DbPool, LogBatchEntry};

const MAX_BATCH_ROWS: usize = 1_000;

#[derive(Debug, Clone)]
pub struct ForwardedHostCount {
    pub original_hostname: String,
    pub hostname: String,
    pub log_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Copy)]
pub struct BackfillProgress {
    pub scanned: usize,
    pub attributed: usize,
    pub cursor: i64,
    pub high_water: i64,
    pub complete: bool,
}

pub(super) fn install_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE forwarded_log_hosts (
             log_id INTEGER PRIMARY KEY REFERENCES logs(id) ON DELETE CASCADE,
             hostname TEXT NOT NULL,
             original_hostname TEXT NOT NULL,
             timestamp TEXT NOT NULL,
             received_at TEXT NOT NULL
         );
         CREATE INDEX idx_forwarded_log_hosts_host_time
             ON forwarded_log_hosts(hostname,timestamp DESC,log_id DESC);
         CREATE INDEX idx_forwarded_log_hosts_host_id
             ON forwarded_log_hosts(hostname,log_id);
         CREATE INDEX idx_forwarded_log_hosts_original_host
             ON forwarded_log_hosts(original_hostname,hostname);
         CREATE TABLE forwarded_host_counts (
             original_hostname TEXT NOT NULL,
             hostname TEXT NOT NULL,
             log_count INTEGER NOT NULL CHECK(log_count > 0),
             first_seen TEXT NOT NULL,
             last_seen TEXT NOT NULL,
             PRIMARY KEY(original_hostname,hostname)
         );
         CREATE TABLE forwarded_host_names (
             hostname TEXT PRIMARY KEY
         );
         CREATE TABLE forwarded_principal_names (
             hostname TEXT PRIMARY KEY
         );
         CREATE TABLE forwarded_deleted_log_hosts (
             log_id INTEGER PRIMARY KEY,
             hostname TEXT NOT NULL,
             deleted_at INTEGER NOT NULL
         );
         CREATE INDEX idx_forwarded_deleted_hosts_host_id
             ON forwarded_deleted_log_hosts(hostname,log_id);
         CREATE INDEX idx_forwarded_deleted_hosts_expiry
             ON forwarded_deleted_log_hosts(deleted_at);
         CREATE TABLE forwarded_host_backfill (
             id INTEGER PRIMARY KEY CHECK(id=1),
             cursor INTEGER NOT NULL DEFAULT 0,
             high_water INTEGER NOT NULL,
             complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN(0,1))
         );
         INSERT INTO forwarded_host_backfill(id,high_water)
             SELECT 1,COALESCE(MAX(id),0) FROM logs;
         -- This bounded scan also repairs legacy host counters inflated by
         -- historical retention. Changes outside its unscanned snapshot must
         -- be included immediately; changes inside it are seen by the scan.
         CREATE TABLE forwarded_raw_host_counts (
             hostname TEXT PRIMARY KEY,
             log_count INTEGER NOT NULL CHECK(log_count>0)
         );
         CREATE TRIGGER forwarded_backfill_raw_insert AFTER INSERT ON logs
         WHEN EXISTS(SELECT 1 FROM forwarded_host_backfill
             WHERE complete=0 AND (new.id>high_water OR new.id<=cursor)) BEGIN
             INSERT INTO forwarded_raw_host_counts(hostname,log_count) VALUES(new.hostname,1)
             ON CONFLICT(hostname) DO UPDATE SET log_count=log_count+1;
         END;
         CREATE TRIGGER forwarded_backfill_raw_delete BEFORE DELETE ON logs
         WHEN EXISTS(SELECT 1 FROM forwarded_host_backfill
             WHERE complete=0 AND (old.id>high_water OR old.id<=cursor)) BEGIN
             DELETE FROM forwarded_raw_host_counts WHERE hostname=old.hostname AND log_count=1;
             UPDATE forwarded_raw_host_counts SET log_count=log_count-1 WHERE hostname=old.hostname;
         END;
         -- Pooled Cortex connections retain historical foreign_keys=OFF.
         -- This trigger supplies deletion cleanup without changing that policy.
         -- Capture before FK cascade or AFTER DELETE cleanup removes the live
         -- projection. The original stream lineage still retains auth identity.
         CREATE TRIGGER logs_forwarded_host_lineage BEFORE DELETE ON logs BEGIN
             DELETE FROM forwarded_deleted_log_hosts WHERE deleted_at < unixepoch()-900;
             INSERT OR REPLACE INTO forwarded_deleted_log_hosts(log_id,hostname,deleted_at)
                 SELECT log_id,hostname,unixepoch() FROM forwarded_log_hosts WHERE log_id=old.id;
         END;
         CREATE TRIGGER logs_delete_forwarded_host AFTER DELETE ON logs BEGIN
             DELETE FROM forwarded_log_hosts WHERE log_id=old.id;
         END;
         CREATE TRIGGER forwarded_log_hosts_insert AFTER INSERT ON forwarded_log_hosts BEGIN
             INSERT OR IGNORE INTO forwarded_host_names(hostname) VALUES(new.hostname);
             INSERT OR IGNORE INTO forwarded_principal_names(hostname) VALUES(new.original_hostname);
             INSERT INTO forwarded_host_counts(original_hostname,hostname,log_count,first_seen,last_seen)
             VALUES(new.original_hostname,new.hostname,1,new.received_at,new.received_at)
             ON CONFLICT(original_hostname,hostname) DO UPDATE SET
                 log_count=log_count+1,
                 first_seen=MIN(first_seen,excluded.first_seen),
                 last_seen=MAX(last_seen,excluded.last_seen);
         END;
         CREATE TRIGGER forwarded_log_hosts_delete AFTER DELETE ON forwarded_log_hosts BEGIN
             DELETE FROM forwarded_host_counts WHERE original_hostname=old.original_hostname
                 AND hostname=old.hostname AND log_count=1;
             UPDATE forwarded_host_counts SET log_count=log_count-1
                 WHERE original_hostname=old.original_hostname AND hostname=old.hostname;
         END;",
    )?;
    install_lineage_expiry_trigger(conn)?;
    Ok(())
}

pub(super) fn install_lineage_expiry_trigger(conn: &Connection) -> Result<()> {
    let dependencies_exist: bool = conn.query_row(
        "SELECT COUNT(*)=2 FROM sqlite_master WHERE type='table'
         AND name IN('stream_deleted_log_lineage','forwarded_deleted_log_hosts')",
        [],
        |row| row.get(0),
    )?;
    if dependencies_exist {
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS forwarded_lineage_expiry
             AFTER DELETE ON stream_deleted_log_lineage BEGIN
                 DELETE FROM forwarded_deleted_log_hosts WHERE log_id=old.id;
             END;",
        )?;
    }
    Ok(())
}

pub(crate) fn insert_in_tx(tx: &Transaction<'_>, id: i64, entry: &LogBatchEntry) -> Result<bool> {
    insert_subject(
        tx,
        id,
        &entry.hostname,
        &entry.source_ip,
        entry.metadata_json.as_deref(),
        &entry.timestamp,
        None,
    )
}

fn insert_subject(
    conn: &Connection,
    id: i64,
    original_hostname: &str,
    source_ip: &str,
    metadata: Option<&str>,
    timestamp: &str,
    received_at: Option<&str>,
) -> Result<bool> {
    let Some(hostname) =
        crate::forwarded_host::subject_hostname(original_hostname, source_ip, metadata)
    else {
        return Ok(false);
    };
    // Future ingestion obtains the server receipt from the inserted row in the
    // same transaction; historical backfill already read that immutable value.
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO forwarded_log_hosts(log_id,hostname,original_hostname,timestamp,received_at)
         SELECT id,?2,?3,?4,COALESCE(?5,received_at) FROM logs WHERE id=?1",
        params![id, hostname, original_hostname, timestamp, received_at],
    )?;
    Ok(inserted > 0)
}

pub fn list_forwarded_host_counts(conn: &Connection) -> Result<Vec<ForwardedHostCount>> {
    Ok(conn
        .prepare(
            "SELECT original_hostname,hostname,log_count,first_seen,last_seen
         FROM forwarded_host_counts ORDER BY original_hostname,hostname",
        )?
        .query_map([], |row| {
            Ok(ForwardedHostCount {
                original_hostname: row.get(0)?,
                hostname: row.get(1)?,
                log_count: row.get(2)?,
                first_seen: row.get(3)?,
                last_seen: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Validated forwarding evidence establishes a principal namespace even after
/// its last attributed row expires. Similar-looking device names do not.
pub(super) fn list_forwarding_principals(
    conn: &Connection,
) -> Result<std::collections::HashSet<String>> {
    Ok(conn
        .prepare("SELECT hostname FROM forwarded_principal_names")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Resume one bounded historical page. Cursor and inserts commit atomically;
/// cancellation between batches or process restart cannot skip an attribution.
pub fn backfill_batch(pool: &DbPool, limit: usize) -> Result<BackfillProgress> {
    let mut conn = super::write_conn(pool)?;
    let tx = conn.transaction()?;
    let (cursor, high_water, complete): (i64, i64, bool) = tx.query_row(
        "SELECT cursor,high_water,complete FROM forwarded_host_backfill WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if complete {
        return Ok(BackfillProgress {
            scanned: 0,
            attributed: 0,
            cursor,
            high_water,
            complete,
        });
    }
    // Scan a fixed number of primary-key rows per transaction, including
    // unrelated lanes, so a sparse forwarding history cannot cause a full
    // corpus scan in one batch. Materialize only bounded forwarding metadata.
    let rows = tx.prepare(
        "SELECT id,hostname,source_ip,
             CASE WHEN (source_ip LIKE 'agent-ai-transcript://%' OR source_ip LIKE 'agent-syslog://%')
                 AND length(CAST(metadata_json AS BLOB))<=65536 THEN metadata_json ELSE NULL END,
             timestamp,received_at FROM logs
         WHERE id>?1 AND id<=?2
         ORDER BY id LIMIT ?3",
    )?.query_map(params![cursor,high_water,limit.clamp(1,MAX_BATCH_ROWS) as i64], |row| Ok((
        row.get::<_,i64>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?,
        row.get::<_,Option<String>>(3)?, row.get::<_,String>(4)?, row.get::<_,String>(5)?,
    )))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let scanned = rows.len();
    let mut raw_counts = std::collections::HashMap::<&str, i64>::new();
    for (_, original, ..) in &rows {
        *raw_counts.entry(original).or_default() += 1;
    }
    let mut raw_statement = tx.prepare_cached(
        "INSERT INTO forwarded_raw_host_counts(hostname,log_count) VALUES(?1,?2)
         ON CONFLICT(hostname) DO UPDATE SET log_count=log_count+excluded.log_count",
    )?;
    for (hostname, count) in raw_counts {
        raw_statement.execute(params![hostname, count])?;
    }
    drop(raw_statement);
    let mut attributed = 0;
    for (id, original, source, metadata, timestamp, receipt) in &rows {
        attributed += usize::from(insert_subject(
            &tx,
            *id,
            original,
            source,
            metadata.as_deref(),
            timestamp,
            Some(receipt),
        )?);
    }
    let complete = scanned < limit.clamp(1, MAX_BATCH_ROWS);
    let cursor = if complete {
        high_water
    } else {
        rows.last().expect("nonempty page").0
    };
    if complete {
        reconcile_raw_host_counts(&tx)?;
    }
    tx.execute(
        "UPDATE forwarded_host_backfill SET cursor=?1,complete=?2 WHERE id=1",
        params![cursor, complete],
    )?;
    tx.commit()?;
    Ok(BackfillProgress {
        scanned,
        attributed,
        cursor,
        high_water,
        complete,
    })
}

/// Only the small host registry is traversed here. Receipt endpoints are
/// indexed equality probes, never COUNT/MIN/MAX scans over the log corpus.
fn reconcile_raw_host_counts(conn: &Connection) -> Result<()> {
    let rows = conn.prepare(
        "SELECT names.hostname,COALESCE(counts.log_count,0)
         FROM (SELECT hostname FROM hosts UNION SELECT hostname FROM forwarded_raw_host_counts) names
         LEFT JOIN forwarded_raw_host_counts counts ON counts.hostname=names.hostname",
    )?.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (hostname, count) in rows {
        if count == 0 {
            conn.execute("DELETE FROM hosts WHERE hostname=?1", [&hostname])?;
            continue;
        }
        let first: String = conn.query_row(
            "SELECT received_at FROM logs INDEXED BY idx_logs_hostname_received_at
             WHERE hostname=?1 ORDER BY received_at ASC LIMIT 1",
            [&hostname],
            |row| row.get(0),
        )?;
        let last: String = conn.query_row(
            "SELECT received_at FROM logs INDEXED BY idx_logs_hostname_received_at
             WHERE hostname=?1 ORDER BY received_at DESC LIMIT 1",
            [&hostname],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO hosts(hostname,first_seen,last_seen,log_count) VALUES(?1,?2,?3,?4)
             ON CONFLICT(hostname) DO UPDATE SET first_seen=excluded.first_seen,
                 last_seen=excluded.last_seen,log_count=excluded.log_count",
            params![hostname, first, last, count],
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "host_attribution_tests.rs"]
mod tests;
