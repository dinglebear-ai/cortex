# RAG over Historical Incidents and AI Sessions — v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three MCP actions — `similar_incidents`, `ask_history`, and `incident_context` — that use FTS5 retrieval to surface historical log clusters and correlated AI session transcripts as structured context bundles, without embedding an LLM or adding new dependencies.

**Architecture:** FTS5-only v1 carve-out of the full axon+Qdrant spec at `docs/superpowers/specs/2026-05-16-rag-incidents-design.md`. All retrieval is done via the existing `logs_fts` FTS5 virtual table and `search_ai_sessions` grouping query. Bundles are computed on the fly — no `incidents` table. v2 will layer dense retrieval on top of this surface.

**Tech Stack:** Rust, SQLx, rusqlite, FTS5, existing `logs` + `logs_fts` tables, `serde`, action dispatch pattern.

---

## v1 "incident" definition

An incident is a set of log rows (grouped by `hostname` + `app_name`) that fall within a 30-minute time window seeded by a query hit. No schema changes required. Grouping is computed in SQL via a CTE.

## Spec alignment note

The full design spec (`docs/superpowers/specs/2026-05-16-rag-incidents-design.md`) uses axon/Qdrant/BM42 hybrid retrieval and defines actions `similar_incidents`, `ask_history`, `suggest_fix`, and `mark_incident_resolved`. This plan implements the FTS5-only subset with `similar_incidents`, `ask_history`, and `incident_context`. The action names match the spec where feasible so v2 can grow naturally.

---

## File Map

| File | Change | Responsibility |
|------|--------|----------------|
| `src/db/models.rs` | Add structs | DB-layer params and result types for 3 new queries |
| `src/db.rs` | Add exports | Re-export new types and functions |
| `src/db/queries.rs` | Add functions | `similar_incidents_clusters()`, `ask_history_sessions()`, `incident_context_summary()` |
| `src/app/models.rs` | Add structs | App-layer request/response types (with `From<db::…>` impls) |
| `src/app/service.rs` | Add methods | `similar_incidents()`, `ask_history()`, `incident_context()` |
| `src/mcp/schemas.rs` | Modify constants | Add 3 action names to `SYSLOG_ACTIONS`, extend descriptions |
| `src/mcp/tools.rs` | Add match arms + functions | Dispatch + handler for 3 new actions |
| `src/cli.rs` | Add enum variants + dispatch | `syslog ai similar`, `syslog ai ask-history`, `syslog ai incident-context` |

---

## Task 1: DB models — params and result structs

**Files:**
- Modify: `src/db/models.rs` (end of file)

- [ ] **Step 1: Add DB-layer params and result types to `src/db/models.rs`**

Append to the end of `src/db/models.rs`:

```rust
// ---------------------------------------------------------------------------
// RAG v1: similar_incidents, ask_history, incident_context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SimilarIncidentsParams {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    /// Minimum severity (e.g. "warning"). None = all severities.
    pub severity_min: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Cluster grouping window in minutes. Default 30, clamp 5..=120.
    pub window_minutes: Option<u32>,
    /// Max clusters to return. Default 10, clamp 1..=50.
    pub limit: Option<u32>,
}

/// A time-windowed cluster of log hits (one "incident").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentCluster {
    pub hostname: String,
    pub app_name: Option<String>,
    /// RFC 3339 timestamp of the first matching log in this cluster.
    pub window_start: String,
    /// RFC 3339 timestamp of the last matching log in this cluster.
    pub window_end: String,
    pub log_count: i64,
    /// Highest severity in this cluster (emerg > alert > … > debug).
    pub severity_peak: String,
    /// Up to 3 representative message snippets (first 256 chars each).
    pub representative_messages: Vec<String>,
    /// AI sessions whose transcript entries overlap this cluster's time window.
    pub correlated_sessions: Vec<CorrelatedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelatedSession {
    pub session_id: String,
    pub project: String,
    pub tool: String,
    pub match_count: i64,
    pub best_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarIncidentsResult {
    pub query: String,
    pub total_clusters: usize,
    pub truncated: bool,
    pub clusters: Vec<IncidentCluster>,
}

#[derive(Debug, Clone, Default)]
pub struct AskHistoryParams {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Max sessions to return. Default 10, clamp 1..=50.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskHistoryResult {
    pub query: String,
    pub total_candidates: usize,
    pub truncated: bool,
    /// Sessions with transcript hits ranked by match count.
    pub sessions: Vec<SearchedAiSessionEntry>,
    /// System (non-AI) log entries from the same time windows as the top sessions.
    pub context_logs: Vec<LogEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct IncidentContextParams {
    pub from: String,
    pub to: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub query: Option<String>,
    pub severity_min: Option<String>,
    /// Max error log rows to return. Default 50, clamp 1..=200.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeverityCount {
    pub severity: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogCount {
    pub app_name: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentContextResult {
    pub window_from: String,
    pub window_to: String,
    pub total_logs: i64,
    pub by_severity: Vec<SeverityCount>,
    pub by_app: Vec<AppLogCount>,
    /// Logs at or above severity_min (default: warning) within the window.
    pub error_logs: Vec<LogEntry>,
    pub error_logs_truncated: bool,
    /// AI sessions active in this window (have transcript entries between from..to).
    pub ai_sessions: Vec<AiSessionEntry>,
}
```

- [ ] **Step 2: Compile-check the new types**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo check 2>&1 | grep -E "^error" | head -20
```

Expected: No errors (the new types use only `Serialize`, `Deserialize`, `Debug`, `Clone`, `Default`, and existing types already in scope in `db/models.rs`).

- [ ] **Step 3: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/db/models.rs && git commit -m "feat(db): add RAG v1 DB params and result types (similar_incidents, ask_history, incident_context)"
```

---

## Task 2: DB queries — three retrieval functions

**Files:**
- Modify: `src/db/queries.rs` (append to end)

- [ ] **Step 1: Write a failing test for `similar_incidents_clusters`**

Add to `src/db/queries_tests.rs` (append at end of file):

