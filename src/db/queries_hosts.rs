use anyhow::Result;

use super::models::HostEntry;
use super::pool::DbPool;

/// Lowercase, trim, and strip trailing dots from a hostname so case and a
/// trailing FQDN dot don't split one machine into several host rows. Does not
/// fold FQDNs to short names — that's [`canonical_host_keys`]'s data-driven step.
fn case_fold_host(raw: &str) -> String {
    raw.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Fold case and trailing dots, then recognize an unambiguous local or tailnet
/// alias only when its bare name is independently present. Arbitrary DNS domains
/// and competing qualified names never establish that two sources are one host.
pub(crate) fn canonical_host_keys(
    hostnames: &[String],
) -> std::collections::HashMap<String, String> {
    use std::collections::{HashMap, HashSet};
    let cased: Vec<(String, String)> = hostnames.iter()
        .map(|h| (h.clone(), case_fold_host(h))).collect();
    let shorts: HashSet<&str> = cased.iter()
        .filter(|(_, c)| !c.is_empty() && !c.contains('.'))
        .map(|(_, c)| c.as_str()).collect();
    let mut qualified: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (_, name) in &cased {
        if let Some((head, _)) = name.split_once('.') {
            qualified.entry(head).or_default().insert(name);
        }
    }
    cased.iter().map(|(raw, name)| {
        let canonical = match name.split_once('.') {
            Some((head, suffix)) if shorts.contains(head)
                && (suffix == "local" || suffix.ends_with(".ts.net"))
                && qualified.get(head).is_some_and(|names| names.len() == 1) => head.to_string(),
            _ => name.clone(),
        };
        (raw.clone(), canonical)
    }).collect()
}

/// Aggregate reported log sources using conservative hostname aliases. Counts
/// and time bounds combine, while raw spellings and source identity remain visible.
/// This is not a merge of heartbeat device IDs or authentication principals.
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
                acc.aliases.push(entry.hostname.clone());
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
                        aliases: vec![entry.hostname.clone()],
                        source_kind: super::models::HostSourceKind::for_hostname(&canonical),
                    },
                );
            }
        }
    }
    let mut out: Vec<HostEntry> = order
        .into_iter()
        .map(|k| merged.remove(&k).expect("key inserted above"))
        .collect();
    for entry in &mut out {
        entry.aliases.sort();
        entry.aliases.dedup();
    }
    out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then_with(|| a.hostname.cmp(&b.hostname)));
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
            aliases: Vec::new(),
            source_kind: Default::default(),
        })
    })?;

    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(dedupe_hosts(rows))
}

#[cfg(test)]
#[path = "queries_hosts_tests.rs"]
mod tests;
