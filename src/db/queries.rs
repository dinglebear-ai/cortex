//! All read-path SQL for the log intelligence core lives here — every SELECT
//! behind the MCP actions, `/api/*` routes, and direct CLI queries.
//!
//! Key invariants:
//! - **All query SQL lives in this module** (deletes live in `maintenance.rs`,
//!   schema in `pool.rs`). Handlers never build SQL strings.
//! - Every query uses parameterized bindings — no user input is interpolated.
//! - FTS5 searches JOIN `logs_fts` back to `logs`, which prunes phantom rows
//!   left behind by retention/storage deletes at query time.
//! - The FTS fast path caps match-set materialization at the 200K most-recent
//!   matches (`SEARCH_FTS_FAST_PATH_MATCH_CAP`); severity-only filtered
//!   searches use the same capped candidate plan.
//! - Unbounded `sessions` reads are served from the `ai_session_rollup`
//!   materialization; time-windowed reads run live against `logs`.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use crate::config::StorageConfig;
use crate::enrich::parser::SourceKind;

use super::entity_resolution::{
    FALLBACK_EXPLICIT_DEGRADED_HOST_CONTEXT, INCLUSION_GRAPH_RELATED, INCLUSION_HOST_CONTEXT,
    ResolverStatus,
};
use super::maintenance::{exceeds_trigger, get_storage_metrics};
use super::models::{
    AbuseIncident, AiAbuseMatch, AiAbuseParams, AiAbuseResult, AiCorrelateParams, AiIncidentParams,
    AiIncidentResult, AiInvestigateParams, AiInvestigateResult, AiProjectInventoryEntry,
    AiRelatedLogsForAnchor, AiRelatedLogsParams, AiSessionEntry, AiToolInventoryEntry, DbStats,
    DurableStreamPage, DurableStreamParams, DurableStreamRow, ErrorSummaryEntry,
    GraphRelatedLogEntry, IncidentEvidence, ListAiProjectsParams, ListAiProjectsResult,
    ListAiSessionsParams, ListAiToolsParams, ListAiToolsResult, LogEntry, RenderedSessionEventRow,
    RenderedSessionPageParams, ResolvedTopicEntity, SearchAiSessionsParams, SearchAiSessionsResult,
    SearchParams, SearchedAiSessionEntry, SessionGraphInputs, TopicGraphInputs,
};
use super::pool::DbPool;
use super::queries_service_instances;

const SEARCH_FTS_CANDIDATE_CAP: usize = 10_000;
const SIMILAR_INCIDENT_FTS_CANDIDATE_CAP: usize = 5_000;
/// Cap on the FTS match-set materialization in the fast (index-led) search
/// path. The `id IN (SELECT rowid FROM logs_fts ...)` subquery is
/// non-correlated, so SQLite materializes the whole match set into an
/// ephemeral index before walking the filter's composite index — for a common
/// term on a multi-million-row DB that was unbounded memory and a full FTS
/// walk (full-review PH1). 200K most-recent matches is recency-biased and far
/// larger than any result LIMIT (search caps at 1000), so in practice results
/// are unaffected; matches older than the newest 200K are no longer
/// intersected.
const SEARCH_FTS_FAST_PATH_MATCH_CAP: usize = 200_000;

fn push_bound_limit(
    sql: &mut String,
    bindings: &mut Vec<rusqlite::types::Value>,
    idx: &mut usize,
    keyword: &str,
    limit: impl Into<i64>,
) {
    let idx_value = *idx;
    bindings.push(rusqlite::types::Value::Integer(limit.into()));
    *idx += 1;
    sql.push_str(&format!(" {keyword} ?{idx_value}"));
}

/// Detect common FTS5 foot-guns and return a fix-it error. Runs before the
/// generic length/term-count checks in [`validate_fts_query`].
///
/// - A whitespace-separated term with a non-leading hyphen (e.g. `smoke-test`)
///   is parsed by FTS5 as `smoke NOT test`, which surprises users searching a
///   hyphenated word. The check is per-term: a leading-hyphen term (`-nginx`)
///   is an intentional NOT and is left alone, and a term that is part of a
///   quoted phrase (contains a `"`) is skipped — so `"disk full" smoke-test`
///   still flags the unquoted `smoke-test`.
/// - An odd number of double-quotes is an unterminated phrase.
fn lint_fts_query(query: &str) -> Result<()> {
    let has_unquoted_hyphen = query
        .split_whitespace()
        .any(|t| !t.contains('"') && t.len() > 1 && t.contains('-') && !t.starts_with('-'));
    if has_unquoted_hyphen {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            "hyphen is the FTS5 NOT operator; quote hyphenated terms as a phrase \
             (e.g. \"smoke-test\") or use --grep for literal text"
                .to_string(),
        )));
    }
    if !query.matches('"').count().is_multiple_of(2) {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            "unbalanced quote in search query; wrap phrases in matching double quotes".to_string(),
        )));
    }
    Ok(())
}

/// Validate a user-supplied FTS5 query before execution.
///
/// Limits:
/// - Max 512 characters (prevents very long queries from taxing the FTS tokenizer)
/// - Max 16 whitespace-separated terms (prevents 28+ wildcard term DoS)
///
/// Returns a user-friendly error; the caller logs the details server-side.
pub fn validate_fts_query(query: &str) -> Result<()> {
    lint_fts_query(query)?;
    if query.len() > 512 {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            format!(
                "Search query too long ({} chars); maximum is 512 characters",
                query.len()
            ),
        )));
    }
    let term_count = query.split_whitespace().count();
    if term_count > 16 {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            format!("Search query has too many terms ({term_count}); maximum is 16 terms"),
        )));
    }
    Ok(())
}

/// Column list for the FTS result projection (must match `map_row`'s order).
pub(super) const FTS_SELECT_COLS: &str = "l.id, l.timestamp, l.hostname, l.facility, l.severity, \
     l.app_name, l.process_id, l.message, l.received_at, l.source_ip, \
     l.ai_tool, l.ai_project, l.ai_session_id, l.ai_transcript_path, l.metadata_json";

fn search_logs_fts_sql(
    query: &str,
    params: &SearchParams,
    limit: u32,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut bindings: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Text(query.to_string())];
    let mut idx = 2;

    if params.has_indexed_equality_filter() {
        // Fast path: a selective indexed equality filter (hostname / source_ip
        // / app_name / event_action / ai_project — NOT severity, see
        // `has_indexed_equality_filter`) is present. Lead with that filter's
        // composite `(<col>, timestamp)` index and intersect against the FTS
        // match set via a bloom-filtered `id IN (...)` subquery. SQLite walks
        // the filtered partition newest-first and stops at LIMIT, so a
        // host-scoped search of a common term drops from ~200s (full FTS scan)
        // to sub-second.
        //
        // The match-set subquery is capped at the most-recent
        // SEARCH_FTS_FAST_PATH_MATCH_CAP rowids: the non-correlated IN
        // subquery is materialized in full before the index walk, which was
        // unbounded memory for common terms (full-review PH1). Results for
        // matches older than the newest 200K are dropped — callers needing
        // deeper history should narrow the time range.
        let mut sql = format!(
            "SELECT {FTS_SELECT_COLS}
             FROM logs l
             WHERE l.id IN (SELECT rowid FROM logs_fts WHERE logs_fts MATCH ?1
                            ORDER BY rowid DESC LIMIT {SEARCH_FTS_FAST_PATH_MATCH_CAP})"
        );
        append_filters(&mut sql, &mut bindings, &mut idx, params);
        sql.push_str(" ORDER BY l.timestamp DESC, l.id DESC");
        push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", limit);
        return (sql, bindings);
    }

    // Default path (no indexed equality filter): materialize the most-recent
    // FTS candidates by rowid, capped, then project. Fast when unfiltered
    // because it never sorts the full match set; the cap bounds the work.
    let mut sql = String::from(
        "WITH fts_candidates(id, ts) AS MATERIALIZED (
            SELECT l.id, l.timestamp
            FROM logs_fts
            JOIN logs l ON l.id = logs_fts.rowid
            WHERE logs_fts MATCH ?1",
    );
    append_filters(&mut sql, &mut bindings, &mut idx, params);
    sql.push_str(" ORDER BY logs_fts.rowid DESC");
    push_bound_limit(
        &mut sql,
        &mut bindings,
        &mut idx,
        "LIMIT",
        SEARCH_FTS_CANDIDATE_CAP as i64,
    );
    sql.push_str(&format!(
        "
         )
         SELECT {FTS_SELECT_COLS}
         FROM fts_candidates c
         JOIN logs l ON l.id = c.id
         ORDER BY c.ts DESC, l.id DESC"
    ));
    push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", limit);
    (sql, bindings)
}

#[derive(Debug, Default)]
struct SqlParams {
    bindings: Vec<rusqlite::types::Value>,
    next_idx: usize,
}

impl SqlParams {
    fn new(next_idx: usize) -> Self {
        Self {
            bindings: Vec::new(),
            next_idx,
        }
    }

    fn push_text(&mut self, value: String) -> usize {
        let idx = self.next_idx;
        self.bindings.push(rusqlite::types::Value::Text(value));
        self.next_idx += 1;
        idx
    }
}

fn push_required_ai_filters(sql: &mut String, alias: &str) {
    sql.push_str(&format!(
        " AND {alias}.ai_project IS NOT NULL AND {alias}.ai_project != ''
          AND {alias}.ai_tool IS NOT NULL AND {alias}.ai_tool != ''
          AND {alias}.ai_session_id IS NOT NULL AND {alias}.ai_session_id != ''"
    ));
}

fn push_ai_scope_filters(
    sql: &mut String,
    params: &mut SqlParams,
    alias: &str,
    project: &Option<String>,
    tool: &Option<String>,
    from: &Option<String>,
    to: &Option<String>,
) {
    if let Some(project) = project {
        let idx = params.push_text(project.clone());
        sql.push_str(&format!(" AND {alias}.ai_project = ?{idx}"));
    }
    if let Some(tool) = tool {
        let idx = params.push_text(tool.clone());
        sql.push_str(&format!(" AND {alias}.ai_tool = ?{idx}"));
    }
    if let Some(from) = from {
        let idx = params.push_text(from.clone());
        sql.push_str(&format!(" AND {alias}.timestamp >= ?{idx}"));
    }
    if let Some(to) = to {
        let idx = params.push_text(to.clone());
        sql.push_str(&format!(" AND {alias}.timestamp <= ?{idx}"));
    }
}