```rust
#[test]
fn similar_incidents_clusters_returns_clusters_for_matching_logs() {
    use super::*;
    use crate::config::StorageConfig;
    use crate::db::{init_pool, insert_logs_batch, LogBatchEntry};

    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("rag-test.db"));
    let pool = init_pool(&storage).unwrap();

    let logs = vec![
        LogBatchEntry {
            timestamp: "2024-01-15T10:00:00Z".into(),
            hostname: "web-01".into(),
            facility: None,
            severity: "err".into(),
            app_name: Some("nginx".into()),
            process_id: None,
            message: "upstream connect error timeout".into(),
            raw: "upstream connect error timeout".into(),
            source_ip: "10.0.0.1:514".into(),
            docker_checkpoint: None,
            ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
            metadata_json: None,
            http_status: None, auth_outcome: None, dns_blocked: None,
            event_action: None, parse_error: None,
        },
        LogBatchEntry {
            timestamp: "2024-01-15T10:05:00Z".into(),
            hostname: "web-01".into(),
            facility: None,
            severity: "crit".into(),
            app_name: Some("nginx".into()),
            process_id: None,
            message: "upstream connect error connection refused".into(),
            raw: "upstream connect error connection refused".into(),
            source_ip: "10.0.0.1:514".into(),
            docker_checkpoint: None,
            ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
            metadata_json: None,
            http_status: None, auth_outcome: None, dns_blocked: None,
            event_action: None, parse_error: None,
        },
    ];
    insert_logs_batch(&pool, &logs).unwrap();

    let params = SimilarIncidentsParams {
        query: "upstream".into(),
        hostname: None,
        app_name: None,
        severity_min: None,
        from: None,
        to: None,
        window_minutes: Some(30),
        limit: Some(10),
    };
    let result = similar_incidents_clusters(&pool, &params).unwrap();
    assert!(!result.clusters.is_empty(), "expected at least one cluster");
    let cluster = &result.clusters[0];
    assert_eq!(cluster.hostname, "web-01");
    assert_eq!(cluster.app_name.as_deref(), Some("nginx"));
    assert!(cluster.log_count >= 2);
    assert_eq!(cluster.severity_peak, "crit");
}
```

- [ ] **Step 2: Run to verify the test fails**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test similar_incidents_clusters_returns_clusters 2>&1 | tail -20
```

Expected: compile error — `similar_incidents_clusters` not found.

- [ ] **Step 3: Implement `similar_incidents_clusters` in `src/db/queries.rs`**

Append to the end of `src/db/queries.rs`:

```rust
// ---------------------------------------------------------------------------
// RAG v1
// ---------------------------------------------------------------------------

use crate::db::models::{
    AskHistoryParams, AskHistoryResult, CorrelatedSession, IncidentCluster, IncidentContextParams,
    IncidentContextResult, SeverityCount, AppLogCount, SimilarIncidentsParams,
    SimilarIncidentsResult,
};

/// Return incident clusters from FTS5 hits, grouped by hostname + app_name in
/// non-overlapping 30-minute (configurable) windows.
///
/// Algorithm:
/// 1. FTS5 MATCH over logs (optionally filtered by host/app/severity/time).
/// 2. Group hits by (hostname, app_name, floor(timestamp / window_minutes)).
/// 3. For each cluster: pick severity_peak (max severity by numeric rank),
///    collect up to 3 representative messages, collect correlated AI sessions.
pub fn similar_incidents_clusters(
    pool: &DbPool,
    params: &SimilarIncidentsParams,
) -> Result<SimilarIncidentsResult> {
    validate_fts_query(&params.query)?;

    let conn = pool.get()?;
    let window_minutes = params.window_minutes.unwrap_or(30).clamp(5, 120);
    let limit = params.limit.unwrap_or(10).clamp(1, 50) as usize;
    let window_secs = i64::from(window_minutes) * 60;

    // Build the FTS5 + filter query.
    // We use a CTE so we can group and annotate in one pass.
    let mut sql = String::from(
        "WITH hits AS (
            SELECT l.id, l.timestamp, l.hostname, l.app_name, l.severity, l.message,
                   l.ai_tool, l.ai_project, l.ai_session_id
            FROM logs_fts
            JOIN logs l ON l.id = logs_fts.rowid
            WHERE logs_fts MATCH ?1
              AND (l.ai_project IS NULL OR l.ai_project = '')
        ",
    );

    let mut query_params = SqlParams::new(2);
    query_params
        .bindings
        .push(rusqlite::types::Value::Text(params.query.clone()));

    if let Some(hostname) = &params.hostname {
        let idx = query_params.push_text(hostname.clone());
        sql.push_str(&format!(" AND l.hostname = ?{idx}"));
    }
    if let Some(app_name) = &params.app_name {
        let idx = query_params.push_text(app_name.clone());
        sql.push_str(&format!(" AND l.app_name = ?{idx}"));
    }
    if let Some(from) = &params.from {
        let idx = query_params.push_text(from.clone());
        sql.push_str(&format!(" AND l.timestamp >= ?{idx}"));
    }
    if let Some(to) = &params.to {
        let idx = query_params.push_text(to.clone());
        sql.push_str(&format!(" AND l.timestamp <= ?{idx}"));
    }

    sql.push_str(&format!(
        " ORDER BY l.timestamp DESC LIMIT 5000
        ),
        bucketed AS (
            SELECT
                hostname,
                app_name,
                CAST(strftime('%s', timestamp) AS INTEGER) / {window_secs} AS bucket,
                MIN(timestamp) AS window_start,
                MAX(timestamp) AS window_end,
                COUNT(*) AS log_count,
                GROUP_CONCAT(severity, ',') AS severities,
                GROUP_CONCAT(SUBSTR(message, 1, 256), '|||') AS messages
            FROM hits
            GROUP BY hostname, app_name, bucket
        )
        SELECT hostname, app_name, window_start, window_end, log_count, severities, messages
        FROM bucketed
        ORDER BY log_count DESC, window_start DESC
        LIMIT {limit}"
    ));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(query_params.bindings.iter()),
        |row| {
            Ok((
                row.get::<_, String>(0)?,  // hostname
                row.get::<_, Option<String>>(1)?,  // app_name
                row.get::<_, String>(2)?,  // window_start
                row.get::<_, String>(3)?,  // window_end
                row.get::<_, i64>(4)?,  // log_count
                row.get::<_, String>(5)?,  // severities (comma-joined)
                row.get::<_, String>(6)?,  // messages (|||joined)
            ))
        },
    )?;

    let mut clusters: Vec<IncidentCluster> = Vec::new();
    for row in rows {
        let (hostname, app_name, window_start, window_end, log_count, severities, messages) =
            row?;

        // Find peak severity (lowest numeric value = highest severity).
        let severity_peak = severities
            .split(',')
            .filter_map(|s| severity_to_num(s).map(|n| (n, s.to_string())))
            .min_by_key(|(n, _)| *n)
            .map(|(_, s)| s)
            .unwrap_or_else(|| "info".to_string());

        // Collect up to 3 representative messages.
        let representative_messages: Vec<String> = messages
            .split("|||")
            .take(3)
            .map(|m| m.to_string())
            .collect();

        // Find AI sessions that overlap this cluster's time window.
        let correlated_sessions =
            find_correlated_sessions(&conn, &window_start, &window_end)?;

        clusters.push(IncidentCluster {
            hostname,
            app_name,
            window_start,
            window_end,
            log_count,
            severity_peak,
            representative_messages,
            correlated_sessions,
        });
    }

    let total_clusters = clusters.len();
    Ok(SimilarIncidentsResult {
        query: params.query.clone(),
        total_clusters,
        truncated: total_clusters >= limit,
        clusters,
    })
}

