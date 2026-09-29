//! Bounded read of OTLP points that are not attached to an agent run.

use anyhow::Result;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::DbPool;

#[derive(Debug, Clone)]
pub struct HostMetricParams {
    pub hostname: String,
    pub service_name: Option<String>,
    pub metric_name: String,
    pub since_unix_nano: i64,
    pub before: Option<MetricCursor>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricCursor {
    pub time_unix_nano: i64,
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetricPoint {
    pub id: i64,
    pub hostname: String,
    pub service_name: Option<String>,
    pub metric_name: String,
    pub instrument_kind: String,
    pub unit: String,
    pub time_unix_nano: i64,
    pub received_at: String,
    pub value: Value,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetricsPage {
    pub points: Vec<HostMetricPoint>,
    pub next_cursor: Option<MetricCursor>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricHost {
    pub hostname: String,
    pub service_name: Option<String>,
    pub latest_time_unix_nano: i64,
}

pub fn list_metric_hosts(pool: &DbPool, since_unix_nano: i64) -> Result<Vec<MetricHost>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT hostname, service_name, MAX(time_unix_nano) FROM otel_metric_points \
         WHERE metric_name='system.memory.utilization' AND hostname!='' AND time_unix_nano>=?1 \
         GROUP BY hostname, service_name ORDER BY MAX(time_unix_nano) DESC LIMIT 100",
    )?;
    Ok(stmt
        .query_map([since_unix_nano], |row| {
            Ok(MetricHost {
                hostname: row.get(0)?,
                service_name: row.get(1)?,
                latest_time_unix_nano: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_host_metrics(pool: &DbPool, params: &HostMetricParams) -> Result<HostMetricsPage> {
    let conn = pool.get()?;
    let mut sql = String::from(
        "SELECT id,hostname,service_name,metric_name,instrument_kind,unit,\
         time_unix_nano,received_at,value_json,attributes_json \
         FROM otel_metric_points WHERE metric_name=?1 AND hostname=?2 \
         AND time_unix_nano>=?3",
    );
    let mut args = vec![
        SqlValue::Text(params.metric_name.clone()),
        SqlValue::Text(params.hostname.clone()),
        SqlValue::Integer(params.since_unix_nano),
    ];
    if let Some(service_name) = &params.service_name {
        sql.push_str(&format!(" AND service_name=?{}", args.len() + 1));
        args.push(SqlValue::Text(service_name.clone()));
    }
    if let Some(cursor) = params.before {
        sql.push_str(&format!(
            " AND (time_unix_nano,id)<(?{},?{})",
            args.len() + 1,
            args.len() + 2
        ));
        args.push(SqlValue::Integer(cursor.time_unix_nano));
        args.push(SqlValue::Integer(cursor.id));
    }
    sql.push_str(&format!(
        " ORDER BY time_unix_nano DESC,id DESC LIMIT {}",
        params.limit + 1
    ));
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement
        .query_map(rusqlite::params_from_iter(args), |row| {
            let value_json: String = row.get(8)?;
            let attributes_json: String = row.get(9)?;
            Ok(HostMetricPoint {
                id: row.get(0)?,
                hostname: row.get(1)?,
                service_name: row.get(2)?,
                metric_name: row.get(3)?,
                instrument_kind: row.get(4)?,
                unit: row.get(5)?,
                time_unix_nano: row.get(6)?,
                received_at: row.get(7)?,
                value: serde_json::from_str(&value_json).unwrap_or(Value::Null),
                attributes: serde_json::from_str(&attributes_json).unwrap_or(Value::Null),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = rows.len() > params.limit;
    rows.truncate(params.limit);
    let next_cursor = if truncated {
        rows.last().map(|point| MetricCursor {
            time_unix_nano: point.time_unix_nano,
            id: point.id,
        })
    } else {
        None
    };
    Ok(HostMetricsPage {
        points: rows,
        next_cursor,
        truncated,
    })
}

#[cfg(test)]
#[path = "host_metrics_tests.rs"]
mod tests;