/// Search logs with flexible filtering + FTS
pub fn search_logs(pool: &DbPool, params: &SearchParams) -> Result<Vec<LogEntry>> {
    if params
        .source_ip_prefix
        .as_ref()
        .is_some_and(|prefix| prefix.is_empty())
        || params
            .source_ip_prefixes
            .as_ref()
            .is_some_and(|prefixes| prefixes.is_empty() || prefixes.iter().any(String::is_empty))
    {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            "source prefixes must be non-empty".to_string(),
        )));
    }
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(100).min(1000);

    // If we have a full-text query, use FTS5 join
    if let Some(ref query) = params.query {
        validate_fts_query(query)?;

        let (sql, bindings) = search_logs_fts_sql(query, params, limit);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bindings.iter()), map_row)
            .map_err(|e| {
                tracing::error!(error = %e, query = %query, "FTS5 MATCH query failed");
                anyhow::anyhow!("Search query failed")
            })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| {
            tracing::error!(error = %e, query = %query, "FTS5 row mapping failed");
            anyhow::anyhow!("Search query failed")
        })
    } else {
        let mut sql = String::from(
            "SELECT l.id, l.timestamp, l.hostname, l.facility, l.severity,
                    l.app_name, l.process_id, l.message, l.received_at, l.source_ip,
                    l.ai_tool, l.ai_project, l.ai_session_id, l.ai_transcript_path, l.metadata_json
             FROM logs l WHERE 1=1",
        );
        let mut bindings: Vec<rusqlite::types::Value> = vec![];
        let mut idx = 1;

        append_filters(&mut sql, &mut bindings, &mut idx, params);
        sql.push_str(" ORDER BY l.timestamp DESC");
        push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", limit);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Get the N most recent logs for a host/service
pub fn tail_logs(
    pool: &DbPool,
    hostname: Option<&str>,
    source_ip: Option<&str>,
    app_name: Option<&str>,
    severity_in: Option<&[String]>,
    n: u32,
) -> Result<Vec<LogEntry>> {
    let conn = pool.get()?;
    let (sql, bindings) = tail_logs_sql(hostname, source_ip, app_name, severity_in, n);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn page_agent_projection_logs(
    pool: &DbPool,
    after_id: i64,
    limit: usize,
) -> Result<Vec<LogEntry>> {
    if after_id < 0 || !(1..=500).contains(&limit) {
        anyhow::bail!("projection log cursor/limit out of bounds");
    }
    let conn = pool.get()?;
    let mut statement = conn.prepare(
        "SELECT id, timestamp, hostname, facility, severity,
                app_name, process_id, message, received_at, source_ip,
                ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
           FROM logs WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    Ok(statement
        .query_map(rusqlite::params![after_id, limit as i64], map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn tail_logs_sql(
    hostname: Option<&str>,
    source_ip: Option<&str>,
    app_name: Option<&str>,
    severity_in: Option<&[String]>,
    n: u32,
) -> (String, Vec<rusqlite::types::Value>) {
    let n = n.min(500);

    // Severity-only fast path: with no other filter, the generic plan walks
    // idx_logs_timestamp newest-first and filters — O(table) when the
    // requested severities are rare (e.g. `tail severity_in=[emerg,alert]`,
    // full-review PM6). Instead probe `idx_logs_sev_time (severity,
    // timestamp)` once per severity with its own LIMIT and merge: each arm is
    // a bounded index walk, and the outer sort covers at most
    // severities × n ≤ 8 × 500 rows. `n` is server-clamped, so interpolating
    // it is safe.
    if hostname.is_none()
        && source_ip.is_none()
        && app_name.is_none()
        && let Some(levels) = severity_in.filter(|levels| !levels.is_empty())
    {
        const COLS: &str = "id, timestamp, hostname, facility, severity, \
                 app_name, process_id, message, received_at, source_ip, \
                 ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json";
        let mut bindings: Vec<rusqlite::types::Value> = Vec::with_capacity(levels.len());
        let arms = levels
            .iter()
            .enumerate()
            .map(|(i, lvl)| {
                bindings.push(rusqlite::types::Value::Text(lvl.clone()));
                format!(
                    "SELECT * FROM (SELECT {COLS} FROM logs WHERE severity = ?{} \
                         ORDER BY timestamp DESC LIMIT {n})",
                    i + 1
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let sql = format!("{arms} ORDER BY timestamp DESC LIMIT {n}");
        return (sql, bindings);
    }

    let mut sql = String::from(
        "SELECT id, timestamp, hostname, facility, severity,
                app_name, process_id, message, received_at, source_ip,
                ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
         FROM logs WHERE 1=1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = vec![];
    let mut idx = 1;

    if let Some(h) = hostname {
        append_host_selector(&mut sql, &mut bindings, &mut idx, "hostname", h);
    }
    if let Some(source_ip) = source_ip {
        sql.push_str(&format!(" AND source_ip = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(source_ip.to_string()));
        idx += 1;
    }
    if let Some(a) = app_name {
        sql.push_str(&format!(" AND app_name = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(levels) = severity_in
        && !levels.is_empty()
    {
        let placeholders: Vec<String> =
            (0..levels.len()).map(|i| format!("?{}", idx + i)).collect();
        sql.push_str(&format!(" AND severity IN ({})", placeholders.join(", ")));
        for lvl in levels {
            bindings.push(rusqlite::types::Value::Text(lvl.clone()));
            idx += 1;
        }
        debug_assert_eq!(bindings.len() + 1, idx);
    }

    sql.push_str(" ORDER BY timestamp DESC");
    push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", n);
    (sql, bindings)
}

/// Get error/warning summary per host in a time window. When `group_by_app` is
/// true, results also include `app_name` as a secondary grouping key.
pub fn get_error_summary(
    pool: &DbPool,
    from: Option<&str>,
    to: Option<&str>,
    group_by_app: bool,
    limit: Option<u32>,
) -> Result<Vec<ErrorSummaryEntry>> {
    let conn = pool.get()?;
    let (sql, bindings) = get_error_summary_sql(from, to, group_by_app, limit);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
        Ok(ErrorSummaryEntry {
            hostname: row.get(0)?,
            app_name: row.get::<_, Option<String>>(1)?,
            severity: row.get(2)?,
            count: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn get_error_summary_sql(
    from: Option<&str>,
    to: Option<&str>,
    group_by_app: bool,
    limit: Option<u32>,
) -> (String, Vec<rusqlite::types::Value>) {
    let from = from.unwrap_or("1970-01-01T00:00:00Z");
    // Upper sentinel: any valid RFC 3339 timestamp will sort before this.
    let to = to.unwrap_or("9999-12-31T23:59:59Z");

    let mut bindings = vec![
        rusqlite::types::Value::Text(from.to_string()),
        rusqlite::types::Value::Text(to.to_string()),
    ];
    let mut idx = 3usize;

    let mut sql = if group_by_app {
        "SELECT hostname, app_name, severity, COUNT(*) as count
         FROM logs
         WHERE severity IN ('emerg', 'alert', 'crit', 'err', 'warning')
           AND timestamp BETWEEN ?1 AND ?2
         GROUP BY hostname, app_name, severity
         ORDER BY hostname, app_name, count DESC"
            .to_string()
    } else {
        "SELECT hostname, NULL AS app_name, severity, COUNT(*) as count
         FROM logs
         WHERE severity IN ('emerg', 'alert', 'crit', 'err', 'warning')
           AND timestamp BETWEEN ?1 AND ?2
         GROUP BY hostname, severity
         ORDER BY hostname, count DESC"
            .to_string()
    };
    if let Some(limit) = limit {
        push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", limit.max(1));
    }
    (sql, bindings)
}

pub(crate) use super::queries_hosts::canonical_host_keys;
pub use super::queries_hosts::list_hosts;

/// List AI transcript sessions ordered by recency.
///
/// Fast path (bead cortex-2vre): when the caller supplies NO time window
/// (`from`/`to` both unset) the result is served from the periodically-refreshed
/// `ai_session_rollup` materialization — an O(#sessions) indexed read instead of
/// the O(#AI-rows) GROUP-BY + temp-btree sort that grew to ~4s at 10M rows. The
/// rollup is refreshed on a background cadence, so unbounded results reflect data
/// as of the last refresh; reach for [`ai_session_rollup_status`] to surface
/// staleness. If the rollup has never been refreshed (e.g. immediately after a
/// migration, before the background task runs) the fast path transparently falls
/// back to the live aggregation, so correctness never depends on the rollup being
/// warm.
///
/// Slow/exact path: when a time window IS supplied, the query is bounded by the
/// timestamp index and runs live against `logs` (the rollup pre-aggregates across
/// all time and cannot answer a windowed `event_count`/`first_seen`/`last_seen`).
pub fn list_ai_sessions(
    pool: &DbPool,
    params: &ListAiSessionsParams,
) -> Result<Vec<AiSessionEntry>> {
    let time_filtered = params.since.is_some() || params.until.is_some();
    if !time_filtered && ai_session_rollup_is_populated(pool)? {
        return list_ai_sessions_from_rollup(pool, params);
    }
    list_ai_sessions_live(pool, params)
}

/// Read one stable session in durable insertion order. The extra row lets the
/// service report truncation without an exact count or a deep OFFSET scan.
pub fn rendered_session_page(
    pool: &DbPool,
    params: &RenderedSessionPageParams,
) -> Result<(Vec<RenderedSessionEventRow>, bool)> {
    let conn = pool.get()?;
    let fetch = params.limit.clamp(1, 201);
    let mut stmt = conn.prepare_cached(
        "SELECT CAST(id AS INTEGER), substr(timestamp,1,128), substr(message,1,65536),
          CASE WHEN length(metadata_json)<=8192 THEN metadata_json END,
          CASE WHEN length(message)>65536 OR length(metadata_json)>8192 OR length(parse_error)>1024
            THEN 'source fields truncated; ' || substr(coalesce(parse_error,''),1,1024)
            ELSE parse_error END
         FROM logs
         WHERE ai_project = ?1
           AND ai_tool = ?2
           AND ai_session_id = ?3
           AND hostname = ?4
           AND id > ?5
         ORDER BY id ASC
         LIMIT ?6",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            params.ai_project,
            params.ai_tool,
            params.ai_session_id,
            params.host,
            params.after_id,
            fetch,
        ],
        |row| {
            Ok(RenderedSessionEventRow {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                message: row.get(2)?,
                metadata_json: row.get(3)?,
                parse_error: row.get(4)?,
            })
        },
    )?;
    let mut page = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let row = row?;
        let row_bytes = row.message.len()
            + row.metadata_json.as_ref().map_or(0, String::len)
            + row.parse_error.as_ref().map_or(0, String::len)
            + row.timestamp.len();
        if !page.is_empty() && bytes.saturating_add(row_bytes) > 1024 * 1024 {
            return Ok((page, true));
        }
        bytes += row_bytes;
        page.push(row);
    }
    Ok((page, false))
}

pub fn durable_stream_page(
    pool: &DbPool,
    params: &DurableStreamParams,
) -> Result<DurableStreamPage> {
    let conn = pool.get()?;
    let mut sql = String::from(
        "SELECT CAST(id AS INTEGER),timestamp,hostname,severity,app_name,message,metadata_json,parse_error FROM logs WHERE id > ?1",
    );
    let mut values = vec![rusqlite::types::Value::Integer(params.after_id)];
    let mut push_filter = |column: &str, value: &Option<String>| {
        if let Some(value) = value {
            values.push(rusqlite::types::Value::Text(value.clone()));
            sql.push_str(&format!(" AND {column} = ?{}", values.len()));
        }
    };
    push_filter("hostname", &params.hostname);
    push_filter("app_name", &params.app_name);
    push_filter("severity", &params.severity);
    push_filter("ai_project", &params.ai_project);
    push_filter("ai_tool", &params.ai_tool);
    push_filter("ai_session_id", &params.ai_session_id);
    if let Some(high) = params.high_watermark {
        values.push(rusqlite::types::Value::Integer(high));
        sql.push_str(&format!(" AND id <= ?{}", values.len()));
    }
    values.push(rusqlite::types::Value::Integer(i64::from(
        params.limit.clamp(1, 101),
    )));
    sql.push_str(&format!(" ORDER BY id ASC LIMIT ?{}", values.len()));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(DurableStreamRow {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                hostname: row.get(2)?,
                severity: row.get(3)?,
                app_name: row.get(4)?,
                message: row.get(5)?,
                metadata_json: row.get(6)?,
                parse_error: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (minimum_watermark, high_watermark) = if params.include_bounds {
        let (retained_sql, retained_values) = stream_bounds_sql("logs", params, false);
        let retained: (Option<i64>, i64) = conn.query_row(
            &retained_sql,
            rusqlite::params_from_iter(retained_values.iter()),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (deleted_sql, deleted_values) =
            stream_bounds_sql("stream_deleted_log_lineage", params, true);
        let deleted: Option<i64> = conn.query_row(
            &deleted_sql,
            rusqlite::params_from_iter(deleted_values.iter()),
            |row| row.get(0),
        )?;
        let floor = deleted
            .map(|last_deleted| last_deleted.saturating_add(1))
            .or(retained.0);
        (floor, retained.1.max(deleted.unwrap_or(0)))
    } else {
        (None, params.high_watermark.unwrap_or(params.after_id))
    };
    Ok(DurableStreamPage {
        rows,
        minimum_watermark,
        high_watermark,
    })
}

/// Build the initial retention-bound query from the exact supported filter
/// shape. Nullable `(? IS NULL OR column = ?)` predicates make SQLite ignore
/// the stream indexes, so every shape deliberately emits only direct equality
/// predicates. Session streams are a separate, fully-qualified shape.
pub(crate) fn stream_bounds_sql(
    table: &str,
    params: &DurableStreamParams,
    deleted: bool,
) -> (String, Vec<rusqlite::types::Value>) {
    let select = if deleted {
        "SELECT MAX(id)"
    } else if params.hostname.is_none()
        && params.app_name.is_none()
        && params.severity.is_none()
        && params.ai_project.is_none()
        && params.ai_tool.is_none()
        && params.ai_session_id.is_none()
    {
        return (
            format!(
                "SELECT (SELECT id FROM {table} WHERE id >= 0 ORDER BY id ASC LIMIT 1), \
                 COALESCE((SELECT id FROM {table} WHERE id >= 0 ORDER BY id DESC LIMIT 1), 0)"
            ),
            Vec::new(),
        );
    } else {
        "SELECT MIN(id), COALESCE(MAX(id), 0)"
    };
    let mut sql = format!("{select} FROM {table}");
    let mut values = Vec::new();
    let mut predicates: Vec<&str> = Vec::new();

    if deleted {
        predicates.push("deleted_at >= unixepoch() - 900");
    }
    let mut direct = |column: &'static str, value: &Option<String>| {
        if let Some(value) = value {
            values.push(rusqlite::types::Value::Text(value.clone()));
            predicates.push(column);
        }
    };
    direct("hostname = ?", &params.hostname);
    direct("app_name = ?", &params.app_name);
    direct("severity = ?", &params.severity);
    direct("ai_project = ?", &params.ai_project);
    direct("ai_tool = ?", &params.ai_tool);
    direct("ai_session_id = ?", &params.ai_session_id);

    if !predicates.is_empty() {
        sql.push_str(" WHERE ");
        let mut parameter = 0usize;
        for (index, predicate) in predicates.iter().enumerate() {
            if index > 0 {
                sql.push_str(" AND ");
            }
            if *predicate == "deleted_at >= unixepoch() - 900" {
                sql.push_str(predicate);
            } else {
                parameter += 1;
                sql.push_str(&predicate.replace('?', &format!("?{parameter}")));
            }
        }
    }
    (sql, values)
}

pub fn prune_expired_stream_lineage(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    Ok(conn.execute(
        "DELETE FROM stream_deleted_log_lineage WHERE deleted_at < unixepoch() - 900",
        [],
    )?)
}

/// Live aggregation over `logs`. This is the ground-truth implementation used
/// for time-windowed queries and to (re)compute the rollup.
pub fn list_ai_sessions_live(
    pool: &DbPool,
    params: &ListAiSessionsParams,
) -> Result<Vec<AiSessionEntry>> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let mut sql = String::from(
        "WITH filtered_logs AS MATERIALIZED (
            SELECT id, ai_project, ai_tool, ai_session_id, ai_transcript_path,
                   hostname, timestamp,
                   COALESCE(json_extract(metadata_json, '$.session.title'),
                            json_extract(metadata_json, '$.source.title')) AS session_title,
                   COALESCE(json_extract(metadata_json, '$.session.title_provenance'),
                            json_extract(metadata_json, '$.source.title_provenance')) AS session_title_provenance
         FROM logs
         WHERE ai_project IS NOT NULL
           AND ai_project != ''
           AND ai_tool IS NOT NULL
           AND ai_tool != ''
           AND ai_session_id IS NOT NULL
           AND ai_session_id != ''",
    );
    let mut bindings: Vec<rusqlite::types::Value> = vec![];
    let mut idx = 1;

    if let Some(project) = &params.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(project.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
        idx += 1;
    }
    if let Some(session_id) = &params.ai_session_id {
        sql.push_str(&format!(" AND ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(session_id.clone()));
        idx += 1;
    }
    if let Some(hostname) = &params.host {
        sql.push_str(&format!(" AND hostname = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(hostname.clone()));
        idx += 1;
    }
    if let Some(from) = &params.since {
        sql.push_str(&format!(" AND timestamp >= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(from.clone()));
        idx += 1;
    }
    if let Some(to) = &params.until {
        sql.push_str(&format!(" AND timestamp <= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(to.clone()));
    }

    sql.push_str(&format!(
        "),
         latest_titles AS MATERIALIZED (
            SELECT ai_project, ai_tool, ai_session_id, hostname,
                   session_title, session_title_provenance
              FROM (
                SELECT ai_project, ai_tool, ai_session_id, hostname,
                       session_title, session_title_provenance,
                       ROW_NUMBER() OVER (
                           PARTITION BY ai_project, ai_tool, ai_session_id, hostname
                           ORDER BY id DESC
                       ) AS title_rank
                  FROM filtered_logs
                 WHERE session_title IS NOT NULL
              )
             WHERE title_rank = 1
         )
         SELECT f.ai_project, f.ai_tool, f.ai_session_id,
                MIN(f.ai_transcript_path) AS ai_transcript_path,
                f.hostname,
                MIN(f.timestamp) AS first_seen,
                MAX(f.timestamp) AS last_seen,
                COUNT(*) AS event_count,
                t.session_title AS title,
                t.session_title_provenance AS title_provenance
           FROM filtered_logs f
           LEFT JOIN latest_titles t
             ON t.ai_project = f.ai_project
            AND t.ai_tool = f.ai_tool
            AND t.ai_session_id = f.ai_session_id
            AND t.hostname = f.hostname
          GROUP BY f.ai_project, f.ai_tool, f.ai_session_id, f.hostname,
                   t.session_title, t.session_title_provenance
          ORDER BY last_seen DESC
          LIMIT {limit}"
    ));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
        Ok(AiSessionEntry {
            ai_project: row.get(0)?,
            ai_tool: row.get(1)?,
            ai_session_id: row.get(2)?,
            ai_transcript_path: row.get(3)?,
            hostname: row.get(4)?,
            first_seen: row.get(5)?,
            last_seen: row.get(6)?,
            event_count: row.get(7)?,
            title: row.get(8)?,
            title_provenance: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Indexed read from the `ai_session_rollup` materialization (no time window).
fn list_ai_sessions_from_rollup(
    pool: &DbPool,
    params: &ListAiSessionsParams,
) -> Result<Vec<AiSessionEntry>> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let mut sql = String::from(
        "SELECT ai_project, ai_tool, ai_session_id, ai_transcript_path,
                hostname, first_seen, last_seen, event_count,
                title, title_provenance
         FROM ai_session_rollup
         WHERE 1=1",
    );
    let mut bindings: Vec<rusqlite::types::Value> = vec![];
    let mut idx = 1;
    if let Some(project) = &params.ai_project {
        sql.push_str(&format!(" AND ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(project.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.ai_tool {
        sql.push_str(&format!(" AND ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
        idx += 1;
    }
    if let Some(session_id) = &params.ai_session_id {
        sql.push_str(&format!(" AND ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(session_id.clone()));
        idx += 1;
    }
    if let Some(hostname) = &params.host {
        sql.push_str(&format!(" AND hostname = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(hostname.clone()));
    }
    // Order by last_seen DESC ONLY — exactly mirroring the live path's
    // `ORDER BY last_seen DESC`. A single-column order lets SQLite serve the
    // sort straight from idx_ai_session_rollup_last_seen with NO temp b-tree
    // (the cost that made the live aggregation slow). Adding tiebreak columns
    // would reintroduce a temp b-tree, so ties stay engine-arbitrary here just
    // as they are in the live query.
    sql.push_str(&format!(" ORDER BY last_seen DESC LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
        Ok(AiSessionEntry {
            ai_project: row.get(0)?,
            ai_tool: row.get(1)?,
            ai_session_id: row.get(2)?,
            ai_transcript_path: row.get(3)?,
            hostname: row.get(4)?,
            first_seen: row.get(5)?,
            last_seen: row.get(6)?,
            event_count: row.get(7)?,
            title: row.get(8)?,
            title_provenance: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// True once the rollup has been refreshed at least once (`refreshed_at` set).
/// Before the first refresh, `list_ai_sessions` falls back to the live path.
fn ai_session_rollup_is_populated(pool: &DbPool) -> Result<bool> {
    let conn = pool.get()?;
    let refreshed: Option<String> = conn
        .query_row(
            "SELECT refreshed_at FROM ai_session_rollup_meta WHERE id = 1",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(refreshed.is_some())
}

/// Staleness snapshot for the AI session rollup.
#[derive(Debug, Clone)]
pub struct AiSessionRollupStatus {
    /// RFC 3339 timestamp of the last successful refresh, or `None` if never.
    pub refreshed_at: Option<String>,
    /// Number of session rows in the rollup as of the last refresh.
    pub row_count: i64,
}

/// Read the rollup staleness metadata (cheap single-row lookup).
pub fn ai_session_rollup_status(pool: &DbPool) -> Result<AiSessionRollupStatus> {
    let conn = pool.get()?;
    let (refreshed_at, row_count) = conn
        .query_row(
            "SELECT refreshed_at, row_count FROM ai_session_rollup_meta WHERE id = 1",
            [],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
        .unwrap_or((None, 0));
    Ok(AiSessionRollupStatus {
        refreshed_at,
        row_count,
    })
}

impl AiSessionRollupStatus {
    /// Human-readable staleness summary, e.g. for `db status` / diagnostics:
    /// `"42 sessions, refreshed 2026-05-29T12:00:00.000Z"` or `"never refreshed"`.
    pub fn summary(&self) -> String {
        match &self.refreshed_at {
            Some(ts) => format!("{} sessions, refreshed {ts}", self.row_count),
            None => "never refreshed".to_string(),
        }
    }
}

/// Cheap source-side fingerprint of the AI-row partition used to decide whether
/// the rollup is stale. `(COUNT(*), MAX(id))` over rows that *could* contribute
/// to the rollup. Computed index-only from `idx_logs_ai_project_time`
/// (partial index `WHERE ai_project IS NOT NULL`): the `!= ''` residual is on
/// the index's leading column, and `id` is the implicit rowid carried in every
/// index entry, so neither a table lookup nor a temp b-tree is needed.
///
/// The predicate is intentionally BROADER than the rollup's contributing-row
/// filter (it omits the `ai_tool`/`ai_session_id` checks): any row that
/// contributes to the rollup necessarily has `ai_project != ''`, so it is
/// counted here too. That makes the fingerprint *conservative* — it may change
/// (forcing a refresh) for a non-contributing row, but it can never miss a
/// change to a contributing row. `id` is a monotonic AUTOINCREMENT PK, so an
/// insert always advances `MAX(id)` and a delete always changes `COUNT(*)`
/// and/or `MAX(id)`. In-place UPDATEs to a row's rollup-relevant columns would
/// be invisible to this fingerprint, but the ingest path never does them:
/// verified there is no `UPDATE ... logs` anywhere (the scanner re-indexes by
/// `DELETE FROM logs` + re-INSERT, both of which the fingerprint catches).
fn ai_rows_watermark(conn: &rusqlite::Connection) -> rusqlite::Result<(i64, i64)> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM logs
         WHERE ai_project IS NOT NULL AND ai_project != ''",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// Outcome of a conditional rollup refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollupRefresh {
    /// The source fingerprint changed; the rollup was recomputed.
    Refreshed { row_count: usize },
    /// The source fingerprint was unchanged since the last refresh; the
    /// expensive re-aggregation was skipped.
    Skipped,
}

/// Refresh the rollup only if the AI-row partition changed since the last
/// refresh (bead cortex-g33v). The full re-aggregation is a temp-btree
/// `GROUP BY` over the whole AI partition (~4s at scale) and holds the
/// maintenance permit while running; the common case on the background cadence
/// is "nothing changed", so this skips that work via the cheap
/// [`ai_rows_watermark`] fingerprint.
///
/// The skip is correct because [`refresh_ai_session_rollup`] stamps the exact
/// fingerprint of the data it aggregated; if the live fingerprint still matches
/// and we have refreshed at least once, the materialization is already current.
pub fn refresh_ai_session_rollup_if_stale(pool: &DbPool) -> Result<RollupRefresh> {
    {
        let conn = pool.get()?;
        let (cur_count, cur_max_id) = ai_rows_watermark(&conn)?;
        let stored: Option<(Option<String>, i64, i64)> = conn
            .query_row(
                "SELECT refreshed_at, source_row_count, source_max_id
                 FROM ai_session_rollup_meta WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((Some(_refreshed_at), src_count, src_max_id)) = stored
            && src_count == cur_count
            && src_max_id == cur_max_id
        {
            return Ok(RollupRefresh::Skipped);
        }
    }
    let row_count = refresh_ai_session_rollup(pool)?;
    Ok(RollupRefresh::Refreshed { row_count })
}

/// Recompute the `ai_session_rollup` materialization from `logs` using a
/// **staging + atomic swap** strategy, then stamp `refreshed_at` and the source
/// watermark. Returns the number of session rows. This is the
/// unconditional/force path; the background task uses
/// [`refresh_ai_session_rollup_if_stale`] to skip no-op refreshes.
///
/// This is a FULL recompute (not incremental) and stays correct under retention
/// DELETEs: AI rows ingest at `info` and get NO severity exemption from the
/// purge paths (maintenance.rs), so they ARE deleted out from under the rollup.
/// A watermark-incremental refresh would corrupt `MIN(first_seen)`, leave ghost
/// rollup rows for fully-purged sessions, and drift `event_count` — MIN/MAX are
/// non-self-maintainable aggregates. See the Migration 21 note in `pool.rs` and
/// bead syslog-mcp-rvcz for the full rationale.
///
/// ## Staging + swap (writer-starvation fix, bead syslog-mcp-rvcz)
/// The full `GROUP BY` over the AI partition costs ~4s at scale. Previously it
/// ran inside the `IMMEDIATE` write transaction, holding the single WAL writer
/// slot for that whole window and starving the ingest writer (dropped inserts)
/// and bloating the WAL. We now split it:
///   1. **Build** the full aggregation into a connection-local TEMP staging
///      table under a READ snapshot — WAL readers do NOT block the writer, so
///      this holds ZERO write lock for the entire ~4s.
///   2. **Swap** under a sub-millisecond `IMMEDIATE` transaction:
///      `DELETE` + `INSERT ... SELECT * FROM staging` + stamp meta + `COMMIT`.
///
/// ### INVARIANT — the build and the swap MUST use the SAME `Connection`.
/// The staging table is a `TEMP` table, which is **connection-local**: it is
/// only visible to the rusqlite `Connection` that created it. This function
/// deliberately holds ONE `conn` (from a single `pool.get()`) across both
/// phases. A future refactor that splits the build and swap into helpers that
/// each call `pool.get()` would silently produce an EMPTY staging table and
/// wipe the rollup (data-loss regression). DO NOT split the connection. The
/// `assert`/guard before the swap (staging row count == built row count) exists
/// to catch exactly that mistake at runtime.
pub fn refresh_ai_session_rollup(pool: &DbPool) -> Result<usize> {
    // ONE connection for BOTH phases — the TEMP staging table is
    // connection-local (see the INVARIANT in the doc comment above).
    let mut conn = pool.get()?;

    // --- Phase 1: BUILD under a read snapshot (no write lock held) ---------
    // A DEFERRED transaction takes a WAL read snapshot on its first read and
    // never upgrades to a writer here (we only CREATE TEMP + SELECT), so it
    // does not contend for the single WAL writer slot. The watermark and the
    // GROUP BY both read from this one consistent snapshot, so the stored
    // fingerprint exactly describes the data we aggregate.
    let (src_count, src_max_id, staged, rollup_eligible) = {
        let build = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        let (src_count, src_max_id) = ai_rows_watermark(&build)?;
        // TEMP table: connection-local, spills to the temp store (/tmp), never
        // to /data. Rebuilt every refresh, so drop any stale prior copy.
        build.execute("DROP TABLE IF EXISTS _ai_rollup_staging", [])?;
        build.execute(
            "CREATE TEMP TABLE _ai_rollup_staging AS
             WITH eligible AS MATERIALIZED (
               SELECT id, ai_project, ai_tool, ai_session_id, hostname,
                      ai_transcript_path, timestamp,
                      COALESCE(json_extract(metadata_json, '$.session.title'),
                               json_extract(metadata_json, '$.source.title')) AS title,
                      COALESCE(json_extract(metadata_json, '$.session.title_provenance'),
                               json_extract(metadata_json, '$.source.title_provenance')) AS title_provenance
                 FROM logs
                WHERE ai_project IS NOT NULL AND ai_project != ''
                  AND ai_tool IS NOT NULL AND ai_tool != ''
                  AND ai_session_id IS NOT NULL AND ai_session_id != ''
             ),
             latest_titles AS (
               SELECT ai_project, ai_tool, ai_session_id, hostname, title, title_provenance
                 FROM (
                   SELECT ai_project, ai_tool, ai_session_id, hostname, title, title_provenance,
                          ROW_NUMBER() OVER (
                            PARTITION BY ai_project, ai_tool, ai_session_id, hostname
                            ORDER BY id DESC
                          ) AS title_rank
                     FROM eligible WHERE title IS NOT NULL
                 ) WHERE title_rank = 1
             )
             SELECT e.ai_project, e.ai_tool, e.ai_session_id, e.hostname,
                    MIN(e.ai_transcript_path) AS ai_transcript_path,
                    MIN(e.timestamp) AS first_seen,
                    MAX(e.timestamp) AS last_seen,
                    COUNT(*) AS event_count,
                    t.title, t.title_provenance
               FROM eligible e
               LEFT JOIN latest_titles t
                 ON t.ai_project = e.ai_project AND t.ai_tool = e.ai_tool
                AND t.ai_session_id = e.ai_session_id AND t.hostname = e.hostname
              GROUP BY e.ai_project, e.ai_tool, e.ai_session_id, e.hostname,
                       t.title, t.title_provenance",
            [],
        )?;
        let staged: i64 =
            build.query_row("SELECT COUNT(*) FROM _ai_rollup_staging", [], |r| r.get(0))?;
        // Rollup-eligible row count under the SAME read snapshot, using the
        // EXACT predicate as the staging INSERT above. Must be computed inside
        // this transaction (not after commit / on a fresh connection): under
        // one snapshot, any row matching this predicate yields >=1 GROUP BY
        // group, so `staged == 0` IMPLIES `rollup_eligible == 0`. The R1 guard
        // below relies on that mutual consistency; counting under a different
        // snapshot would let a concurrent INSERT revive a false positive.
        let rollup_eligible: i64 = build.query_row(
            "SELECT COUNT(*) FROM logs
             WHERE ai_project IS NOT NULL AND ai_project != ''
               AND ai_tool IS NOT NULL AND ai_tool != ''
               AND ai_session_id IS NOT NULL AND ai_session_id != ''",
            [],
            |r| r.get(0),
        )?;
        // Commit the read snapshot (releases the read lock). The TEMP table
        // survives the commit — it is tied to the connection, not the txn.
        build.commit()?;
        (src_count, src_max_id, staged, rollup_eligible)
    };

    // R1 guardrail (bead syslog-mcp-rvcz security addendum): the same-connection
    // requirement is NOT compile-time enforceable. If a refactor ever ran the
    // build on a different pooled connection, the TEMP table would be invisible
    // here and the swap would wipe the rollup. We must distinguish that
    // regression from a LEGITIMATELY empty rollup: rows can have `ai_project`
    // set but no recognized `ai_tool`/`ai_session_id` (e.g. OTLP logs carrying
    // only project.path), which the watermark counts (`src_count > 0`) but the
    // rollup GROUP BY correctly excludes (`staged == 0`). Comparing against
    // `src_count` would error forever on that data shape. Instead, only bail
    // when staging is empty AND rows matching the FULL rollup predicate exist —
    // i.e. the build genuinely produced groups but the TEMP table is invisible.
    debug_assert!(staged >= 0, "staging row count must be non-negative");
    if staged == 0 && rollup_eligible > 0 {
        return Err(anyhow::anyhow!(
            "ai_session_rollup staging table is empty despite {rollup_eligible} \
             rollup-eligible AI rows present — the build and swap MUST share one \
             Connection (TEMP tables are connection-local); refusing to wipe the \
             rollup"
        ));
    }

    // --- Phase 2: SWAP under a sub-millisecond IMMEDIATE write lock ---------
    // IMMEDIATE (not DEFERRED): take the write lock up front. We read nothing
    // before the DELETE here, but IMMEDIATE keeps the swap a single short
    // writer that never risks an SQLITE_BUSY_SNAPSHOT upgrade failure (which
    // busy_timeout does NOT retry). The GROUP BY is already done, so this lock
    // is held only for the DELETE + INSERT-from-staging + meta UPDATE.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM ai_session_rollup", [])?;
    tx.execute(
        "INSERT INTO ai_session_rollup
             (ai_project, ai_tool, ai_session_id, hostname,
              ai_transcript_path, first_seen, last_seen, event_count,
              title, title_provenance)
         SELECT ai_project, ai_tool, ai_session_id, hostname,
                ai_transcript_path, first_seen, last_seen, event_count,
                title, title_provenance
         FROM _ai_rollup_staging",
        [],
    )?;
    // The staged count is already known from Phase 1 — use it directly rather than
    // running a post-INSERT COUNT(*) inside the IMMEDIATE transaction, which
    // unnecessarily extends write-lock hold time.
    let row_count = staged;
    tx.execute(
        "UPDATE ai_session_rollup_meta
            SET refreshed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                row_count = ?1,
                source_row_count = ?2,
                source_max_id = ?3
          WHERE id = 1",
        params![row_count, src_count, src_max_id],
    )?;
    tx.commit()?;
    // Drop the TEMP table so a long-lived pooled connection doesn't carry it
    // back into the pool. Best-effort: a failure here doesn't affect the
    // already-committed swap.
    let _ = conn.execute("DROP TABLE IF EXISTS _ai_rollup_staging", []);
    Ok(row_count as usize)
}

// -----------------------------------------------------------------------------
// timeline_hourly rollup (bead syslog-mcp-kcvq)
// -----------------------------------------------------------------------------

/// SQLite `strftime` pattern bucketing a `timestamp` into the hour grain stored
/// in `timeline_hourly.bucket`. Kept in one place so the backfill (pool.rs), the
/// incremental refresh, and any future caller stay byte-identical.
pub const TIMELINE_HOUR_FMT: &str = "%Y-%m-%dT%H:00:00Z";

/// Staleness/coverage snapshot for the `timeline_hourly` rollup.
#[derive(Debug, Clone)]
pub struct TimelineRollupStatus {
    /// RFC 3339 timestamp of the last successful incremental refresh, or `None`.
    pub refreshed_at: Option<String>,
    /// Highest `logs.id` aggregated into the rollup so far. Reads add the live
    /// delta `WHERE id > source_max_id` on top of the rollup for fresh totals.
    /// Exposed for diagnostics/tests; the read paths query the meta row directly.
    #[allow(dead_code)]
    pub source_max_id: i64,
}

/// Read the timeline rollup metadata (cheap single-row lookup).
pub fn timeline_rollup_status(pool: &DbPool) -> Result<TimelineRollupStatus> {
    let conn = pool.get()?;
    let (refreshed_at, source_max_id) = conn
        .query_row(
            "SELECT refreshed_at, source_max_id FROM timeline_hourly_meta WHERE id = 1",
            [],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
        .unwrap_or((None, 0));
    Ok(TimelineRollupStatus {
        refreshed_at,
        source_max_id,
    })
}

/// Incrementally fold new `logs` rows into `timeline_hourly`.
///
/// Aggregates ONLY `logs WHERE id > source_max_id AND id <= MAX(id)` and
/// upsert-ADDS into the per-hour buckets, then advances the watermark. This is
/// self-maintainable for adds because the rollup holds only `COUNT(*)` (no
/// MIN/MAX): a new high-id row with an old timestamp correctly adds to its old
/// bucket. The `id <= new_max` upper bound is captured inside the same IMMEDIATE
/// transaction as the aggregate, so a row inserted mid-refresh is neither
/// double-counted now nor skipped next tick.
///
/// `app_name` is normalized to `COALESCE(app_name,'')` to match the NOT NULL PK
/// column — without this, null-app rows would never hit the ON CONFLICT path and
/// would duplicate every tick.
///
/// Per cadence this touches only the rows ingested since the last tick
/// (milliseconds), unlike the AI rollup's full re-aggregation, so a single short
/// IMMEDIATE write is correct and simpler than the staging+swap dance.
///
/// Returns the number of source `logs` rows folded in this tick (0 when the
/// watermark was already current — the common idle-tick case).
pub fn refresh_timeline_rollup(pool: &DbPool) -> Result<usize> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old_max: i64 = tx.query_row(
        "SELECT source_max_id FROM timeline_hourly_meta WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    let new_max: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) FROM logs", [], |r| r.get(0))?;
    if new_max <= old_max {
        // Watermark already current; nothing new to fold. (Deletes are handled
        // out-of-band by the retention prune, not here.)
        tx.commit()?;
        return Ok(0);
    }
    let folded: i64 = tx.query_row(
        "SELECT COUNT(*) FROM logs WHERE id > ?1 AND id <= ?2",
        params![old_max, new_max],
        |r| r.get(0),
    )?;
    tx.execute(
        &format!(
            "INSERT INTO timeline_hourly (bucket, hostname, app_name, severity, event_count)
             SELECT strftime('{TIMELINE_HOUR_FMT}', timestamp) AS bucket,
                    hostname,
                    COALESCE(app_name, '') AS app_name,
                    severity,
                    COUNT(*) AS event_count
             FROM logs
             WHERE id > ?1 AND id <= ?2
             GROUP BY bucket, hostname, app_name, severity
             ON CONFLICT(bucket, hostname, app_name, severity)
                 DO UPDATE SET event_count = event_count + excluded.event_count"
        ),
        params![old_max, new_max],
    )?;
    tx.execute(
        "UPDATE timeline_hourly_meta
            SET refreshed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                source_max_id = ?1
          WHERE id = 1",
        [new_max],
    )?;
    tx.commit()?;
    Ok(folded as usize)
}

/// Prune `timeline_hourly` buckets that are entirely older than the oldest
/// remaining `logs` row, called after a retention purge (which deletes oldest
/// rows by `received_at`). Removes ghost buckets so `timeline`/`stats` totals do
/// not drift upward unbounded on hosts whose ingest watermark is idle while
/// retention keeps purging.
///
/// A minor transient overcount can remain in the single boundary hour (the hour
/// straddling the purge cutoff keeps its full pre-purge count until that hour
/// itself ages out) — accepted as negligible for a volume chart.
///
/// Returns the number of rollup rows deleted.
pub fn prune_timeline_rollup(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    // Fetch MIN(timestamp) as a plain string and apply strftime formatting in Rust
    // so SQLite can use the B-tree MIN optimization (a single leaf seek) rather than
    // scanning the full index when strftime() wraps the MIN expression.
    let oldest_ts: Option<String> = conn.query_row("SELECT MIN(timestamp) FROM logs", [], |r| {
        r.get::<_, Option<String>>(0)
    })?;
    let Some(oldest_ts) = oldest_ts else {
        // No logs at all — clear the whole rollup so it can't ghost.
        let n = conn.execute("DELETE FROM timeline_hourly", [])?;
        return Ok(n);
    };
    // Truncate to the hour bucket format used by the rollup (e.g. "2024-01-15T14:00:00Z").
    // Timestamps are RFC 3339 / ISO 8601 strings with at least 13 chars ("YYYY-MM-DDTHH").
    let oldest_bucket = if oldest_ts.len() >= 13 {
        format!("{}:00:00Z", &oldest_ts[..13])
    } else {
        oldest_ts
    };
    let n = conn.execute(
        "DELETE FROM timeline_hourly WHERE bucket < ?1",
        [oldest_bucket],
    )?;
    Ok(n)
}

pub fn search_ai_sessions(
    pool: &DbPool,
    params: &SearchAiSessionsParams,
) -> Result<SearchAiSessionsResult> {
    validate_fts_query(&params.query)?;

    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let conn = pool.get()?;
    let (sql, bindings) = search_ai_sessions_sql(params, limit);

    let mut stmt = conn.prepare(&sql)?;
    let mut total_candidates = 0usize;
    let mut raw_candidate_count = 0usize;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
        total_candidates = row.get::<_, i64>(11)? as usize;
        raw_candidate_count = row.get::<_, i64>(12)? as usize;
        Ok(SearchedAiSessionEntry {
            ai_project: row.get(0)?,
            ai_tool: row.get(1)?,
            ai_session_id: row.get(2)?,
            hostname: row.get(3)?,
            first_seen: row.get(4)?,
            last_seen: row.get(5)?,
            event_count: row.get(6)?,
            match_count: row.get(7)?,
            best_snippet: row.get(8)?,
            title: row.get(9)?,
            title_provenance: row.get(10)?,
        })
    })?;
    let sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(SearchAiSessionsResult {
        total_candidates,
        candidate_rows: raw_candidate_count.min(CANDIDATE_CAP),
        candidate_cap: CANDIDATE_CAP,
        candidate_window_truncated: raw_candidate_count > CANDIDATE_CAP,
        truncated: total_candidates > sessions.len() || raw_candidate_count > CANDIDATE_CAP,
        sessions,
    })
}

const CANDIDATE_CAP: usize = 5_000;

fn search_ai_sessions_sql(
    params: &SearchAiSessionsParams,
    limit: usize,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut filters = String::new();
    push_required_ai_filters(&mut filters, "l");
    let mut query_params = SqlParams::new(2);
    query_params
        .bindings
        .push(rusqlite::types::Value::Text(params.query.clone()));
    push_ai_scope_filters(
        &mut filters,
        &mut query_params,
        "l",
        &params.ai_project,
        &params.ai_tool,
        &params.since,
        &params.until,
    );
    if let Some(hostname) = &params.host {
        let idx = query_params.push_text(hostname.clone());
        filters.push_str(&format!(" AND l.hostname = ?{idx}"));
    }
    if let Some(app_name) = &params.app {
        let idx = query_params.push_text(app_name.clone());
        filters.push_str(&format!(" AND l.app_name = ?{idx}"));
    }
    let sql = format!(
        "WITH candidates AS MATERIALIZED (
            SELECT l.ai_project,
                   l.ai_tool,
                   l.ai_session_id,
                   l.hostname,
                   l.timestamp,
                   l.message
            FROM ai_logs_fts
            JOIN logs l ON l.id = ai_logs_fts.rowid
            WHERE ai_logs_fts MATCH ?1{filters}
            ORDER BY ai_logs_fts.rowid DESC
            LIMIT {}
         ),
         bounded_candidates AS MATERIALIZED (
            SELECT * FROM candidates
            LIMIT {CANDIDATE_CAP}
         ),
         grouped AS MATERIALIZED (
            SELECT ai_project,
                   ai_tool,
                   ai_session_id,
                   hostname,
                   COUNT(*) AS match_count,
                   MIN(timestamp) AS first_match,
                   MAX(timestamp) AS latest_match
            FROM bounded_candidates c
            GROUP BY ai_project, ai_tool, ai_session_id, hostname
         ),
         selected AS MATERIALIZED (
            SELECT *
            FROM grouped
            ORDER BY latest_match DESC
            LIMIT {limit}
         ),
         rollup_meta AS MATERIALIZED (
            SELECT COALESCE(MAX(source_max_id), 0) AS source_max_id
            FROM ai_session_rollup_meta
         ),
         tail_stats AS MATERIALIZED (
            SELECT l.ai_project,
                   l.ai_tool,
                   l.ai_session_id,
                   l.hostname,
                   MIN(l.timestamp) AS first_seen,
                   MAX(l.timestamp) AS last_seen,
                   COUNT(*) AS event_count
            FROM selected g
            CROSS JOIN rollup_meta meta
            JOIN logs l INDEXED BY idx_logs_ai_session_host_time
              ON g.ai_project = l.ai_project
             AND g.ai_tool = l.ai_tool
             AND g.ai_session_id = l.ai_session_id
             AND g.hostname = l.hostname
             AND l.id > meta.source_max_id
            WHERE l.ai_project IS NOT NULL
              AND l.ai_tool IS NOT NULL
              AND l.ai_session_id IS NOT NULL
            GROUP BY l.ai_project, l.ai_tool, l.ai_session_id, l.hostname
         ),
         tail_titles AS MATERIALIZED (
            SELECT ai_project, ai_tool, ai_session_id, hostname, title, title_provenance
              FROM (
                SELECT l.ai_project, l.ai_tool, l.ai_session_id, l.hostname,
                       COALESCE(json_extract(l.metadata_json, '$.session.title'),
                                json_extract(l.metadata_json, '$.source.title')) AS title,
                       COALESCE(json_extract(l.metadata_json, '$.session.title_provenance'),
                                json_extract(l.metadata_json, '$.source.title_provenance')) AS title_provenance,
                       ROW_NUMBER() OVER (
                         PARTITION BY l.ai_project, l.ai_tool, l.ai_session_id, l.hostname
                         ORDER BY l.id DESC
                       ) AS title_rank
                  FROM selected g
                  CROSS JOIN rollup_meta meta
                  JOIN logs l INDEXED BY idx_logs_ai_session_host_time
                    ON g.ai_project = l.ai_project
                   AND g.ai_tool = l.ai_tool
                   AND g.ai_session_id = l.ai_session_id
                   AND g.hostname = l.hostname
                   AND l.id > meta.source_max_id
                 WHERE COALESCE(json_extract(l.metadata_json, '$.session.title'),
                                json_extract(l.metadata_json, '$.source.title')) IS NOT NULL
              ) WHERE title_rank = 1
         ),
         totals AS MATERIALIZED (
            SELECT COUNT(*) AS total_candidates,
                   COALESCE(SUM(match_count), 0) AS raw_candidate_count
            FROM (
                SELECT COUNT(*) AS match_count
                FROM candidates
                GROUP BY ai_project, ai_tool, ai_session_id, hostname
            ) filtered_sessions
         )
         SELECT g.ai_project, g.ai_tool, g.ai_session_id, g.hostname,
                COALESCE(CASE
                    WHEN rollup.first_seen IS NULL THEN tail.first_seen
                    WHEN tail.first_seen IS NULL THEN rollup.first_seen
                    ELSE MIN(rollup.first_seen, tail.first_seen)
                END, g.first_match) AS first_seen,
                COALESCE(CASE
                    WHEN rollup.last_seen IS NULL THEN tail.last_seen
                    WHEN tail.last_seen IS NULL THEN rollup.last_seen
                    ELSE MAX(rollup.last_seen, tail.last_seen)
                END, g.latest_match) AS last_seen,
                CASE
                    WHEN rollup.event_count IS NULL AND tail.event_count IS NULL
                        THEN g.match_count
                    ELSE COALESCE(rollup.event_count, 0) + COALESCE(tail.event_count, 0)
                END AS event_count,
                g.match_count,
                (
                    SELECT c2.message
                    FROM bounded_candidates c2
                    WHERE c2.ai_project = g.ai_project
                      AND c2.ai_tool = g.ai_tool
                      AND c2.ai_session_id = g.ai_session_id
                      AND c2.hostname = g.hostname
                    ORDER BY c2.timestamp DESC
                    LIMIT 1
                ) AS best_snippet,
                COALESCE(tail_title.title, rollup.title) AS title,
                COALESCE(tail_title.title_provenance, rollup.title_provenance) AS title_provenance,
                totals.total_candidates,
                totals.raw_candidate_count
         FROM selected g
         LEFT JOIN ai_session_rollup rollup
           ON rollup.ai_project = g.ai_project
          AND rollup.ai_tool = g.ai_tool
          AND rollup.ai_session_id = g.ai_session_id
          AND rollup.hostname = g.hostname
         LEFT JOIN tail_stats tail
           ON tail.ai_project = g.ai_project
          AND tail.ai_tool = g.ai_tool
          AND tail.ai_session_id = g.ai_session_id
          AND tail.hostname = g.hostname
         LEFT JOIN tail_titles tail_title
           ON tail_title.ai_project = g.ai_project
          AND tail_title.ai_tool = g.ai_tool
          AND tail_title.ai_session_id = g.ai_session_id
          AND tail_title.hostname = g.hostname
         CROSS JOIN totals
         -- Rank by match recency (latest matching row), not the full-session
         -- last_seen computed above: a session with an old match but newer
         -- non-matching activity must not jump ahead of more recent matches.
         -- This mirrors `selected`'s own `latest_match DESC` pre-selection and
         -- the pre-refactor match-recency ordering.
         ORDER BY g.latest_match DESC
         LIMIT {limit}",
        CANDIDATE_CAP + 1
    );
    (sql, query_params.bindings)
}

pub fn search_ai_anchors(pool: &DbPool, params: &AiCorrelateParams) -> Result<Vec<LogEntry>> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(10).clamp(1, 50);
    let mut bindings: Vec<rusqlite::types::Value> = vec![];
    let mut idx = 1usize;
    let has_query = if let Some(query) = &params.ai_query {
        validate_fts_query(query)?;
        bindings.push(rusqlite::types::Value::Text(query.clone()));
        idx += 1;
        true
    } else {
        false
    };

    // Shared filter clause — applied inside the FTS candidate CTE (so the cap
    // operates on already-filtered rows) or directly on the plain scan.
    let mut filters = String::new();
    push_required_ai_filters(&mut filters, "l");
    if let Some(project) = &params.ai_project {
        filters.push_str(&format!(" AND l.ai_project = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(project.clone()));
        idx += 1;
    }
    if let Some(tool) = &params.ai_tool {
        filters.push_str(&format!(" AND l.ai_tool = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(tool.clone()));
        idx += 1;
    }
    if let Some(session_id) = &params.ai_session_id {
        filters.push_str(&format!(" AND l.ai_session_id = ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(session_id.clone()));
        idx += 1;
    }
    if let Some(from) = &params.since {
        filters.push_str(&format!(" AND l.timestamp >= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(from.clone()));
        idx += 1;
    }
    if let Some(to) = &params.until {
        filters.push_str(&format!(" AND l.timestamp <= ?{idx}"));
        bindings.push(rusqlite::types::Value::Text(to.clone()));
    }

    let sql = if has_query {
        // Capped-candidate plan: the previous `FROM logs_fts JOIN logs ...
        // ORDER BY timestamp` shape fetched and sorted the ENTIRE FTS match
        // set in a temp b-tree before LIMIT applied — the exact pathology
        // `search_logs` fixed with its candidate cap, never applied to this
        // entry point (full-review PM2).
        format!(
            "WITH fts_candidates(id, ts) AS MATERIALIZED (
                SELECT l.id, l.timestamp
                FROM logs_fts
                JOIN logs l ON l.id = logs_fts.rowid
                WHERE logs_fts MATCH ?1{filters}
                ORDER BY logs_fts.rowid DESC LIMIT {SEARCH_FTS_CANDIDATE_CAP}
             )
             SELECT {FTS_SELECT_COLS}
             FROM fts_candidates c
             JOIN logs l ON l.id = c.id
             ORDER BY c.ts DESC, l.id DESC LIMIT {}",
            limit + 1
        )
    } else {
        format!(
            "SELECT {FTS_SELECT_COLS}
             FROM logs l
             WHERE 1=1{filters}
             ORDER BY l.timestamp DESC, l.id DESC LIMIT {}",
            limit + 1
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn search_ai_related_logs(
    pool: &DbPool,
    params: &AiRelatedLogsParams,
) -> Result<Vec<AiRelatedLogsForAnchor>> {
    if params.windows.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(query) = &params.query {
        validate_fts_query(query)?;
    }

    let conn = pool.get()?;
    let limit = params.limit_per_anchor.clamp(1, 200) as usize;

    // Reject duplicate anchors up front (preserved contract).
    {
        let mut seen = std::collections::HashSet::with_capacity(params.windows.len());
        for window in &params.windows {
            if !seen.insert(window.anchor_index) {
                anyhow::bail!(
                    "duplicate anchor_index {} in AiRelatedLogsParams windows",
                    window.anchor_index
                );
            }
        }
    }

    // One bounded, index-served query PER ANCHOR instead of the previous
    // single windowed CTE that ROW_NUMBER()-ranked EVERY log row inside every
    // window before applying the per-anchor limit — SQLite cannot push the
    // rank limit into the window scan, so a 10-minute window during a log
    // storm sorted 100K+ rows per anchor (full-review PM1). Each per-anchor
    // query remains bounded to one anchor window and returns only n+1 rows.
    // Ranking by distance to the anchor makes the result evidentially useful:
    // a busy window no longer discards the rows nearest the AI action merely
    // because unrelated traffic arrived near the end of the window. Anchor
    // counts are small (bounded by the anchor search limit) and the statement
    // is compiled once via prepare_cached.
    //
    // Placeholders: FTS path ?1=query ?2=from ?3=to ?4=anchor, filters from ?5;
    // plain path ?1=from ?2=to ?3=anchor, filters from ?4.
    let first_filter_idx = if params.query.is_some() { 5 } else { 4 };
    let mut filter_sql = String::new();
    let mut sql_params = SqlParams::new(first_filter_idx);
    let search_params = SearchParams {
        query: None,
        host: params.host.clone(),
        source: params.source.clone(),
        source_ip_prefix: None,
        source_ip_prefixes: None,
        severity: None,
        severity_in: Some(params.severity_in.clone()),
        app: params.app.clone(),
        facility: None,
        exclude_facility: None,
        process_id: None,
        since: None,
        until: None,
        received_since: None,
        received_until: None,
        limit: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        event_action: None,
        exclude_ai: true,
    };
    append_filters(
        &mut filter_sql,
        &mut sql_params.bindings,
        &mut sql_params.next_idx,
        &search_params,
    );

    let sql = if params.query.is_some() {
        format!(
            "SELECT {FTS_SELECT_COLS}
             FROM logs_fts
             JOIN logs l ON l.id = logs_fts.rowid
             WHERE logs_fts MATCH ?1
               AND l.timestamp >= ?2 AND l.timestamp <= ?3{filter_sql}
             ORDER BY ABS(unixepoch(l.timestamp) - unixepoch(?4)), l.timestamp DESC, l.id DESC LIMIT {}",
            limit + 1
        )
    } else {
        format!(
            "SELECT {FTS_SELECT_COLS}
             FROM logs l
             WHERE l.timestamp >= ?1 AND l.timestamp <= ?2{filter_sql}
             ORDER BY ABS(unixepoch(l.timestamp) - unixepoch(?3)), l.timestamp DESC, l.id DESC LIMIT {}",
            limit + 1
        )
    };

    let mut grouped = Vec::with_capacity(params.windows.len());
    let mut stmt = conn.prepare_cached(&sql)?;
    for window in &params.windows {
        let mut bindings: Vec<rusqlite::types::Value> =
            Vec::with_capacity(3 + sql_params.bindings.len());
        if let Some(query) = &params.query {
            bindings.push(rusqlite::types::Value::Text(query.clone()));
        }
        bindings.push(rusqlite::types::Value::Text(window.window_from.clone()));
        bindings.push(rusqlite::types::Value::Text(window.window_to.clone()));
        bindings.push(rusqlite::types::Value::Text(window.anchor_time.clone()));
        bindings.extend(sql_params.bindings.iter().cloned());

        let mut logs = Vec::new();
        let mut truncated = false;
        let mut rows = stmt.query(rusqlite::params_from_iter(bindings.iter()))?;
        let mut row_count = 0usize;
        while let Some(row) = rows.next()? {
            row_count += 1;
            if row_count > limit {
                truncated = true;
            } else {
                logs.push(map_row(row)?);
            }
        }
        grouped.push(AiRelatedLogsForAnchor {
            anchor_index: window.anchor_index,
            logs,
            truncated,
        });
    }

    Ok(grouped)
}

/// Push each value as a bound `Text` parameter and return a `?, ?, …`
/// placeholder list of matching arity for an `IN (...)` clause.
pub(super) fn bind_in_list(
    bindings: &mut Vec<rusqlite::types::Value>,
    values: &[String],
) -> String {
    let start = bindings.len();
    for v in values {
        bindings.push(rusqlite::types::Value::Text(v.clone()));
    }
    vec!["?"; bindings.len() - start].join(", ")
}

/// Controls which walked entities may contribute host-wide
/// (`l.hostname IN (…)`) predicates to the graph log fan-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFanoutScope {
    /// Any reached `host`/`container` entity adds its hostname. Session
    /// correlation uses this deliberately: following `ai_session → host`
    /// edges to the host's logs is the point of the query.
    WalkReached,
    /// Only `host` entities that were seeds themselves (exact topic match on
    /// the host) add hostnames. Hosts reached transitively from app/container
    /// seeds never drive host-wide log inclusion — topic correlation's
    /// no-silent-fan-out guarantee.
    SeedHostsOnly,
}

/// Graph-anchored log fan-out: traverse the investigation graph outward from a
/// set of seed entities, then return the logs emitted by every related entity
/// within `[since, until]`.
///
/// This is the **graph-first** query order (Performance Oracle research):
/// resolve ~tens of related entity keys via the indexed recursive CTE, map them
/// to `hostname` / `ai_project` / `ai_session_id` filters, and let the
/// `(hostname, app_name, timestamp)` covering index (migration 32) drive the
/// log scan — 10-100× fewer rows than an FTS-first scan over a common term.
///
/// Entity → log-column mapping:
/// - `host` → `hostname` (subject to `host_fanout_scope`)
/// - `container` (`docker_host:…` keys) → leading `hostname` (only under
///   [`HostFanoutScope::WalkReached`])
/// - `ai_project` → `ai_project`
/// - `ai_session` (`project:tool:session` keys) → trailing `ai_session_id`
///
/// `max_depth` is clamped to `[1, GRAPH_WALK_MAX_DEPTH]` and `limit` to
/// `[1, 1000]`. Returns raw `LogEntry` rows for the service layer to shape.
#[allow(clippy::too_many_arguments)]
pub fn search_logs_from_graph_related_entities(
    pool: &DbPool,
    entity_canonical_keys: &[String],
    max_depth: u8,
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
    limit: usize,
    host_fanout_scope: HostFanoutScope,
) -> Result<Vec<LogEntry>> {
    if entity_canonical_keys.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;

    // 1. Graph-first: traverse to the related entity set (seeds + N hops).
    let entities = super::graph::graph_walk_n_hops(&conn, entity_canonical_keys, max_depth)?;
    if entities.is_empty() {
        return Ok(Vec::new());
    }
    let seed_set: std::collections::HashSet<&str> =
        entity_canonical_keys.iter().map(String::as_str).collect();

    // 2. Map related entities to indexed log-column filters.
    //
    // Hard break (entity_resolution_v2): `service_instance` entities do NOT
    // map to host-wide log filters here — service-scoped logs come from
    // `search_logs_for_service_instances` predicates instead, so a service
    // topic never silently expands to every log on its host.
    let mut hostnames: Vec<String> = Vec::new();
    let mut ai_projects: Vec<String> = Vec::new();
    let mut ai_sessions: Vec<String> = Vec::new();
    for entity in &entities {
        match entity.entity_type.as_str() {
            super::graph::ENTITY_TYPE_HOST => {
                if host_fanout_scope == HostFanoutScope::WalkReached
                    || seed_set.contains(entity.canonical_key.as_str())
                {
                    hostnames.push(entity.canonical_key.clone());
                }
            }
            super::graph::ENTITY_TYPE_CONTAINER
                if host_fanout_scope == HostFanoutScope::WalkReached =>
            {
                // `docker_host:container_id` — the leading segment is the
                // host the workload runs on.
                if let Some(host) =
                    super::entity_resolution::container_key_host(&entity.canonical_key)
                {
                    hostnames.push(host.to_string());
                }
            }
            super::graph::ENTITY_TYPE_AI_PROJECT => ai_projects.push(entity.canonical_key.clone()),
            super::graph::ENTITY_TYPE_AI_SESSION => {
                // `project:tool:session` — the session id is the 3rd segment.
                if let Some(session) = entity.canonical_key.splitn(3, ':').nth(2)
                    && !session.is_empty()
                {
                    ai_sessions.push(session.to_string());
                }
            }
            _ => {}
        }
    }
    hostnames.sort();
    hostnames.dedup();
    ai_projects.sort();
    ai_projects.dedup();
    ai_sessions.sort();
    ai_sessions.dedup();

    if hostnames.is_empty() && ai_projects.is_empty() && ai_sessions.is_empty() {
        return Ok(Vec::new());
    }

    // 3. Build the graph-first log fan-out. hostname IN (...) leads on the
    //    covering index; ai_project / ai_session_id are OR-ed in via their own
    //    partial indexes.
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    let mut entity_clauses: Vec<String> = Vec::new();
    if !hostnames.is_empty() {
        let ph = bind_in_list(&mut bindings, &hostnames);
        entity_clauses.push(format!("l.hostname IN ({ph})"));
    }
    if !ai_projects.is_empty() {
        let ph = bind_in_list(&mut bindings, &ai_projects);
        entity_clauses.push(format!("l.ai_project IN ({ph})"));
    }
    if !ai_sessions.is_empty() {
        let ph = bind_in_list(&mut bindings, &ai_sessions);
        entity_clauses.push(format!("l.ai_session_id IN ({ph})"));
    }

    let mut where_sql = format!("({})", entity_clauses.join(" OR "));
    if let Some(since) = since {
        where_sql.push_str(" AND l.timestamp >= ?");
        bindings.push(rusqlite::types::Value::Text(since.to_string()));
    }
    if let Some(until) = until {
        where_sql.push_str(" AND l.timestamp <= ?");
        bindings.push(rusqlite::types::Value::Text(until.to_string()));
    }
    if let Some(kinds) = source_kinds
        && !kinds.is_empty()
    {
        let kind_strs: Vec<String> = kinds.iter().map(|k| k.as_str().to_string()).collect();
        let ph = bind_in_list(&mut bindings, &kind_strs);
        where_sql.push_str(&format!(
            " AND json_extract(l.metadata_json, '$.source_kind') IN ({ph})"
        ));
    }
    bindings.push(rusqlite::types::Value::Integer(limit as i64));

    let sql = format!(
        "SELECT {FTS_SELECT_COLS}
         FROM logs l
         WHERE {where_sql}
         ORDER BY l.timestamp DESC, l.id DESC
         LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let logs = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(logs)
}

/// Graph-anchored, session-scoped correlation inputs for `ai_correlate`.
///
/// Resolves the session's time bounds from its log rows, finds the `ai_session`
/// graph entity (key ends in `:{session_id}`), traverses the graph (depth 2) to
/// discover related hosts/containers/services, then fans logs out across all
/// source kinds within the session window via
/// `search_logs_from_graph_related_entities`. The fan-out's `ai_session_id`
/// filter pulls in the agent-command lane (Claude's bash calls) and the
/// hostname filter pulls in the shell-history / syslog lanes on the discovered
/// hosts.
///
/// Falls back to a plain `ai_session_id`-filtered query (`used_graph = false`)
/// when the graph has no entity for the session yet. Returns empty bounds when
/// the session has no rows at all.
///
/// Deliberate scoping: session correlation uses host/container mapping only
/// and intentionally does not fan out via `service_instance` predicates.
pub fn correlate_session_graph(
    pool: &DbPool,
    session_id: &str,
    limit: usize,
) -> Result<SessionGraphInputs> {
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;

    // Session window = [MIN, MAX] timestamp over the session's rows.
    let bounds: Option<(String, String)> = conn
        .query_row(
            "SELECT MIN(timestamp), MAX(timestamp) FROM logs WHERE ai_session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?
        .and_then(|(min, max)| match (min, max) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        });

    let Some((start, end)) = bounds else {
        return Ok(SessionGraphInputs::default());
    };

    // Find the ai_session graph entity (canonical_key `project:tool:session`).
    let session_keys: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT canonical_key FROM graph_entities
             WHERE entity_type = ?1 AND canonical_key LIKE '%:' || ?2",
        )?;
        stmt.query_map(
            rusqlite::params![super::graph::ENTITY_TYPE_AI_SESSION, session_id],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let used_graph = !session_keys.is_empty();

    // Discover related entities/hosts by traversing from the session entity.
    let mut discovered_entities: Vec<String> = Vec::new();
    let mut discovered_hosts: Vec<String> = Vec::new();
    if used_graph {
        for entity in super::graph::graph_walk_n_hops(&conn, &session_keys, 2)? {
            match entity.entity_type.as_str() {
                super::graph::ENTITY_TYPE_HOST => {
                    discovered_hosts.push(entity.canonical_key.clone())
                }
                super::graph::ENTITY_TYPE_CONTAINER => {
                    if let Some(host) =
                        super::entity_resolution::container_key_host(&entity.canonical_key)
                    {
                        discovered_hosts.push(host.to_string());
                    }
                }
                super::graph::ENTITY_TYPE_SERVICE_INSTANCE => {
                    if let Some((host, _)) =
                        super::entity_resolution::split_service_instance_key(&entity.canonical_key)
                    {
                        discovered_hosts.push(host.to_string());
                    }
                }
                _ => {}
            }
            discovered_entities.push(entity.canonical_key);
        }
        discovered_hosts.sort();
        discovered_hosts.dedup();
        discovered_entities.sort();
        discovered_entities.dedup();
    }

    // Fan logs out across all source kinds within the session window.
    let logs = if used_graph {
        drop(conn);
        search_logs_from_graph_related_entities(
            pool,
            &session_keys,
            2,
            Some(&start),
            Some(&end),
            None,
            limit,
            HostFanoutScope::WalkReached,
        )?
    } else {
        // Fallback: the graph hasn't projected this session yet — return its own
        // rows (transcript + agent-command lanes) by exact session id.
        let mut stmt = conn.prepare(&format!(
            "SELECT {FTS_SELECT_COLS}
             FROM logs l
             WHERE l.ai_session_id = ?1
             ORDER BY l.timestamp DESC, l.id DESC
             LIMIT ?2"
        ))?;
        stmt.query_map(rusqlite::params![session_id, limit as i64], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(SessionGraphInputs {
        bounds: Some((start, end)),
        session_entity_keys: session_keys,
        discovered_hosts,
        discovered_entities,
        used_graph,
        logs,
    })
}

/// Topic-anchored universal correlation: resolve a set of (lowercased) topic
/// terms to graph entities, expand the graph `max_depth` hops, and fan logs out
/// across all source kinds within `[since, until]` via
/// `search_logs_from_graph_related_entities` (graph-first order).
///
/// Returns the resolved seed entities, the graph expansion (reached entities
/// that were not seeds), the discovered hosts, and the correlated logs. Empty
/// when no term resolves to an entity.
#[allow(clippy::too_many_arguments)]
pub fn topic_correlate_inputs(
    pool: &DbPool,
    terms: &[String],
    max_depth: u8,
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
    limit: usize,
) -> Result<TopicGraphInputs> {
    if terms.is_empty() {
        return Ok(TopicGraphInputs::default());
    }
    let conn = pool.get()?;
    let mut resolved = queries_service_instances::resolve_topic_entities(&conn, terms)?;
    if resolved.is_empty() {
        drop(conn);
        return topic_correlate_ai_project_fallback(pool, terms, since, until, source_kinds, limit);
    }

    // Partition seeds. Only `resolved` (exact/alias) identities drive log
    // fan-out; weak prefix/label candidates stay visible as `ambiguous`.
    // Service identity seeds resolve through service-instance predicates:
    // an exact `logical_service` expands to its `instance_of` instances.
    let mut instance_keys: Vec<String> = Vec::new();
    let mut logical_keys: Vec<String> = Vec::new();
    let mut generic_seeds: Vec<String> = Vec::new();
    for entity in &resolved {
        if entity.resolver_status != ResolverStatus::Resolved {
            continue;
        }
        match entity.entity_type.as_str() {
            super::graph::ENTITY_TYPE_LOGICAL_SERVICE => {
                logical_keys.push(entity.canonical_key.clone())
            }
            super::graph::ENTITY_TYPE_SERVICE_INSTANCE => {
                instance_keys.push(entity.canonical_key.clone())
            }
            _ => generic_seeds.push(entity.canonical_key.clone()),
        }
    }
    if !logical_keys.is_empty() {
        let linked =
            queries_service_instances::service_instances_of_logical_services(&conn, &logical_keys)?;
        // A resolved logical service with ZERO instance_of instances means
        // the projection is stale or unbuilt (e.g. right after migration 41
        // before `cortex graph rebuild`). Mark it degraded so the empty
        // service timeline is explained rather than silent. Canonical
        // instance keys are `host/<logical_key>` by the resolver grammar.
        let covered: std::collections::HashSet<String> = linked
            .iter()
            .filter_map(|key| {
                super::entity_resolution::split_service_instance_key(key)
                    .map(|(_, service)| service.to_string())
            })
            .collect();
        for entity in resolved.iter_mut() {
            if entity.entity_type == super::graph::ENTITY_TYPE_LOGICAL_SERVICE
                && entity.resolver_status == ResolverStatus::Resolved
                && !covered.contains(entity.canonical_key.as_str())
            {
                entity.resolver_status = ResolverStatus::Degraded;
            }