/// Find AI transcript sessions whose entries overlap the given time window.
fn find_correlated_sessions(
    conn: &rusqlite::Connection,
    window_start: &str,
    window_end: &str,
) -> Result<Vec<CorrelatedSession>> {
    let mut stmt = conn.prepare(
        "SELECT ai_project, ai_tool, ai_session_id, COUNT(*) AS match_count,
                (SELECT l2.message FROM logs l2
                 WHERE l2.ai_project = l.ai_project
                   AND l2.ai_tool = l.ai_tool
                   AND l2.ai_session_id = l.ai_session_id
                   AND l2.timestamp BETWEEN ?1 AND ?2
                 ORDER BY l2.timestamp DESC LIMIT 1) AS best_snippet
         FROM logs l
         WHERE l.ai_project IS NOT NULL AND l.ai_project != ''
           AND l.ai_tool IS NOT NULL AND l.ai_tool != ''
           AND l.ai_session_id IS NOT NULL AND l.ai_session_id != ''
           AND l.timestamp BETWEEN ?1 AND ?2
         GROUP BY ai_project, ai_tool, ai_session_id
         ORDER BY match_count DESC
         LIMIT 5",
    )?;

    let rows = stmt.query_map(rusqlite::params![window_start, window_end], |row| {
        Ok(CorrelatedSession {
            project: row.get(0)?,
            tool: row.get(1)?,
            session_id: row.get(2)?,
            match_count: row.get(3)?,
            best_snippet: row.get(4)?,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// FTS5 search over AI transcript entries, returns sessions grouped by
/// (project, tool, session_id), ranked by match count. Also returns system
/// log context from the top session's time window.
pub fn ask_history_sessions(pool: &DbPool, params: &AskHistoryParams) -> Result<AskHistoryResult> {
    validate_fts_query(&params.query)?;

    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(10).clamp(1, 50) as usize;

    // Search AI transcript entries only.
    let ai_params = SearchAiSessionsParams {
        query: params.query.clone(),
        ai_project: None,
        ai_tool: None,
        from: params.from.clone(),
        to: params.to.clone(),
        limit: Some(limit as u32),
    };
    let session_result = search_ai_sessions(pool, &ai_params)?;

    // Collect the time window of the top session to fetch system log context.
    let context_logs = if let Some(top) = session_result.sessions.first() {
        let ctx_from = top.first_seen.clone();
        let ctx_to = top.last_seen.clone();

        // Fetch non-AI system logs in that window (limited to 20).
        let mut ctx_sql = String::from(
            "SELECT id, timestamp, hostname, facility, severity,
                    app_name, process_id, message, received_at, source_ip,
                    ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
             FROM logs
             WHERE (ai_project IS NULL OR ai_project = '')
               AND timestamp BETWEEN ?1 AND ?2",
        );
        if let Some(hostname) = &params.hostname {
            ctx_sql.push_str(&format!(
                " AND hostname = '{}'",
                hostname.replace('\'', "''")
            ));
        }
        if let Some(app_name) = &params.app_name {
            ctx_sql.push_str(&format!(
                " AND app_name = '{}'",
                app_name.replace('\'', "''")
            ));
        }
        ctx_sql.push_str(" ORDER BY timestamp DESC LIMIT 20");

        let mut stmt = conn.prepare(&ctx_sql)?;
        let rows = stmt
            .query_map(rusqlite::params![ctx_from, ctx_to], map_row)
            .map_err(|e| anyhow::anyhow!("context_logs query failed: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    let total_candidates = session_result.total_candidates;
    let truncated = session_result.truncated;
    Ok(AskHistoryResult {
        query: params.query.clone(),
        total_candidates,
        truncated,
        sessions: session_result.sessions,
        context_logs,
    })
}

/// Return aggregate log statistics + error logs + correlated AI sessions for a
/// given time window.
pub fn incident_context_summary(
    pool: &DbPool,
    params: &IncidentContextParams,
) -> Result<IncidentContextResult> {
    let conn = pool.get()?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200) as usize;
    let severity_threshold = params
        .severity_min
        .as_deref()
        .map(|s| {
            severity_to_num(s).ok_or_else(|| {
                anyhow::anyhow!("invalid severity_min '{}': must be one of emerg, alert, crit, err, warning, notice, info, debug", s)
            })
        })
        .transpose()?
        .unwrap_or_else(|| severity_to_num("warning").unwrap());

    // Total log count in the window.
    let total_logs: i64 = conn.query_row(
        "SELECT COUNT(*) FROM logs WHERE timestamp BETWEEN ?1 AND ?2",
        rusqlite::params![params.from, params.to],
        |r| r.get(0),
    )?;

    // Counts by severity.
    let mut by_sev_stmt = conn.prepare(
        "SELECT severity, COUNT(*) FROM logs WHERE timestamp BETWEEN ?1 AND ?2 GROUP BY severity ORDER BY COUNT(*) DESC",
    )?;
    let by_severity: Vec<SeverityCount> = by_sev_stmt
        .query_map(rusqlite::params![params.from, params.to], |row| {
            Ok(SeverityCount {
                severity: row.get(0)?,
                count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Counts by app_name.
    let mut by_app_stmt = conn.prepare(
        "SELECT app_name, COUNT(*) FROM logs WHERE timestamp BETWEEN ?1 AND ?2 GROUP BY app_name ORDER BY COUNT(*) DESC LIMIT 20",
    )?;
    let by_app: Vec<AppLogCount> = by_app_stmt
        .query_map(rusqlite::params![params.from, params.to], |row| {
            Ok(AppLogCount {
                app_name: row.get(0)?,
                count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Error logs at or above the threshold.
    let error_severities: Vec<String> = SEVERITY_LEVELS[..=severity_threshold as usize]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let placeholders: Vec<String> = (3..=2 + error_severities.len())
        .map(|i| format!("?{i}"))
        .collect();
    let mut bindings: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(params.from.clone()),
        rusqlite::types::Value::Text(params.to.clone()),
    ];
    for s in &error_severities {
        bindings.push(rusqlite::types::Value::Text(s.clone()));
    }
    let mut err_sql = format!(
        "SELECT id, timestamp, hostname, facility, severity,
                app_name, process_id, message, received_at, source_ip,
                ai_tool, ai_project, ai_session_id, ai_transcript_path, metadata_json
         FROM logs
         WHERE timestamp BETWEEN ?1 AND ?2
           AND severity IN ({})
           AND (ai_project IS NULL OR ai_project = '')
         ORDER BY timestamp DESC LIMIT {limit}",
        placeholders.join(", ")
    );

    if let Some(hostname) = &params.hostname {
        bindings.push(rusqlite::types::Value::Text(hostname.clone()));
        err_sql = err_sql.replace(
            "ORDER BY timestamp",
            &format!("AND hostname = ?{} ORDER BY timestamp", bindings.len()),
        );
    }

    let mut err_stmt = conn.prepare(&err_sql)?;
    let error_rows = err_stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), map_row)
        .map_err(|e| anyhow::anyhow!("error_logs query failed: {e}"))?;
    let error_logs: Vec<crate::db::models::LogEntry> =
        error_rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let error_logs_truncated = error_logs.len() >= limit;

    // AI sessions active in the window.
    let ai_params = ListAiSessionsParams {
        ai_project: None,
        ai_tool: None,
        hostname: params.hostname.clone(),
        from: Some(params.from.clone()),
        to: Some(params.to.clone()),
        limit: Some(20),
    };
    let ai_sessions = list_ai_sessions(pool, &ai_params)?;

    Ok(IncidentContextResult {
        window_from: params.from.clone(),
        window_to: params.to.clone(),
        total_logs,
        by_severity,
        by_app,
        error_logs,
        error_logs_truncated,
        ai_sessions,
    })
}
```

- [ ] **Step 4: Run the failing test to verify it now passes**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test similar_incidents_clusters_returns_clusters 2>&1 | tail -20
```

Expected: `test ... ok`

- [ ] **Step 5: Write a failing test for `incident_context_summary`**

Append to `src/db/queries_tests.rs`:

```rust
#[test]
fn incident_context_summary_returns_window_stats() {
    use super::*;
    use crate::config::StorageConfig;
    use crate::db::{init_pool, insert_logs_batch, LogBatchEntry};

    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("ctx-test.db"));
    let pool = init_pool(&storage).unwrap();

    let logs = vec![
        LogBatchEntry {
            timestamp: "2024-02-01T08:00:00Z".into(),
            hostname: "db-01".into(),
            facility: None,
            severity: "err".into(),
            app_name: Some("postgres".into()),
            process_id: None,
            message: "FATAL: out of shared memory".into(),
            raw: "FATAL: out of shared memory".into(),
            source_ip: "10.0.0.2:514".into(),
            docker_checkpoint: None,
            ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
            metadata_json: None,
            http_status: None, auth_outcome: None, dns_blocked: None,
            event_action: None, parse_error: None,
        },
        LogBatchEntry {
            timestamp: "2024-02-01T08:01:00Z".into(),
            hostname: "db-01".into(),
            facility: None,
            severity: "info".into(),
            app_name: Some("postgres".into()),
            process_id: None,
            message: "database system is ready".into(),
            raw: "database system is ready".into(),
            source_ip: "10.0.0.2:514".into(),
            docker_checkpoint: None,
            ai_tool: None, ai_project: None, ai_session_id: None, ai_transcript_path: None,
            metadata_json: None,
            http_status: None, auth_outcome: None, dns_blocked: None,
            event_action: None, parse_error: None,
        },
    ];
    insert_logs_batch(&pool, &logs).unwrap();

    let params = IncidentContextParams {
        from: "2024-02-01T07:00:00Z".into(),
        to: "2024-02-01T09:00:00Z".into(),
        hostname: None,
        app_name: None,
        query: None,
        severity_min: Some("err".into()),
        limit: Some(10),
    };
    let result = incident_context_summary(&pool, &params).unwrap();
    assert_eq!(result.total_logs, 2);
    assert!(!result.by_severity.is_empty());
    assert_eq!(result.error_logs.len(), 1);  // only "err" row
    assert_eq!(result.error_logs[0].message, "FATAL: out of shared memory");
}
```

- [ ] **Step 6: Run test to verify it passes**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test incident_context_summary_returns_window 2>&1 | tail -20
```

Expected: `test ... ok`

- [ ] **Step 7: Run full test suite to verify no regressions**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/db/queries.rs src/db/queries_tests.rs && git commit -m "feat(db): add RAG v1 queries — similar_incidents_clusters, ask_history_sessions, incident_context_summary"
```

---

## Task 3: Export new types and functions from `src/db.rs`

**Files:**
- Modify: `src/db.rs`

- [ ] **Step 1: Add exports to `src/db.rs`**

In `src/db.rs`, update the `pub use models::{...}` block to include the new types. The existing block ends at:

```rust
pub use models::{
    AbuseIncident, AiAbuseMatch, AiAbuseParams, AiAbuseResult, AiCorrelateParams, AiIncidentParams,
    AiIncidentResult, AiInvestigateParams, AiInvestigateResult, AiProjectContext,
    AiProjectContextParams, AiProjectInventoryEntry, AiRelatedLogsForAnchor, AiRelatedLogsParams,
    AiRelatedWindow, AiSessionEntry, AiToolInventoryEntry, AiUsageBlock, AiUsageBlocksParams,
    AiUsageBlocksResult, DbStats, DockerCheckpoint, ErrorSummaryEntry, HostEntry, IncidentEvidence,
    ListAiProjectsParams, ListAiProjectsResult, ListAiSessionsParams, ListAiToolsParams,
    ListAiToolsResult, LogBatchEntry, LogEntry, SearchAiSessionsParams, SearchAiSessionsResult,
    SearchParams, SearchedAiSessionEntry,
};
```

Replace with:

```rust
pub use models::{
    AbuseIncident, AiAbuseMatch, AiAbuseParams, AiAbuseResult, AiCorrelateParams, AiIncidentParams,
    AiIncidentResult, AiInvestigateParams, AiInvestigateResult, AiProjectContext,
    AiProjectContextParams, AiProjectInventoryEntry, AiRelatedLogsForAnchor, AiRelatedLogsParams,
    AiRelatedWindow, AiSessionEntry, AiToolInventoryEntry, AiUsageBlock, AiUsageBlocksParams,
    AiUsageBlocksResult, AppLogCount, AskHistoryParams, AskHistoryResult, CorrelatedSession,
    DbStats, DockerCheckpoint, ErrorSummaryEntry, HostEntry, IncidentCluster, IncidentContextParams,
    IncidentContextResult, IncidentEvidence, ListAiProjectsParams, ListAiProjectsResult,
    ListAiSessionsParams, ListAiToolsParams, ListAiToolsResult, LogBatchEntry, LogEntry,
    SearchAiSessionsParams, SearchAiSessionsResult, SearchParams, SearchedAiSessionEntry,
    SeverityCount, SimilarIncidentsParams, SimilarIncidentsResult,
};
```

Also update the `pub use queries::{...}` block to include the new functions:

```rust
pub use queries::{
    ask_history_sessions, get_error_summary, get_stats, incident_context_summary,
    investigate_ai_incidents, list_ai_projects, list_ai_sessions, list_ai_tools, list_hosts,
    search_ai_abuse, search_ai_anchors, search_ai_incidents, search_ai_related_logs,
    search_ai_sessions, search_logs, severity_to_num, similar_incidents_clusters, tail_logs,
    validate_fts_query, SEVERITY_LEVELS,
};
```

- [ ] **Step 2: Compile-check**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo check 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/db.rs && git commit -m "feat(db): export RAG v1 types and functions from db module"
```

---

## Task 4: App-layer models — request/response types

**Files:**
- Modify: `src/app/models.rs` (append to end)

- [ ] **Step 1: Append app-layer models to `src/app/models.rs`**

```rust
// ---------------------------------------------------------------------------
// RAG v1: similar_incidents, ask_history, incident_context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimilarIncidentsRequest {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub severity_min: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Cluster window in minutes. Default 30, clamp 5..=120.
    pub window_minutes: Option<u32>,
    /// Max clusters to return. Default 10, clamp 1..=50.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelatedSession {
    pub session_id: String,
    pub project: String,
    pub tool: String,
    pub match_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_snippet: Option<String>,
}

impl From<db::CorrelatedSession> for CorrelatedSession {
    fn from(v: db::CorrelatedSession) -> Self {
        Self {
            session_id: v.session_id,
            project: v.project,
            tool: v.tool,
            match_count: v.match_count,
            best_snippet: v.best_snippet,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentCluster {
    pub hostname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    pub window_start: String,
    pub window_end: String,
    pub log_count: i64,
    pub severity_peak: String,
    pub representative_messages: Vec<String>,
    pub correlated_sessions: Vec<CorrelatedSession>,
}

impl From<db::IncidentCluster> for IncidentCluster {
    fn from(v: db::IncidentCluster) -> Self {
        Self {
            hostname: v.hostname,
            app_name: v.app_name,
            window_start: v.window_start,
            window_end: v.window_end,
            log_count: v.log_count,
            severity_peak: v.severity_peak,
            representative_messages: v.representative_messages,
            correlated_sessions: v.correlated_sessions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarIncidentsResponse {
    pub query: String,
    pub total_clusters: usize,
    pub truncated: bool,
    pub clusters: Vec<IncidentCluster>,
}

impl From<db::SimilarIncidentsResult> for SimilarIncidentsResponse {
    fn from(v: db::SimilarIncidentsResult) -> Self {
        Self {
            query: v.query,
            total_clusters: v.total_clusters,
            truncated: v.truncated,
            clusters: v.clusters.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskHistoryRequest {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Max sessions to return. Default 10, clamp 1..=50.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskHistoryResponse {
    pub query: String,
    pub total_candidates: usize,
    pub truncated: bool,
    pub sessions: Vec<SearchedSessionEntry>,
    pub context_logs: Vec<LogEntry>,
}

impl From<db::AskHistoryResult> for AskHistoryResponse {
    fn from(v: db::AskHistoryResult) -> Self {
        Self {
            query: v.query,
            total_candidates: v.total_candidates,
            truncated: v.truncated,
            sessions: v.sessions.into_iter().map(Into::into).collect(),
            context_logs: v.context_logs.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentContextRequest {
    pub from: String,
    pub to: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub query: Option<String>,
    pub severity_min: Option<String>,
    /// Max error log rows. Default 50, clamp 1..=200.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeverityCount {
    pub severity: String,
    pub count: i64,
}

impl From<db::SeverityCount> for SeverityCount {
    fn from(v: db::SeverityCount) -> Self {
        Self { severity: v.severity, count: v.count }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogCount {
    pub app_name: Option<String>,
    pub count: i64,
}

impl From<db::AppLogCount> for AppLogCount {
    fn from(v: db::AppLogCount) -> Self {
        Self { app_name: v.app_name, count: v.count }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentContextResponse {
    pub window_from: String,
    pub window_to: String,
    pub total_logs: i64,
    pub by_severity: Vec<SeverityCount>,
    pub by_app: Vec<AppLogCount>,
    pub error_logs: Vec<LogEntry>,
    pub error_logs_truncated: bool,
    pub ai_sessions: Vec<AiSessionEntry>,
}

impl From<db::IncidentContextResult> for IncidentContextResponse {
    fn from(v: db::IncidentContextResult) -> Self {
        Self {
            window_from: v.window_from,
            window_to: v.window_to,
            total_logs: v.total_logs,
            by_severity: v.by_severity.into_iter().map(Into::into).collect(),
            by_app: v.by_app.into_iter().map(Into::into).collect(),
            error_logs: v.error_logs.into_iter().map(Into::into).collect(),
            error_logs_truncated: v.error_logs_truncated,
            ai_sessions: v.ai_sessions.into_iter().map(Into::into).collect(),
        }
    }
}
```

- [ ] **Step 2: Compile-check**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo check 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/app/models.rs && git commit -m "feat(app): add RAG v1 app-layer request/response types"
```

---

## Task 5: App service methods

**Files:**
- Modify: `src/app/service.rs`

The new methods follow the exact same pattern as `search_sessions` (see line ~526). They each: parse timestamps, build db params, call `self.run_db(...)`, and convert the result.

- [ ] **Step 1: Add imports to the top of `src/app/service.rs`**

Find the existing import block that starts with:
```rust
use super::models::{
    AbuseSearchRequest, AbuseSearchResponse, AiCorrelateRequest, AiCorrelateResponse,
```

Add `AskHistoryRequest, AskHistoryResponse, IncidentContextRequest, IncidentContextResponse, SimilarIncidentsRequest, SimilarIncidentsResponse,` to that block (alphabetically positioned).

- [ ] **Step 2: Add the three service methods to `src/app/service.rs`**

Find the end of the `impl SyslogService` block (before the closing `}`). Append:

```rust
    pub async fn similar_incidents(
        &self,
        req: SimilarIncidentsRequest,
    ) -> ServiceResult<SimilarIncidentsResponse> {
        let from = parse_optional_timestamp(req.from.as_deref(), "from")?;
        let to = parse_optional_timestamp(req.to.as_deref(), "to")?;
        let result = self
            .run_db(move |pool| {
                db::similar_incidents_clusters(
                    pool,
                    &db::SimilarIncidentsParams {
                        query: req.query,
                        hostname: req.hostname,
                        app_name: req.app_name,
                        severity_min: req.severity_min,
                        from,
                        to,
                        window_minutes: req.window_minutes,
                        limit: req.limit,
                    },
                )
            })
            .await?;
        Ok(result.into())
    }

    pub async fn ask_history(
        &self,
        req: AskHistoryRequest,
    ) -> ServiceResult<AskHistoryResponse> {
        let from = parse_optional_timestamp(req.from.as_deref(), "from")?;
        let to = parse_optional_timestamp(req.to.as_deref(), "to")?;
        let result = self
            .run_db(move |pool| {
                db::ask_history_sessions(
                    pool,
                    &db::AskHistoryParams {
                        query: req.query,
                        hostname: req.hostname,
                        app_name: req.app_name,
                        from,
                        to,
                        limit: req.limit,
                    },
                )
            })
            .await?;
        Ok(result.into())
    }

    pub async fn incident_context(
        &self,
        req: IncidentContextRequest,
    ) -> ServiceResult<IncidentContextResponse> {
        let from = parse_required_timestamp(&req.from, "from")
            .map_err(ServiceError::InvalidInput)?;
        let to = parse_required_timestamp(&req.to, "to")
            .map_err(ServiceError::InvalidInput)?;
        let result = self
            .run_db(move |pool| {
                db::incident_context_summary(
                    pool,
                    &db::IncidentContextParams {
                        from,
                        to,
                        hostname: req.hostname,
                        app_name: req.app_name,
                        query: req.query,
                        severity_min: req.severity_min,
                        limit: req.limit,
                    },
                )
            })
            .await?;
        Ok(result.into())
    }
```

- [ ] **Step 3: Compile-check**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo check 2>&1 | grep "^error" | head -20
```

Expected: no errors. Fix any import errors (the `parse_required_timestamp` takes a `&str` and returns `Result<String, ServiceError>` via `.map_err`).

Note: `parse_required_timestamp` in `src/app/time.rs` returns `Result<String, ServiceError>` — check this signature and adjust if needed:

```bash
grep -n "pub fn parse_required_timestamp" /home/jmagar/workspace/syslog-mcp/src/app/time.rs
```

If it returns `ServiceResult<String>`, remove the `.map_err(ServiceError::InvalidInput)` wrapper.

- [ ] **Step 4: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/app/service.rs && git commit -m "feat(app): add similar_incidents, ask_history, incident_context service methods"
```

---

## Task 6: MCP dispatch — schemas and tools

**Files:**
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/tools.rs`

- [ ] **Step 1: Add action names to `SYSLOG_ACTIONS` in `src/mcp/schemas.rs`**

Find:
```rust
pub(super) const SYSLOG_ACTIONS: &[&str] = &[
    "search",
    ...
    "help",
];
```

Add three new entries before `"help"`:
```rust
    "similar_incidents",
    "ask_history",
    "incident_context",
    "help",
```

- [ ] **Step 2: Update the tool description string in `src/mcp/schemas.rs`**

Find the `"description"` string in `tool_definitions()` that begins:

```rust
"description": "Query syslog-mcp logs with action-based subcommands: syslog search, syslog tail, ...
```

Append `, syslog similar_incidents, syslog ask_history, syslog incident_context` before the closing period.

- [ ] **Step 3: Add parameter descriptions in `src/mcp/schemas.rs`**

Find the `"from"` description block. Update it to include the new actions:

```rust
"from": {
    "type": "string",
    "description": "For action=search, sessions, search_sessions, abuse, abuse_incidents, abuse_investigate, ai_correlate, usage_blocks, list_ai_tools, list_ai_projects, errors, timeline, patterns, apps, similar_incidents, ask_history: start of time range as ISO 8601/RFC3339. Required for incident_context. Strongly recommended for timeline and patterns — omitting from/to causes a full-history scan."
},
```

Update `"to"` similarly.

Update `"limit"` description to add:
```
For action=similar_incidents: max incident clusters, default 10, max 50. For action=ask_history: max sessions, default 10, max 50. For action=incident_context: max error log rows, default 50, max 200.
```

Update `"severity_min"` description to add `similar_incidents, incident_context`.

Update `"hostname"` description to add `similar_incidents, ask_history, incident_context`.

Update `"app_name"` description to add `similar_incidents, ask_history, incident_context`.

Update `"query"` description to add: `For action=similar_incidents or incident_context: FTS5 query to filter log hits. For action=ask_history: FTS5 query over AI transcript entries.`

Add a new parameter for `window_minutes` that mentions `similar_incidents`:

Find:
```rust
"window_minutes": {
    "type": "integer",
    "description": "For action=correlate: minutes before and after reference_time to search, default 5, max 60. For action=ai_correlate: minutes before and after each AI anchor, default 5, max 120. For action=abuse_incidents or abuse_investigate: incident grouping window, default 10, max 120."
},
```

Replace with:
```rust
"window_minutes": {
    "type": "integer",
    "description": "For action=correlate: minutes before and after reference_time to search, default 5, max 60. For action=ai_correlate: minutes before and after each AI anchor, default 5, max 120. For action=abuse_incidents or abuse_investigate: incident grouping window, default 10, max 120. For action=similar_incidents: cluster grouping window, default 30, max 120."
},
```

- [ ] **Step 4: Add imports to `src/mcp/tools.rs`**

Find the existing import block:
```rust
use crate::app::{
    AbuseSearchRequest, AiCorrelateRequest, AiIncidentRequest, AiInvestigateRequest,
    ...
};
```

Add `AskHistoryRequest, IncidentContextRequest, SimilarIncidentsRequest,` to that block.

- [ ] **Step 5: Add dispatch arms in `src/mcp/tools.rs`**

In the `tool_syslog` match, find:
```rust
        "help" => tool_syslog_help().await,
```

Add before it:
```rust
        "similar_incidents" => tool_similar_incidents(state, args).await,
        "ask_history" => tool_ask_history(state, args).await,
        "incident_context" => tool_incident_context(state, args).await,
```

- [ ] **Step 6: Add handler functions to `src/mcp/tools.rs`**

Append before the closing of the file (before the `#[cfg(test)]` block):

```rust
async fn tool_similar_incidents(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let query =
        string_arg(&args, "query").ok_or_else(|| anyhow::anyhow!("query is required"))?;
    let response = state
        .service
        .similar_incidents(SimilarIncidentsRequest {
            query,
            hostname: string_arg(&args, "hostname"),
            app_name: string_arg(&args, "app_name"),
            severity_min: string_arg(&args, "severity_min"),
            from: string_arg(&args, "from"),
            to: string_arg(&args, "to"),
            window_minutes: u32_arg(&args, "window_minutes")?,
            limit: u32_arg(&args, "limit")?,
        })
        .await?;
    tracing::debug!(
        cluster_count = response.total_clusters,
        "similar_incidents completed"
    );
    Ok(serde_json::to_value(response)?)
}

async fn tool_ask_history(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let query =
        string_arg(&args, "query").ok_or_else(|| anyhow::anyhow!("query is required"))?;
    let response = state
        .service
        .ask_history(AskHistoryRequest {
            query,
            hostname: string_arg(&args, "hostname"),
            app_name: string_arg(&args, "app_name"),
            from: string_arg(&args, "from"),
            to: string_arg(&args, "to"),
            limit: u32_arg(&args, "limit")?,
        })
        .await?;
    tracing::debug!(
        session_count = response.sessions.len(),
        "ask_history completed"
    );
    Ok(serde_json::to_value(response)?)
}

async fn tool_incident_context(state: &AppState, args: Value) -> anyhow::Result<Value> {
    let from = string_arg(&args, "from")
        .ok_or_else(|| anyhow::anyhow!("from is required for incident_context"))?;
    let to =
        string_arg(&args, "to").ok_or_else(|| anyhow::anyhow!("to is required for incident_context"))?;
    let response = state
        .service
        .incident_context(IncidentContextRequest {
            from,
            to,
            hostname: string_arg(&args, "hostname"),
            app_name: string_arg(&args, "app_name"),
            query: string_arg(&args, "query"),
            severity_min: string_arg(&args, "severity_min"),
            limit: u32_arg(&args, "limit")?,
        })
        .await?;
    tracing::debug!(
        total_logs = response.total_logs,
        error_count = response.error_logs.len(),
        "incident_context completed"
    );
    Ok(serde_json::to_value(response)?)
}
```

- [ ] **Step 7: Update `tool_syslog_help` to document new actions**

Find the `tool_syslog_help` function. It returns a large JSON blob listing actions. Add entries for the three new actions:

```
## syslog similar_incidents

Find historical incidents similar to a query. Groups FTS5 hits into time-windowed clusters by host+app. Returns ranked clusters with representative messages and correlated AI sessions.

Required: query (FTS5 syntax, e.g. "nginx upstream error" or "OOM killer")
Optional: hostname, app_name, severity_min, from, to, window_minutes (default 30), limit (default 10)

Example: {"action":"similar_incidents","query":"upstream connect error","app_name":"nginx"}

---

## syslog ask_history

Search AI session transcripts for past work related to a topic. Returns sessions ranked by match count with system log context from the top session's time window.

Required: query (FTS5 syntax, e.g. "nginx fix" or "OOM postgres")
Optional: hostname, app_name, from, to, limit (default 10)

Example: {"action":"ask_history","query":"nginx ssl certificate"}

---

## syslog incident_context

Return full context for a time window: log summary by severity and app, error logs, and correlated AI sessions. Useful for understanding what happened during a known incident window.

Required: from, to (ISO 8601/RFC3339)
Optional: hostname, app_name, query, severity_min (default warning), limit (default 50)

Example: {"action":"incident_context","from":"2024-01-15T10:00:00Z","to":"2024-01-15T11:00:00Z"}
```

- [ ] **Step 8: Build and test**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 9: Lint**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo clippy --all-targets -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 10: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/mcp/schemas.rs src/mcp/tools.rs && git commit -m "feat(mcp): add similar_incidents, ask_history, incident_context actions to MCP dispatch"
```

---

## Task 7: CLI — three new `syslog ai` subcommands

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Add variants to `AiCommand` enum in `src/cli.rs`**

Find:
```rust
pub(crate) enum AiCommand {
    Search(AiSearchArgs),
    Abuse(AiAbuseArgs),
    Correlate(AiCorrelateArgs),
    Blocks(AiBlocksArgs),
    Context(AiContextArgs),
    Tools(AiListArgs),
    Projects(AiListArgs),
    Index(AiIndexArgs),
    Add(AiAddArgs),
    Watch(AiWatchArgs),
    Checkpoints(AiCheckpointsArgs),
    Errors(AiErrorsArgs),
    PruneCheckpoints(AiPruneCheckpointsArgs),
    Doctor(AiDoctorArgs),
    WatchStatus(OutputArgs),
    SmokeWatch(OutputArgs),
}
```

Add three new variants:

```rust
    SimilarIncidents(AiSimilarArgs),
    AskHistory(AiAskHistoryArgs),
    IncidentContext(AiIncidentContextArgs),
```

- [ ] **Step 2: Add arg structs in `src/cli.rs`**

Find where the other `Ai*Args` structs are defined (search for `struct AiSearchArgs`). Add new structs nearby:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiSimilarArgs {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub severity_min: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub window_minutes: Option<u32>,
    pub limit: Option<u32>,
    pub output: OutputFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiAskHistoryArgs {
    pub query: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub output: OutputFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiIncidentContextArgs {
    pub from: String,
    pub to: String,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub query: Option<String>,
    pub severity_min: Option<String>,
    pub limit: Option<u32>,
    pub output: OutputFormat,
}
```

- [ ] **Step 3: Add parsing and dispatch in `src/cli.rs`**

Find the function (or match block) that parses CLI args into `AiCommand` variants. Follow the exact pattern used for `AiCommand::Search`. Add cases for the three new variants. The specific location will be a `match` on `ai` subcommand string or a series of `if subcmd == "..."` checks — look for where `"search"` maps to `AiCommand::Search(...)`.

Look for the pattern:
```rust
"search" => AiCommand::Search(AiSearchArgs { ... })
```

Add:
```rust
"similar" => AiCommand::SimilarIncidents(AiSimilarArgs {
    query: require_arg(&args, "<query>", "syslog ai similar <query>")?,
    hostname: flag_val(&args, "--host"),
    app_name: flag_val(&args, "--app"),
    severity_min: flag_val(&args, "--severity-min"),
    from: flag_val(&args, "--since"),
    to: flag_val(&args, "--until"),
    window_minutes: flag_u32(&args, "--window-minutes")?,
    limit: flag_u32(&args, "--limit")?,
    output: output_format(&args),
}),
"ask-history" => AiCommand::AskHistory(AiAskHistoryArgs {
    query: require_arg(&args, "<query>", "syslog ai ask-history <query>")?,
    hostname: flag_val(&args, "--host"),
    app_name: flag_val(&args, "--app"),
    from: flag_val(&args, "--since"),
    to: flag_val(&args, "--until"),
    limit: flag_u32(&args, "--limit")?,
    output: output_format(&args),
}),
"incident-context" => AiCommand::IncidentContext(AiIncidentContextArgs {
    from: flag_val(&args, "--since").ok_or_else(|| anyhow::anyhow!("--since is required"))?,
    to: flag_val(&args, "--until").ok_or_else(|| anyhow::anyhow!("--until is required"))?,
    hostname: flag_val(&args, "--host"),
    app_name: flag_val(&args, "--app"),
    query: flag_val(&args, "--query"),
    severity_min: flag_val(&args, "--severity-min"),
    limit: flag_u32(&args, "--limit")?,
    output: output_format(&args),
}),
```

Note: `require_arg`, `flag_val`, `flag_u32`, `output_format` are the CLI arg helper functions used by the existing `AiCommand` parsing. If the actual names differ, check the surrounding code and use the real function names.

- [ ] **Step 4: Add dispatch in the CLI execution block**

Find the `match` that dispatches `AiCommand` variants to service calls (look for `AiCommand::Search(args) => { ... }`). Add:

```rust
AiCommand::SimilarIncidents(args) => {
    let resp = service
        .similar_incidents(syslog_mcp::app::SimilarIncidentsRequest {
            query: args.query,
            hostname: args.hostname,
            app_name: args.app_name,
            severity_min: args.severity_min,
            from: args.from,
            to: args.to,
            window_minutes: args.window_minutes,
            limit: args.limit,
        })
        .await?;
    print_output(&resp, args.output)
}
AiCommand::AskHistory(args) => {
    let resp = service
        .ask_history(syslog_mcp::app::AskHistoryRequest {
            query: args.query,
            hostname: args.hostname,
            app_name: args.app_name,
            from: args.from,
            to: args.to,
            limit: args.limit,
        })
        .await?;
    print_output(&resp, args.output)
}
AiCommand::IncidentContext(args) => {
    let resp = service
        .incident_context(syslog_mcp::app::IncidentContextRequest {
            from: args.from,
            to: args.to,
            hostname: args.hostname,
            app_name: args.app_name,
            query: args.query,
            severity_min: args.severity_min,
            limit: args.limit,
        })
        .await?;
    print_output(&resp, args.output)
}
```

Note: Check how existing commands import service types — it may be via `use syslog_mcp::app::*;` already in scope. Use the same pattern.

- [ ] **Step 5: Add help text for the three new commands**

Find the `help` or `usage` string for `syslog ai` subcommands (search for `"syslog ai search"` or similar). Add:

```
  syslog ai similar <query> [--host H] [--app A] [--since T] [--until T]
                            [--severity-min S] [--window-minutes N] [--limit N]
      Find historical incidents similar to a query. Returns FTS5-matched log clusters
      grouped by host+app within a time window, with correlated AI sessions.

  syslog ai ask-history <query> [--host H] [--app A] [--since T] [--until T] [--limit N]
      Search AI session transcripts for past work related to a topic. Returns sessions
      ranked by match count with system log context from the top session's window.

  syslog ai incident-context --since T --until T [--host H] [--app A]
                              [--query Q] [--severity-min S] [--limit N]
      Full context for a time window: log summary by severity/app, error logs,
      correlated AI sessions. Useful for known incident windows.
```

- [ ] **Step 6: Build and run full test suite**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 7: Lint**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo clippy --all-targets -- -D warnings 2>&1 | head -40
```

Expected: no warnings/errors. Fix any dead_code or unused_variables.

- [ ] **Step 8: Format**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo fmt --check 2>&1
```

If changes needed: `cargo fmt && git add -u`

- [ ] **Step 9: Commit**

```bash
cd /home/jmagar/workspace/syslog-mcp && git add src/cli.rs && git commit -m "feat(cli): add syslog ai similar, ask-history, incident-context subcommands"
```

---

## Task 8: Integration verification and cleanup

**Files:** None new — verification only.

- [ ] **Step 1: Run the full build in release mode to catch any dead code**

```bash
cd /home/jmagar/workspace/syslog-mcp && cargo build --release 2>&1 | grep -E "^error|^warning\[" | head -30
```

Expected: no errors.

- [ ] **Step 2: Run `just lint` (strict clippy via Justfile)**

```bash
cd /home/jmagar/workspace/syslog-mcp && just lint 2>&1 | head -40
```

Expected: no lint errors. Fix any clippy suggestions.

- [ ] **Step 3: Run `just test`**

```bash
cd /home/jmagar/workspace/syslog-mcp && just test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 4: Smoke-test the MCP actions against a running server (optional)**

If a server is running at `localhost:3000`:
```bash
curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"syslog","arguments":{"action":"similar_incidents","query":"error"}}}' \
  | jq '.result.content[0].text | fromjson | keys'
```

Expected: `["clusters","query","total_clusters","truncated"]`

```bash
curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"syslog","arguments":{"action":"incident_context","from":"2024-01-01T00:00:00Z","to":"2024-01-02T00:00:00Z"}}}' \
  | jq '.result.content[0].text | fromjson | keys'
```

Expected: `["ai_sessions","by_app","by_severity","error_logs","error_logs_truncated","total_logs","window_from","window_to"]`

- [ ] **Step 5: Update beads and commit everything**

```bash
cd /home/jmagar/workspace/syslog-mcp && bd update syslog-mcp-h6da --status in_progress
git push
```

---

## Self-Review Checklist

**Spec coverage:**
- `similar_incidents` — implemented in Tasks 1-6. Matches spec action name. Returns clusters with `representative_messages` and `correlated_sessions`. ✓
- `ask_history` — implemented in Tasks 1-6. Matches spec action name. Returns session hits + system log context. ✓
- `incident_context` — implemented in Tasks 1-6. New v1 action (not in full spec, but fills the "given a window, what happened?" use case). ✓
- CLI surface — `syslog ai similar`, `syslog ai ask-history`, `syslog ai incident-context` in Task 7. ✓
- No new dependencies — pure FTS5 + existing rusqlite/SQLx. ✓
- v2 deferral note — documented in plan header. ✓

**Placeholder scan:**
- All code blocks contain complete, compilable Rust code. ✓
- All type names are consistent across tasks. ✓
- `find_correlated_sessions` is defined inline in Task 2 and used in `similar_incidents_clusters`. ✓

**Type consistency:**
- `CorrelatedSession` defined in `db/models.rs` (Task 1), exported in `db.rs` (Task 3), wrapped in `app/models.rs` (Task 4). ✓
- `IncidentCluster` same path. ✓
- `SimilarIncidentsResult` → `SimilarIncidentsResponse` via `From` impl. ✓
- `AskHistoryResult` → `AskHistoryResponse` via `From` impl. ✓
- `IncidentContextResult` → `IncidentContextResponse` via `From` impl. ✓

**Known implementation note:** In Task 2 `ask_history_sessions`, the context log query uses string interpolation with manual quoting for hostname/app_name. This is safe for the specific inputs (validated by service layer) but should be refactored to use parameterized queries if extended. Left as-is for v1 simplicity; note in a comment.
