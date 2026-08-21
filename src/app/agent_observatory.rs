//! Handler-independent Agent Observatory read service.

use serde::{Deserialize, Serialize};

use super::cursor::{
    CursorDirection, CursorError, PageCursor, decode_cursor, encode_cursor, filter_fingerprint,
};
use super::{CortexService, ServiceError, ServiceResult};
use crate::db;
use crate::db::agent_observatory as ao;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    pub limit: usize,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub pagination: Pagination,
    pub as_of: String,
    pub stream_cursor: String,
}

fn cursor_error(error: CursorError) -> ServiceError {
    ServiceError::InvalidInput(error.to_string())
}
fn limit(value: usize, maximum: usize) -> usize {
    value.clamp(1, maximum)
}
#[allow(clippy::too_many_arguments)]
fn page<T>(
    mut items: Vec<T>,
    requested: usize,
    maximum: usize,
    filters: &str,
    direction: CursorDirection,
    key: impl Fn(&T) -> (String, i64),
    as_of: String,
    stream_cursor: String,
) -> ServiceResult<Page<T>> {
    let limit = limit(requested, maximum);
    let truncated = items.len() > limit;
    if truncated {
        items.truncate(limit);
    }
    let next_cursor = if truncated {
        items
            .last()
            .map(|item| {
                let (sort, id) = key(item);
                encode_cursor(&PageCursor {
                    sort,
                    id,
                    direction,
                    filters: filters.to_owned(),
                })
                .map_err(cursor_error)
            })
            .transpose()?
    } else {
        None
    };
    Ok(Page {
        items,
        pagination: Pagination {
            limit,
            next_cursor,
            truncated,
        },
        as_of,
        stream_cursor,
    })
}
fn decode(
    value: Option<&str>,
    fingerprint: &str,
    direction: CursorDirection,
) -> ServiceResult<Option<PageCursor>> {
    value
        .map(|v| decode_cursor(v, fingerprint, direction).map_err(cursor_error))
        .transpose()
}
fn snapshot(pool: &db::DbPool) -> anyhow::Result<(String, String)> {
    let conn = pool.get()?;
    Ok((
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?,
        conn.query_row(
            "SELECT COALESCE(MAX(id),0) FROM agent_projection_outbox",
            [],
            |r| r.get::<_, i64>(0),
        )?
        .to_string(),
    ))
}

