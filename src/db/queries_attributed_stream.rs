//! Physical-device log streams; session cursors retain their raw identity.

use super::*;

pub(super) fn page(
    conn: &rusqlite::Connection,
    params: &DurableStreamParams,
) -> Result<DurableStreamPage> {
    let aliases = conn
        .prepare(&super::super::queries_hosts::host_alias_select("?1"))?
        .query_map([params.hostname.as_deref().expect("device filter")], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let limit = params.limit.clamp(1, 101);
    let mut rows = Vec::new();
    let mut minimum: Option<i64> = None;
    let mut high = 0;
    let mut deleted: Option<i64> = None;
    for alias in aliases {
        for attributed in [false, true] {
            let (sql, values) = statement(params, &alias, attributed, None);
            rows.extend(
                conn.prepare(&sql)?
                    .query_map(rusqlite::params_from_iter(values.iter()), |r| {
                        let raw: String = r.get(2)?;
                        let metadata: Option<String> = r.get(6)?;
                        let hostname = crate::forwarded_host::subject_hostname(
                            &raw,
                            &r.get::<_, String>(8)?,
                            metadata.as_deref(),
                        )
                        .unwrap_or(raw);
                        Ok(DurableStreamRow {
                            id: r.get(0)?,
                            timestamp: r.get(1)?,
                            hostname,
                            severity: r.get(3)?,
                            app_name: r.get(4)?,
                            message: r.get(5)?,
                            metadata_json: metadata,
                            parse_error: r.get(7)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
            if params.include_bounds {
                for (mode, ascending) in
                    [("retained", true), ("retained", false), ("deleted", false)]
                {
                    let (sql, values) =
                        statement(params, &alias, attributed, Some((mode, ascending)));
                    let value = conn
                        .prepare(&sql)?
                        .query_map(rusqlite::params_from_iter(values.iter()), |r| {
                            r.get::<_, i64>(0)
                        })?
                        .next()
                        .transpose()?;
                    if let Some(value) = value {
                        if mode == "deleted" {
                            deleted = Some(deleted.map_or(value, |old| old.max(value)));
                        } else if ascending {
                            minimum = Some(minimum.map_or(value, |old| old.min(value)));
                        }
                        high = high.max(value);
                    }
                }
            }
        }
    }
    rows.sort_unstable_by_key(|r| r.id);
    rows.dedup_by_key(|r| r.id);
    rows.truncate(limit as usize);
    Ok(DurableStreamPage {
        rows,
        minimum_watermark: if params.include_bounds {
            deleted.map(|v| v.saturating_add(1)).or(minimum)
        } else {
            None
        },
        high_watermark: if params.include_bounds {
            high
        } else {
            params.high_watermark.unwrap_or(params.after_id)
        },
    })
}

fn statement(
    params: &DurableStreamParams,
    alias: &str,
    attributed: bool,
    bound: Option<(&str, bool)>,
) -> (String, Vec<rusqlite::types::Value>) {
    let deleted = bound.is_some_and(|(mode, _)| mode == "deleted");
    let table = if deleted {
        "stream_deleted_log_lineage"
    } else {
        "logs"
    };
    let select = if bound.is_some() {
        "l.id"
    } else {
        "l.id,l.timestamp,l.hostname,l.severity,l.app_name,l.message,l.metadata_json,l.parse_error,l.source_ip"
    };
    let mut sql = if attributed {
        let (projection, index) = if deleted {
            (
                "forwarded_deleted_log_hosts",
                "idx_forwarded_deleted_hosts_host_id",
            )
        } else {
            ("forwarded_log_hosts", "idx_forwarded_log_hosts_host_id")
        };
        format!(
            "SELECT {select} FROM {projection} a INDEXED BY {index} CROSS JOIN {table} l WHERE a.hostname=?1 AND l.id=a.log_id"
        )
    } else {
        format!("SELECT {select} FROM {table} l WHERE l.hostname=?1")
    };
    let mut values = vec![rusqlite::types::Value::Text(alias.to_string())];
    for (column, value) in [
        ("app_name", &params.app_name),
        ("severity", &params.severity),
    ] {
        if let Some(value) = value {
            values.push(rusqlite::types::Value::Text(value.clone()));
            sql.push_str(&format!(" AND l.{column}=?{}", values.len()));
        }
    }
    if deleted {
        sql.push_str(" AND l.deleted_at>=unixepoch()-900");
    }
    if bound.is_none() {
        values.push(rusqlite::types::Value::Integer(params.after_id));
        sql.push_str(&format!(" AND l.id>?{}", values.len()));
        if let Some(high) = params.high_watermark {
            values.push(rusqlite::types::Value::Integer(high));
            sql.push_str(&format!(" AND l.id<=?{}", values.len()));
        }
    }
    let id = if attributed { "a.log_id" } else { "l.id" };
    let direction = if bound.is_some_and(|(_, ascending)| !ascending) {
        "DESC"
    } else {
        "ASC"
    };
    sql.push_str(&format!(" ORDER BY {id} {direction}"));
    values.push(rusqlite::types::Value::Integer(if bound.is_some() {
        1
    } else {
        i64::from(params.limit.clamp(1, 101))
    }));
    sql.push_str(&format!(" LIMIT ?{}", values.len()));
    (sql, values)
}

#[cfg(test)]
#[path = "queries_attributed_stream_tests.rs"]
mod tests;
