use anyhow::Result;

use super::models::HostEntry;
use super::pool::DbPool;

/// Lowercase, trim, and strip trailing dots from a hostname so case and a
/// trailing FQDN dot don't split one machine into several host rows. Does not
/// fold FQDNs to short names — that's [`canonical_host_keys`]'s data-driven step.
fn case_fold_host(raw: &str) -> String {
    raw.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Map each input hostname to its canonical identity, applying two folds:
/// 1. **Case / trailing-dot** via [`case_fold_host`] (`BACKUPHOST` → `backuphost`).
/// 2. **FQDN → short name, only when the short name independently exists** among
///    the inputs (`nashost.<tailnet>` → `nashost` when a bare `nashost` is present,
///    but `host.docker.internal` is left alone). This never invents a merge that
///    could mask a distinct machine.
///
/// Shared by [`dedupe_hosts`] (the `hosts` action) and `clock_skew` so every
/// host-keyed view collapses the same case/FQDN variants.
pub(crate) fn canonical_host_keys(
    hostnames: &[String],
) -> std::collections::HashMap<String, String> {
    let cased: Vec<(String, String)> = hostnames
        .iter()
        .map(|h| (h.clone(), case_fold_host(h)))
        .collect();
    let shorts: std::collections::HashSet<&str> = cased
        .iter()
        .filter(|(_, c)| !c.is_empty() && !c.contains('.'))
        .map(|(_, c)| c.as_str())
        .collect();
    cased
        .iter()
        .map(|(raw, c)| {
            let canonical = match c.split_once('.') {
                Some((head, _)) if shorts.contains(head) => head.to_string(),
                _ => c.clone(),
            };
            (raw.clone(), canonical)
        })
        .collect()
}

/// Merge host rows that refer to the same machine. Two folds are applied:
/// 1. **Case / trailing-dot** — `BACKUPHOST` and `backuphost` collapse, `WINHOST`→`winhost`.
/// 2. **FQDN → short name, only when the short name independently exists** as
///    its own host. So `nashost.example.ts.net` folds into `nashost`
///    (a real host), but `host.docker.internal` is left alone because no bare
///    `host` row exists — we never invent a merge that could mask a distinct
///    machine.
///
/// Blank hostnames are excluded because they cannot be selected or correlated.
/// Other ambiguous self-identifiers (`localhost`, `host:user` forms with no
/// dot) are left untouched: resolving those to a real machine needs the
/// network-verified `source_ip`, which is a deferred follow-up.
/// Merged rows sum `log_count`, take the earliest `first_seen` and latest
/// `last_seen`, and display the canonical (lowercased) name.
pub(super) fn dedupe_hosts(rows: Vec<HostEntry>) -> Vec<HostEntry> {
    let names: Vec<String> = rows.iter().map(|h| h.hostname.clone()).collect();
    let canon = canonical_host_keys(&names);
    let mut merged: std::collections::HashMap<String, HostEntry> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for entry in rows {
        let canonical = canon
            .get(&entry.hostname)
            .cloned()
            .unwrap_or_else(|| case_fold_host(&entry.hostname));
        if canonical.is_empty() {
            continue;
        }
        match merged.get_mut(&canonical) {
            Some(acc) => {
                acc.log_count += entry.log_count;
                if entry.first_seen < acc.first_seen {
                    acc.first_seen = entry.first_seen.clone();
                }
                if entry.last_seen > acc.last_seen {
                    acc.last_seen = entry.last_seen.clone();
                }
            }
            None => {
                order.push(canonical.clone());
                merged.insert(
                    canonical.clone(),
                    HostEntry {
                        hostname: canonical.clone(),
                        first_seen: entry.first_seen.clone(),
                        last_seen: entry.last_seen.clone(),
                        log_count: entry.log_count,
                    },
                );
            }
        }
    }
    let mut out: Vec<HostEntry> = order
        .into_iter()
        .map(|k| merged.remove(&k).expect("key inserted above"))
        .collect();
    out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    out
}

/// List all known hosts with stats, deduplicated across case and FQDN variants.
pub fn list_hosts(pool: &DbPool) -> Result<Vec<HostEntry>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT hostname, first_seen, last_seen, log_count FROM hosts ORDER BY last_seen DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(HostEntry {
            hostname: row.get(0)?,
            first_seen: row.get(1)?,
            last_seen: row.get(2)?,
            log_count: row.get(3)?,
        })
    })?;

    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(dedupe_hosts(rows))
}

#[cfg(test)]
#[path = "queries_hosts_tests.rs"]
mod tests;
