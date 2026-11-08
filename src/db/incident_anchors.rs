//! Shared, host-scoped incident anchor retrieval with explicit sampling bounds.

use anyhow::Result;
use rusqlite::{Connection, types::Value};
use std::collections::{BTreeSet, HashMap};

const WINDOW_CAP: usize = 500;
const ROW_BUDGET: usize = 50_000;
const BATCH_WINDOWS: usize = 64;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct AnchorWindow {
    session: String,
    project: String,
    tool: String,
    hostname: String,
    from: String,
    to: String,
}

impl AnchorWindow {
    pub(super) fn new(
        session: &str,
        project: &str,
        tool: &str,
        hostname: &str,
        first: &str,
        last: &str,
        window_secs: i64,
    ) -> Self {
        let to = chrono::DateTime::parse_from_rfc3339(last)
            .map(|dt| {
                (dt.with_timezone(&chrono::Utc) + chrono::Duration::seconds(window_secs))
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string()
            })
            .unwrap_or_else(|_| last.to_string());
        Self {
            session: session.into(),
            project: project.into(),
            tool: tool.into(),
            hostname: hostname.into(),
            from: first.into(),
            to,
        }
    }
}

pub(super) struct AnchorRow {
    pub id: i64,
    pub message: String,
    pub metadata_json: Option<String>,
}

pub(super) struct WindowAnchors {
    pub rows: Vec<AnchorRow>,
    pub truncated: bool,
}

pub(super) struct AnchorCache {
    windows: HashMap<AnchorWindow, WindowAnchors>,
    #[cfg(test)]
    pub queries: usize,
}

impl AnchorCache {
    pub(super) fn get(&self, window: &AnchorWindow) -> &WindowAnchors {
        &self.windows[window]
    }

    pub(super) fn truncated(&self) -> bool {
        self.windows.values().any(|window| window.truncated)
    }
}

pub(super) fn load_anchors(
    conn: &Connection,
    windows: impl IntoIterator<Item = AnchorWindow>,
) -> Result<AnchorCache> {
    load_with_budget(conn, windows, ROW_BUDGET)
}

fn load_with_budget(
    conn: &Connection,
    windows: impl IntoIterator<Item = AnchorWindow>,
    mut remaining: usize,
) -> Result<AnchorCache> {
    // Stable order makes budget exhaustion deterministic, independent of HashMap order.
    let windows: Vec<_> = windows
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut cache = AnchorCache {
        windows: windows
            .iter()
            .cloned()
            .map(|window| {
                (
                    window,
                    WindowAnchors {
                        rows: Vec::new(),
                        truncated: true,
                    },
                )
            })
            .collect(),
        #[cfg(test)]
        queries: 0,
    };
    let mut offset = 0;
    while offset < windows.len() {
        let count = BATCH_WINDOWS
            .min(remaining / (WINDOW_CAP + 1))
            .min(windows.len() - offset);
        if count == 0 {
            break;
        }
        let batch = &windows[offset..offset + count];
        let sql = batch_sql(count);
        let mut bindings = Vec::with_capacity(count * 7);
        for (i, window) in batch.iter().enumerate() {
            bindings.push(Value::Integer(i as i64));
            bindings.extend(
                [
                    &window.session,
                    &window.project,
                    &window.tool,
                    &window.hostname,
                    &window.from,
                    &window.to,
                ]
                .map(|value| Value::Text(value.clone())),
            );
            cache
                .windows
                .get_mut(window)
                .expect("known window")
                .truncated = false;
        }
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(bindings.iter()))?;
        #[cfg(test)]
        {
            cache.queries += 1;
        }
        while let Some(row) = rows.next()? {
            remaining -= 1;
            let i = row.get::<_, u32>(0)? as usize;
            let anchors = cache.windows.get_mut(&batch[i]).expect("known window");
            if anchors.rows.len() == WINDOW_CAP {
                anchors.truncated = true;
            } else {
                anchors.rows.push(AnchorRow {
                    id: row.get(1)?,
                    message: row.get(2)?,
                    metadata_json: row.get(3)?,
                });
            }
        }
        offset += count;
    }
    Ok(cache)
}

fn batch_sql(count: usize) -> String {
    let values = std::iter::repeat_n("(?,?,?,?,?,?,?)", count)
        .collect::<Vec<_>>()
        .join(",");
    // Each correlated lookup is index-seekable and stops at cap+1. Avoid a
    // ROW_NUMBER scan that sorts an entire busy session before bounding it.
    format!(
        "WITH windows(i,session,project,tool,hostname,from_ts,to_ts) AS (VALUES {values})
             SELECT w.i,l.id,l.message,l.metadata_json
             FROM windows w CROSS JOIN logs l
             WHERE l.id IN (
                 SELECT a.id FROM logs a
                 WHERE a.ai_session_id=w.session AND a.ai_project=w.project
                   AND a.ai_tool=w.tool AND a.hostname=w.hostname
                   AND a.timestamp>=w.from_ts AND a.timestamp<=w.to_ts
                 ORDER BY a.timestamp ASC,a.id ASC LIMIT {}
             ) ORDER BY w.i,l.timestamp ASC,l.id ASC",
        WINDOW_CAP + 1,
    )
}

#[cfg(test)]
#[path = "incident_anchors_tests.rs"]
mod tests;
