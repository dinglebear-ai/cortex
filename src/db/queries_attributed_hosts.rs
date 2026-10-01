//! Indexed per-device log reads without materializing a device's history.

use super::*;

pub(super) fn search(
    conn: &rusqlite::Connection,
    params: &SearchParams,
    limit: u32,
) -> Result<Vec<LogEntry>> {
    let mut statement = conn.prepare(&super::super::queries_hosts::host_alias_select("?1"))?;
    let aliases = statement
        .query_map([params.host.as_deref().expect("device filter")], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rows = Vec::new();
    for alias in aliases {
        for attributed in [false, true] {
            let (sql, bindings) = query_sql(params, &alias, limit, attributed);
            rows.extend(
                conn.prepare(&sql)?
                    .query_map(
                        rusqlite::params_from_iter(bindings.iter()),
                        map_attributed_row,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
    }
    rows.sort_unstable_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id)));
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| seen.insert(row.id));
    rows.truncate(limit as usize);
    Ok(rows)
}

pub(super) fn query_sql(
    params: &SearchParams,
    hostname: &str,
    limit: u32,
    attributed: bool,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = if attributed {
        format!(
            "SELECT {FTS_SELECT_COLS} FROM forwarded_log_hosts a
                 INDEXED BY idx_forwarded_log_hosts_host_time CROSS JOIN logs l
                 WHERE a.hostname=?1 AND l.id=a.log_id"
        )
    } else {
        let index = if host_only_search(params) {
            " INDEXED BY idx_logs_host_time"
        } else {
            ""
        };
        format!("SELECT {FTS_SELECT_COLS} FROM logs l{index} WHERE l.hostname=?1")
    };
    let mut bindings = vec![rusqlite::types::Value::Text(hostname.to_string())];
    let mut idx = 2;
    let mut filters = params.clone();
    filters.host = None;
    append_filters(&mut sql, &mut bindings, &mut idx, &filters);
    if let Some(query) = params.query.as_deref() {
        sql.push_str(&format!(
            " AND l.id IN (
            SELECT rowid FROM logs_fts WHERE logs_fts MATCH ?{idx}
            ORDER BY rowid DESC LIMIT {SEARCH_FTS_FAST_PATH_MATCH_CAP})"
        ));
        bindings.push(rusqlite::types::Value::Text(query.to_string()));
        idx += 1;
    }
    sql.push_str(if attributed {
        " ORDER BY a.timestamp DESC,a.log_id DESC"
    } else {
        " ORDER BY l.timestamp DESC,l.id DESC"
    });
    push_bound_limit(&mut sql, &mut bindings, &mut idx, "LIMIT", limit);
    (sql, bindings)
}