impl CortexService {
    pub async fn observatory_repositories(
        &self,
        query: ao::RepositoryQuery,
        cursor: Option<String>,
        requested: usize,
    ) -> ServiceResult<Page<ao::ObservatoryRepositoryRow>> {
        let fingerprint = filter_fingerprint(&query).map_err(cursor_error)?;
        let decoded = decode(cursor.as_deref(), &fingerprint, CursorDirection::Desc)?;
        self.run_db("observatory.repositories", move |pool| {
            let (rows, meta) = (
                ao::list_observatory_repositories(
                    pool,
                    &query,
                    decoded.as_ref().map(|c| (c.sort.as_str(), c.id)),
                    limit(requested, 200) + 1,
                )?,
                snapshot(pool)?,
            );
            page(
                rows,
                requested,
                200,
                &fingerprint,
                CursorDirection::Desc,
                |r| (r.last_seen_at.clone(), r.id),
                meta.0,
                meta.1,
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))
        })
        .await
    }
    pub async fn observatory_runs(
        &self,
        query: ao::AgentRunQuery,
        cursor: Option<String>,
        requested: usize,
    ) -> ServiceResult<Page<ao::ObservatoryRunRow>> {
        let fingerprint = filter_fingerprint(&query).map_err(cursor_error)?;
        let decoded = decode(cursor.as_deref(), &fingerprint, CursorDirection::Desc)?;
        self.run_db("observatory.runs", move |pool| {
            let (rows, meta) = (
                ao::list_observatory_runs(
                    pool,
                    &query,
                    decoded.as_ref().map(|c| (c.sort.as_str(), c.id)),
                    limit(requested, 200) + 1,
                )?,
                snapshot(pool)?,
            );
            page(
                rows,
                requested,
                200,
                &fingerprint,
                CursorDirection::Desc,
                |r| (r.last_activity_at.clone(), r.id),
                meta.0,
                meta.1,
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))
        })
        .await
    }

    pub async fn observatory_worktrees(
        &self,
        repository_id: i64,
        branch: Option<String>,
        dirty: Option<bool>,
        include_removed: bool,
        cursor: Option<String>,
        requested: usize,
    ) -> ServiceResult<Page<ao::ObservatoryWorktreeRow>> {
        #[derive(Serialize)]
        struct Bound<'a> {
            repository_id: i64,
            branch: &'a Option<String>,
            dirty: Option<bool>,
            include_removed: bool,
        }
        let fingerprint = filter_fingerprint(&Bound {
            repository_id,
            branch: &branch,
            dirty,
            include_removed,
        })
        .map_err(cursor_error)?;
        let decoded = decode(cursor.as_deref(), &fingerprint, CursorDirection::Desc)?;
        self.run_db("observatory.worktrees", move |pool| {
            let rows = ao::list_observatory_worktrees(
                pool,
                repository_id,
                branch.as_deref(),
                dirty,
                include_removed,
                decoded.as_ref().map(|c| (c.sort.as_str(), c.id)),
                limit(requested, 200) + 1,
            )?;
            let meta = snapshot(pool)?;
            page(
                rows,
                requested,
                200,
                &fingerprint,
                CursorDirection::Desc,
                |row| (row.last_seen_at.clone(), row.id),
                meta.0,
                meta.1,
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))
        })
        .await
    }
    pub async fn observatory_events(
        &self,
        run_key: String,
        query: ao::AgentEventQuery,
        cursor: Option<String>,
        requested: usize,
        asc: bool,
    ) -> ServiceResult<Page<ao::ObservatoryEventRow>> {
        #[derive(Serialize)]
        struct Bound<'a> {
            run_key: &'a str,
            query: &'a ao::AgentEventQuery,
            asc: bool,
        }
        let fingerprint = filter_fingerprint(&Bound {
            run_key: &run_key,
            query: &query,
            asc,
        })
        .map_err(cursor_error)?;
        let direction = if asc {
            CursorDirection::Asc
        } else {
            CursorDirection::Desc
        };
        let decoded = decode(cursor.as_deref(), &fingerprint, direction)?;
        self.run_db("observatory.events", move |pool| {
            let (rows, meta) = (
                ao::list_observatory_events(
                    pool,
                    &run_key,
                    &query,
                    decoded.as_ref().map(|c| (c.sort.as_str(), c.id)),
                    limit(requested, 500) + 1,
                    asc,
                )?,
                snapshot(pool)?,
            );
            page(
                rows,
                requested,
                500,
                &fingerprint,
                direction,
                |r| (r.observed_at.clone(), r.id),
                meta.0,
                meta.1,
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))
        })
        .await
    }

    pub async fn observatory_telemetry(
        &self,
        run_key: String,
        query: ao::TelemetryQuery,
        span_cursor: Option<String>,
        metric_cursor: Option<String>,
        span_limit: usize,
        metric_limit: usize,
    ) -> ServiceResult<(Page<ao::ObservatorySpanRow>, Page<ao::ObservatoryMetricRow>)> {
        #[derive(Serialize)]
        struct Bound<'a> {
            run_key: &'a str,
            query: &'a ao::TelemetryQuery,
            signal: &'static str,
        }
        let span_fp = filter_fingerprint(&Bound {
            run_key: &run_key,
            query: &query,
            signal: "spans",
        })
        .map_err(cursor_error)?;
        let metric_fp = filter_fingerprint(&Bound {
            run_key: &run_key,
            query: &query,
            signal: "metrics",
        })
        .map_err(cursor_error)?;
        let spans_after = decode(span_cursor.as_deref(), &span_fp, CursorDirection::Desc)?;
        let metrics_after = decode(metric_cursor.as_deref(), &metric_fp, CursorDirection::Desc)?;
        self.run_db("observatory.telemetry", move |pool| {
            let conn = pool.get()?;
            let run_id = conn
                .query_row(
                    "SELECT id FROM agent_runs WHERE run_key=?1",
                    [&run_key],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => {
                        anyhow::anyhow!(ServiceError::NotFound("run_not_found".into()))
                    }
                    other => anyhow::Error::from(other),
                })?;
            drop(conn);
            let span_after = spans_after
                .as_ref()
                .map(|c| c.sort.parse::<i64>().map(|sort| (sort, c.id)))
                .transpose()
                .map_err(|_| anyhow::anyhow!(CursorError::Invalid))?;
            let metric_after = metrics_after
                .as_ref()
                .map(|c| c.sort.parse::<i64>().map(|sort| (sort, c.id)))
                .transpose()
                .map_err(|_| anyhow::anyhow!(CursorError::Invalid))?;
            let spans = ao::list_observatory_spans(
                pool,
                run_id,
                &query,
                span_after,
                limit(span_limit, 500) + 1,
            )?;
            let metrics = ao::list_observatory_metrics(
                pool,
                run_id,
                &query,
                metric_after,
                limit(metric_limit, 500) + 1,
            )?;
            let meta = snapshot(pool)?;
            let span_page = page(
                spans,
                span_limit,
                500,
                &span_fp,
                CursorDirection::Desc,
                |row| (row.start_time_unix_nano.to_string(), row.id),
                meta.0.clone(),
                meta.1.clone(),
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let metric_page = page(
                metrics,
                metric_limit,
                500,
                &metric_fp,
                CursorDirection::Desc,
                |row| (row.time_unix_nano.to_string(), row.id),
                meta.0,
                meta.1,
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok((span_page, metric_page))
        })
        .await
    }
}
