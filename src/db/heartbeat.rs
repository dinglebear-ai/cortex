use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use tracing::warn;

use super::pool::DbPool;

const DISK_PRESSURE_SQL_FILTER: &str = "
    used_percent IS NOT NULL
    AND COALESCE(filesystem, '') NOT IN (
        'autofs',
        'binfmt_misc',
        'bpf',
        'cgroup',
        'cgroup2',
        'configfs',
        'debugfs',
        'devpts',
        'devtmpfs',
        'efivarfs',
        'fuse.snapfuse',
        'fusectl',
        'hugetlbfs',
        'iso9660',
        'mqueue',
        'nsfs',
        'overlay',
        'proc',
        'pstore',
        'ramfs',
        'rootfs',
        'securityfs',
        'squashfs',
        'sysfs',
        'tmpfs',
        'tracefs'
    )
    AND COALESCE(mountpoint, '') NOT IN ('', '/init')
    AND COALESCE(mountpoint, '') NOT LIKE '/snap/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/mnt/wsl/docker-desktop/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/mnt/wslg/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/usr/lib/modules/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/usr/lib/wsl/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/run/%'
    AND COALESCE(mountpoint, '') NOT LIKE '/var/run/%'
";

#[derive(Debug, Clone)]
pub enum HeartbeatHostLookup {
    HostId(String),
    Hostname(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatHostState {
    pub host_id: String,
    pub hostname: String,
    pub total_samples: usize,
    pub truncated: bool,
    pub flags: HeartbeatStateFlags,
    pub latest: Option<HeartbeatSampleState>,
    pub samples: Vec<HeartbeatSampleState>,
}

/// Server-computed derived signals for a heartbeat sample.
/// These are the canonical source of truth for fleet views and correlation;
/// agent-supplied local flags are informational only.
///
/// All flag computation is centralised in `app::heartbeat_flags::derive_flags`
/// so that MCP, REST, and CLI adapters share identical thresholds and logic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatStateFlags {
    // ── Availability ─────────────────────────────────────────────────────────
    pub collector_partial: bool,
    pub heartbeat_late: bool,
    pub clock_skew: bool,
    // ── Resource pressure ────────────────────────────────────────────────────
    pub cpu_pressure: bool,
    pub memory_pressure: bool,
    pub swap_pressure: bool,
    pub disk_capacity_pressure: bool,
    pub network_error_pressure: bool,
    pub container_unhealthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatSampleState {
    pub heartbeat_id: i64,
    pub host_id: String,
    pub hostname: String,
    pub sampled_at: String,
    pub received_at: String,
    pub source_ip: String,
    pub boot_id: String,
    pub sequence: i64,
    pub uptime_secs: i64,
    pub collection_ms: i64,
    pub partial: bool,
    pub agent_version: String,
    pub os: String,
    pub kernel: Option<String>,
    pub architecture: String,
    pub metadata: Option<Value>,
    pub cpu: Option<Value>,
    pub memory: Option<Value>,
    pub disks: Vec<Value>,
    pub network: Vec<Value>,
    pub processes: Option<Value>,
    pub containers: Vec<Value>,
}

// ── Fleet-state types ─────────────────────────────────────────────────────

/// One row from `host_heartbeats_latest` — the fleet-state cache table.
/// Holds only the fields needed to compute derived flags without joining
/// the main `host_heartbeats` table.
#[derive(Debug, Clone)]
pub struct HeartbeatLatestEntry {
    pub host_id: String,
    pub heartbeat_id: i64,
    pub hostname: String,
    pub sampled_at: String,
    pub received_at: String,
    pub partial: bool,
    pub metadata_json: Option<String>,
}

/// Aggregated metric values for a single heartbeat_id.
/// Used by `app::heartbeat_flags::derive_flags` to compute pressure signals.
#[derive(Debug, Clone, Default)]
pub struct HeartbeatMetricSnapshot {
    pub cpu_usage_percent: Option<f64>,
    pub mem_used_percent: Option<f64>,
    pub swap_total_bytes: Option<i64>,
    pub swap_used_bytes: Option<i64>,
    pub max_disk_used_percent: Option<f64>,
    pub total_network_errors: Option<i64>,
    pub container_unhealthy_count: Option<i64>,
}

/// A host whose latest accepted heartbeat has gone stale.
#[derive(Debug, Clone)]
pub struct StaleHeartbeatHost {
    pub host_id: String,
    pub hostname: String,
    pub received_at: String,
    pub age_secs: u64,
}

/// Hosts whose newest heartbeat is older than `threshold_secs` but younger
/// than `forget_secs` — the heartbeat_silence notification rule's input.
/// O(hosts) scan of the small `host_heartbeats_latest` cache; the forget
/// bound keeps decommissioned hosts from staying alert-eligible forever.
pub fn stale_heartbeat_hosts(
    conn: &rusqlite::Connection,
    threshold_secs: u64,
    forget_secs: u64,
) -> Result<Vec<StaleHeartbeatHost>> {
    let age_expr = "CAST(strftime('%s','now') AS INTEGER) - \
                    CAST(strftime('%s', received_at) AS INTEGER)";
    // Thresholds are trusted u64 config values, inlined because SQLite does
    // not allow SELECT aliases in WHERE.
    let sql = format!(
        "SELECT host_id, hostname, received_at, {age_expr} AS age_secs
         FROM host_heartbeats_latest
         WHERE ({age_expr}) > {threshold_secs} AND ({age_expr}) < {forget_secs}
         ORDER BY hostname ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(StaleHeartbeatHost {
                host_id: row.get(0)?,
                hostname: row.get(1)?,
                received_at: row.get(2)?,
                age_secs: row.get::<_, i64>(3)?.max(0) as u64,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Return all entries from `host_heartbeats_latest`, ordered by hostname.
///
/// This is an O(hosts) full scan of a small cache table — it deliberately
/// avoids scanning `host_heartbeats` (which may contain millions of rows).
/// EXPLAIN QUERY PLAN should show `SCAN host_heartbeats_latest`, never
/// `SCAN host_heartbeats`.
pub fn heartbeat_latest_all(pool: &DbPool) -> Result<Vec<HeartbeatLatestEntry>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT host_id, heartbeat_id, hostname, sampled_at, received_at,
                partial, metadata_json
         FROM host_heartbeats_latest
         ORDER BY hostname ASC",
    )?;
    let entries = stmt
        .query_map([], |row| {
            Ok(HeartbeatLatestEntry {
                host_id: row.get(0)?,
                heartbeat_id: row.get(1)?,
                hostname: row.get(2)?,
                sampled_at: row.get(3)?,
                received_at: row.get(4)?,
                partial: row.get::<_, i64>(5)? != 0,
                metadata_json: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries)
}

/// Fetch aggregated metric values for one heartbeat by `heartbeat_id`.
///
/// All five queries target indexed `heartbeat_id` columns. Each returns at
/// most one row (or one aggregate). Kept for targeted tests; production code
/// uses `heartbeat_metric_snapshot_batch`.
#[cfg(test)]
pub fn heartbeat_metric_snapshot(
    pool: &DbPool,
    heartbeat_id: i64,
) -> Result<HeartbeatMetricSnapshot> {
    let conn = pool.get()?;

    let cpu: Option<(Option<f64>, Option<f64>)> = conn
        .query_row(
            "SELECT usage_percent, load1
             FROM heartbeat_cpu WHERE heartbeat_id = ?1",
            [heartbeat_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let mem: Option<(Option<f64>, Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT used_percent, swap_total_bytes, swap_used_bytes
             FROM heartbeat_memory WHERE heartbeat_id = ?1",
            [heartbeat_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let max_disk: Option<f64> = conn
        .query_row(
            &format!(
                "SELECT MAX(used_percent) FROM heartbeat_disks \
                 WHERE heartbeat_id = ?1 AND {DISK_PRESSURE_SQL_FILTER}"
            ),
            [heartbeat_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let net_errors: Option<i64> = conn
        .query_row(
            "SELECT SUM(COALESCE(rx_errors, 0) + COALESCE(tx_errors, 0))
             FROM heartbeat_network WHERE heartbeat_id = ?1",
            [heartbeat_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let container_unhealthy: Option<i64> = conn
        .query_row(
            "SELECT MAX(COALESCE(unhealthy, 0))
             FROM heartbeat_containers WHERE heartbeat_id = ?1",
            [heartbeat_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    Ok(HeartbeatMetricSnapshot {
        cpu_usage_percent: cpu.as_ref().and_then(|(u, _)| *u),
        mem_used_percent: mem.as_ref().and_then(|(u, _, _)| *u),
        swap_total_bytes: mem.as_ref().and_then(|(_, t, _)| *t),
        swap_used_bytes: mem.and_then(|(_, _, u)| u),
        max_disk_used_percent: max_disk,
        total_network_errors: net_errors,
        container_unhealthy_count: container_unhealthy,
    })
}

/// Fetch aggregated metric values for multiple heartbeat IDs in one pass.
///
/// Returns a map from heartbeat_id → snapshot. IDs with no data are absent
/// from the map (callers should use `unwrap_or_default()`).
pub fn heartbeat_metric_snapshot_batch(
    pool: &DbPool,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, HeartbeatMetricSnapshot>> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let conn = pool.get()?;
    let placeholders = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let mut map: std::collections::HashMap<i64, HeartbeatMetricSnapshot> = ids
        .iter()
        .map(|&id| (id, HeartbeatMetricSnapshot::default()))
        .collect();

    let cpu_sql = format!(
        "SELECT heartbeat_id, usage_percent FROM heartbeat_cpu WHERE heartbeat_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&cpu_sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?))
    })?;
    for row in rows.flatten() {
        map.entry(row.0).or_default().cpu_usage_percent = row.1;
    }

    let mem_sql = format!(
        "SELECT heartbeat_id, used_percent, swap_total_bytes, swap_used_bytes \
         FROM heartbeat_memory WHERE heartbeat_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&mem_sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<f64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    for row in rows.flatten() {
        let e = map.entry(row.0).or_default();
        e.mem_used_percent = row.1;
        e.swap_total_bytes = row.2;
        e.swap_used_bytes = row.3;
    }

    let disk_sql = format!(
        "SELECT heartbeat_id, MAX(used_percent) FROM heartbeat_disks \
         WHERE heartbeat_id IN ({placeholders}) AND {DISK_PRESSURE_SQL_FILTER} \
         GROUP BY heartbeat_id"
    );
    let mut stmt = conn.prepare(&disk_sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?))
    })?;
    for row in rows.flatten() {
        map.entry(row.0).or_default().max_disk_used_percent = row.1;
    }

    let net_sql = format!(
        "SELECT heartbeat_id, SUM(COALESCE(rx_errors,0)+COALESCE(tx_errors,0)) \
         FROM heartbeat_network WHERE heartbeat_id IN ({placeholders}) GROUP BY heartbeat_id"
    );
    let mut stmt = conn.prepare(&net_sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    for row in rows.flatten() {
        map.entry(row.0).or_default().total_network_errors = row.1;
    }

    let ctr_sql = format!(
        "SELECT heartbeat_id, MAX(COALESCE(unhealthy,0)) FROM heartbeat_containers \
         WHERE heartbeat_id IN ({placeholders}) GROUP BY heartbeat_id"
    );
    let mut stmt = conn.prepare(&ctr_sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    for row in rows.flatten() {
        map.entry(row.0).or_default().container_unhealthy_count = row.1;
    }

    Ok(map)
}

/// Return all heartbeat rows for `host_id` within `[from, to]` (inclusive),
/// with lightweight summaries for `correlate_state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatWindowSummary {
    pub host_id: String,
    pub hostname: String,
    pub samples: usize,
    pub partial_samples: usize,
    pub max_cpu_usage_percent: Option<f64>,
    pub min_mem_available_bytes: Option<i64>,
    pub pressure_flags: Vec<String>,
}

/// Build per-host heartbeat summaries for a time window.
///
/// When `host_id` is `Some`, only that host is included (single-host
/// correlate_state). When `None`, all hosts with heartbeats in the window are
/// included. The query uses the `idx_host_heartbeats_received` index on
/// `received_at` as the primary range predicate to avoid broad table scans.
pub fn heartbeat_window_summaries(
    pool: &DbPool,
    from: &str,
    to: &str,
    host_id: Option<&str>,
) -> Result<Vec<HeartbeatWindowSummary>> {
    let conn = pool.get()?;

    type WindowRow = (String, String, i64, i64, Option<f64>, Option<i64>);
    let row_from = |row: &rusqlite::Row<'_>| -> rusqlite::Result<WindowRow> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    };

    // `max_cpu`/`min_mem` come from the most-recent heartbeat in the window
    // for each host. The latest id must be resolved with a scalar subquery
    // (`SELECT MAX(h2.id) ...`) rather than referencing `MAX(h.id)` inside a
    // correlated subquery — SQLite rejects the latter as "misuse of aggregate"
    // under GROUP BY.
    let rows: Vec<WindowRow> = if let Some(hid) = host_id {
        let mut stmt = conn.prepare(
            "SELECT h.host_id, h.hostname,
                    COUNT(*) AS samples,
                    SUM(h.partial) AS partial_samples,
                    (SELECT c.usage_percent FROM heartbeat_cpu c
                     WHERE c.heartbeat_id = (
                         SELECT MAX(h2.id) FROM host_heartbeats h2
                         WHERE h2.host_id = h.host_id
                           AND h2.received_at >= ?2
                           AND h2.received_at <= ?3
                     )) AS max_cpu,
                    (SELECT m.available_bytes FROM heartbeat_memory m
                     WHERE m.heartbeat_id = (
                         SELECT MAX(h2.id) FROM host_heartbeats h2
                         WHERE h2.host_id = h.host_id
                           AND h2.received_at >= ?2
                           AND h2.received_at <= ?3
                     )) AS min_mem
             FROM host_heartbeats h
             WHERE h.host_id = ?1
               AND h.received_at >= ?2
               AND h.received_at <= ?3
             GROUP BY h.host_id, h.hostname",
        )?;

        stmt.query_map(params![hid, from, to], row_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT h.host_id, h.hostname,
                    COUNT(*) AS samples,
                    SUM(h.partial) AS partial_samples,
                    (SELECT c.usage_percent FROM heartbeat_cpu c
                     WHERE c.heartbeat_id = (
                         SELECT MAX(h2.id) FROM host_heartbeats h2
                         WHERE h2.host_id = h.host_id
                           AND h2.received_at >= ?1
                           AND h2.received_at <= ?2
                     )) AS max_cpu,
                    (SELECT m.available_bytes FROM heartbeat_memory m
                     WHERE m.heartbeat_id = (
                         SELECT MAX(h2.id) FROM host_heartbeats h2
                         WHERE h2.host_id = h.host_id
                           AND h2.received_at >= ?1
                           AND h2.received_at <= ?2
                     )) AS min_mem
             FROM host_heartbeats h
             WHERE h.received_at >= ?1
               AND h.received_at <= ?2
             GROUP BY h.host_id, h.hostname
             ORDER BY h.hostname ASC",
        )?;

        stmt.query_map(params![from, to], row_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(rows
        .into_iter()
        .map(
            |(host_id, hostname, samples, partial_samples, max_cpu, min_mem)| {
                HeartbeatWindowSummary {
                    host_id,
                    hostname,
                    samples: samples as usize,
                    partial_samples: partial_samples as usize,
                    max_cpu_usage_percent: max_cpu,
                    min_mem_available_bytes: min_mem,
                    pressure_flags: Vec::new(), // filled by service layer
                }
            },
        )
        .collect())
}

// ── Private row types ─────────────────────────────────────────────────────

#[derive(Debug)]
struct HeartbeatRow {
    id: i64,
    host_id: String,
    hostname: String,
    source_ip: String,
    sampled_at: String,
    received_at: String,
    boot_id: String,
    uptime_secs: i64,
    sequence: i64,
    collection_ms: i64,
    partial: bool,
    agent_version: String,
    os: String,
    kernel: Option<String>,
    architecture: String,
    metadata_json: Option<String>,
}

pub fn heartbeat_host_state(
    pool: &DbPool,
    lookup: HeartbeatHostLookup,
    since: Option<&str>,
    limit: usize,
) -> Result<HeartbeatHostState> {
    let conn = pool.get()?;
    let host_id = match lookup {
        HeartbeatHostLookup::HostId(host_id) => host_id,
        HeartbeatHostLookup::Hostname(hostname) => resolve_unique_hostname(&conn, &hostname)?,
    };

    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let rows = if let Some(since) = since {
        let mut stmt = conn.prepare(
            "SELECT id, host_id, hostname, source_ip, sampled_at, received_at, boot_id,
                    uptime_secs, sequence, collection_ms, partial, agent_version,
                    os, kernel, architecture, metadata_json
             FROM host_heartbeats
             WHERE host_id = ?1 AND sampled_at >= ?2
             ORDER BY sampled_at DESC, id DESC
             LIMIT ?3",
        )?;

        stmt.query_map(
            params![host_id, since, fetch_limit as i64],
            map_heartbeat_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, host_id, hostname, source_ip, sampled_at, received_at, boot_id,
                    uptime_secs, sequence, collection_ms, partial, agent_version,
                    os, kernel, architecture, metadata_json
             FROM host_heartbeats
             WHERE host_id = ?1
             ORDER BY sampled_at DESC, id DESC
             LIMIT ?2",
        )?;

        stmt.query_map(params![host_id, fetch_limit as i64], map_heartbeat_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    if rows.is_empty() {
        return Err(anyhow!("not_found"));
    }

    let truncated = rows.len() > limit;
    let mut samples = Vec::with_capacity(limit.min(rows.len()));
    for row in rows.into_iter().take(limit) {
        samples.push(sample_from_row(&conn, row)?);
    }
    let latest = samples.first().cloned();
    let host_id = samples[0].host_id.clone();
    let hostname = samples[0].hostname.clone();
    let flags = latest.as_ref().map(heartbeat_flags).unwrap_or_default();

    Ok(HeartbeatHostState {
        host_id,
        hostname,
        total_samples: samples.len(),
        truncated,
        flags,
        latest,
        samples,
    })
}

/// Derive `HeartbeatStateFlags` from a fully-loaded sample state.
///
/// For the single-host path (`host_state`) where metric data is already
/// embedded in `HeartbeatSampleState` as JSON Values, this function computes
/// all derived signals without additional DB queries. For the fleet path
/// (`fleet_state`), use `app::heartbeat_flags::derive_flags` instead.
pub(crate) fn heartbeat_flags(sample: &HeartbeatSampleState) -> HeartbeatStateFlags {
    use crate::app::heartbeat_flags;
    heartbeat_flags::from_sample(sample)
}

pub(super) fn resolve_unique_hostname(
    conn: &rusqlite::Connection,
    hostname: &str,
) -> Result<String> {
    // Current device identity comes from the small latest-heartbeat table. Use
    // the same conservative aliases as log-source listings, but never merge
    // two stable device IDs merely because their reported names look alike.
    let current = {
        let mut stmt = conn.prepare("SELECT hostname, host_id FROM host_heartbeats_latest")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut names: Vec<String> = current.iter().map(|(name, _)| name.clone()).collect();
    names.push(hostname.to_owned());
    let aliases = super::queries::canonical_host_keys(&names);
    let requested = aliases.get(hostname);
    let matching_ids: std::collections::HashSet<&str> = current
        .iter()
        .filter(|(name, _)| aliases.get(name) == requested)
        .map(|(_, id)| id.as_str())
        .collect();
    if matching_ids.len() > 1 {
        return Err(anyhow!("ambiguous_host"));
    }
    if let Some(current_id) = matching_ids.into_iter().next() {
        // A renamed device's historical name may later be reused by another
        // device. Current cache entries cannot erase that ambiguity. Check only
        // the exact requested/current alias names using the existing index.
        let mut history_names: Vec<&str> = current
            .iter()
            .filter(|(name, _)| aliases.get(name) == requested)
            .map(|(name, _)| name.as_str())
            .collect();
        history_names.push(hostname);
        history_names.sort_unstable();
        history_names.dedup();
        let mut stmt = conn
            .prepare("SELECT DISTINCT host_id FROM host_heartbeats WHERE hostname = ?1 LIMIT 2")?;
        for name in history_names {
            let ids = stmt
                .query_map([name], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if ids.iter().any(|id| id != current_id) {
                return Err(anyhow!("ambiguous_host"));
            }
        }
        return Ok(current_id.to_owned());
    }
    // Preserve indexed exact-name lookup for historical names after a rename.
    let mut stmt = conn.prepare(
        "SELECT host_id
         FROM host_heartbeats
         WHERE hostname = ?1
         GROUP BY host_id
         ORDER BY MAX(received_at) DESC
         LIMIT 2",
    )?;
    let host_ids = stmt
        .query_map([hostname], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match host_ids.as_slice() {
        [] => Err(anyhow!("not_found")),
        [host_id] => Ok(host_id.clone()),
        _ => Err(anyhow!("ambiguous_host")),
    }
}

fn map_heartbeat_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HeartbeatRow> {
    Ok(HeartbeatRow {
        id: row.get(0)?,
        host_id: row.get(1)?,
        hostname: row.get(2)?,
        source_ip: row.get(3)?,
        sampled_at: row.get(4)?,
        received_at: row.get(5)?,
        boot_id: row.get(6)?,
        uptime_secs: row.get(7)?,
        sequence: row.get(8)?,
        collection_ms: row.get(9)?,
        partial: row.get::<_, i64>(10)? != 0,
        agent_version: row.get(11)?,
        os: row.get(12)?,
        kernel: row.get(13)?,
        architecture: row.get(14)?,
        metadata_json: row.get(15)?,
    })
}

fn sample_from_row(conn: &rusqlite::Connection, row: HeartbeatRow) -> Result<HeartbeatSampleState> {
    Ok(HeartbeatSampleState {
        heartbeat_id: row.id,
        host_id: row.host_id,
        hostname: row.hostname,
        sampled_at: row.sampled_at,
        received_at: row.received_at,
        source_ip: row.source_ip,
        boot_id: row.boot_id,
        sequence: row.sequence,
        uptime_secs: row.uptime_secs,
        collection_ms: row.collection_ms,
        partial: row.partial,
        agent_version: row.agent_version,
        os: row.os,
        kernel: row.kernel,
        architecture: row.architecture,
        metadata: row
            .metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
        cpu: one_json(
            conn,
            "SELECT json_object(
                 'load1', load1, 'load5', load5, 'load15', load15,
                 'usage_percent', usage_percent, 'steal_percent', steal_percent,
                 'io_wait_percent', io_wait_percent
             ) FROM heartbeat_cpu WHERE heartbeat_id = ?1",
            row.id,
        )?,
        memory: one_json(
            conn,
            "SELECT json_object(
                 'total_bytes', total_bytes, 'available_bytes', available_bytes,
                 'used_percent', used_percent, 'swap_total_bytes', swap_total_bytes,
                 'swap_used_bytes', swap_used_bytes
             ) FROM heartbeat_memory WHERE heartbeat_id = ?1",
            row.id,
        )?,
        disks: many_json(
            conn,
            "SELECT json_object(
                 'mountpoint', mountpoint, 'filesystem', filesystem,
                 'total_bytes', total_bytes, 'available_bytes', available_bytes,
                 'used_percent', used_percent, 'read_bytes_per_sec', read_bytes_per_sec,
                 'write_bytes_per_sec', write_bytes_per_sec
             ) FROM heartbeat_disks WHERE heartbeat_id = ?1 ORDER BY id ASC",
            row.id,
        )?,
        network: many_json(
            conn,
            "SELECT json_object(
                 'interface', interface, 'rx_bytes_per_sec', rx_bytes_per_sec,
                 'tx_bytes_per_sec', tx_bytes_per_sec, 'rx_errors', rx_errors,
                 'tx_errors', tx_errors
             ) FROM heartbeat_network WHERE heartbeat_id = ?1 ORDER BY id ASC",
            row.id,
        )?,
        processes: one_json(
            conn,
            "SELECT json_object(
                 'total', total, 'running', running, 'sleeping', sleeping, 'zombie', zombie,
                 'top_cpu', json(top_cpu_json), 'top_memory', json(top_memory_json)
             ) FROM heartbeat_processes WHERE heartbeat_id = ?1",
            row.id,
        )?,
        containers: many_json(
            conn,
            "SELECT json_object(
                 'runtime', runtime, 'running', running, 'stopped', stopped,
                 'restarting', restarting, 'unhealthy', unhealthy, 'summary', json(summary_json)
             ) FROM heartbeat_containers WHERE heartbeat_id = ?1 ORDER BY id ASC",
            row.id,
        )?,
    })
}

fn one_json(conn: &rusqlite::Connection, sql: &str, heartbeat_id: i64) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row(sql, [heartbeat_id], |row| row.get(0))
        .optional()?;
    Ok(raw.and_then(|raw| {
        serde_json::from_str(&raw)
            .map_err(|error| {
                warn!(heartbeat_id, error = %error, "failed to parse heartbeat JSON column");
            })
            .ok()
    }))
}

fn many_json(conn: &rusqlite::Connection, sql: &str, heartbeat_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([heartbeat_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|error| {
                    warn!(heartbeat_id, error = %error, "failed to parse heartbeat JSON row");
                })
                .ok()
        })
        .collect())
}

#[cfg(test)]
#[path = "heartbeat_tests.rs"]
mod tests;
