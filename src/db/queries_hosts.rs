use anyhow::Result;

use super::models::{HostEntry, HostSourceKind};
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
    let cased: Vec<(String, String)> = hostnames
        .iter()
        .map(|h| {
            (
                h.clone(),
                if HostSourceKind::for_hostname(h) == HostSourceKind::ForwardingPrincipal {
                    h.trim().to_string()
                } else {
                    case_fold_host(h)
                },
            )
        })
        .collect();
    let shorts: HashSet<&str> = cased
        .iter()
        .filter(|(_, c)| !c.is_empty() && !c.contains('.'))
        .map(|(_, c)| c.as_str())
        .collect();
    let mut qualified: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (_, name) in &cased {
        if let Some((head, _)) = name.split_once('.') {
            qualified.entry(head).or_default().insert(name);
        }
    }
    cased
        .iter()
        .map(|(raw, name)| {
            let canonical = match name.split_once('.') {
                Some((head, suffix))
                    if HostSourceKind::for_hostname(raw) != HostSourceKind::ForwardingPrincipal
                        && shorts.contains(head)
                        && (suffix == "local" || suffix.ends_with(".ts.net"))
                        && qualified.get(head).is_some_and(|names| names.len() == 1) =>
                {
                    head.to_string()
                }
                _ => name.clone(),
            };
            (raw.clone(), canonical)
        })
        .collect()
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
                if entry.source_kind == HostSourceKind::ClaimedHost {
                    acc.source_kind = HostSourceKind::ClaimedHost;
                }
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
                        host_id: None,
                        first_seen: entry.first_seen.clone(),
                        last_seen: entry.last_seen.clone(),
                        log_count: entry.log_count,
                        aliases: vec![entry.hostname.clone()],
                        source_kind: if entry.source_kind == HostSourceKind::ClaimedHost {
                            HostSourceKind::ClaimedHost
                        } else {
                            HostSourceKind::for_hostname(&canonical)
                        },
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
    out.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| a.hostname.cmp(&b.hostname))
    });
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
            host_id: None,
            first_seen: row.get(1)?,
            last_seen: row.get(2)?,
            log_count: row.get(3)?,
            aliases: Vec::new(),
            source_kind: Default::default(),
        })
    })?;

    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // Raw source totals remain intact: source-level filters include every row
    // received under that principal, including rows also shown by claimed
    // device name. These two views overlap rather than partitioning evidence.
    for attributed in super::host_attribution::list_forwarded_host_counts(&conn)? {
        rows.push(HostEntry {
            hostname: attributed.hostname,
            host_id: None,
            first_seen: attributed.first_seen,
            last_seen: attributed.last_seen,
            log_count: attributed.log_count,
            aliases: Vec::new(),
            source_kind: HostSourceKind::ClaimedHost,
        });
    }
    rows.retain(|row| row.log_count > 0);
    let mut rows = dedupe_hosts(rows);
    for row in &mut rows {
        if row.source_kind == HostSourceKind::Host {
            match super::heartbeat::resolve_unique_hostname(&conn, &row.hostname) {
                Ok(host_id) => row.host_id = Some(host_id),
                Err(error)
                    if matches!(error.to_string().as_str(), "not_found" | "ambiguous_host") => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(rows)
}

/// Inventory and indexed log filters must use the same conservative alias
/// contract. Active aggregates define current ambiguity exactly as list_hosts
/// does; catalog-only deleted names retain exact stream lineage access without
/// changing that grouping. Safe local/tailnet tombstones can join a bare group
/// only when no active qualified name conflicts. No raw history scan is needed.
pub(super) fn host_alias_select(parameter: &str) -> String {
    let normalize = |column: &str| {
        format!(
            "CASE WHEN lower(trim({column})) IN ('agent-shared_bearer','agent-loopback')
                      OR lower(trim({column})) LIKE 'bearer-shared-%'
                  THEN trim({column}) ELSE lower(rtrim(trim({column}), '.')) END"
        )
    };
    let requested = normalize(parameter);
    let normalized = normalize("hostname");
    format!(
        "WITH active AS (
             SELECT h.hostname FROM hosts h
             WHERE h.log_count > COALESCE((SELECT SUM(f.log_count)
                 FROM forwarded_host_counts f WHERE f.original_hostname=h.hostname),0)
             UNION SELECT hostname FROM forwarded_host_counts
         ), active_cased AS (
             SELECT hostname, {normalized} AS normalized FROM active
         ), active_mapped AS (
             SELECT h.hostname,h.normalized,
                 CASE WHEN instr(h.normalized, '.') > 0
                     AND (substr(h.normalized, instr(h.normalized, '.')) = '.local'
                          OR substr(h.normalized, -7) = '.ts.net')
                     AND EXISTS (SELECT 1 FROM active_cased bare
                         WHERE bare.normalized = substr(h.normalized, 1, instr(h.normalized, '.')-1))
                     AND (SELECT COUNT(DISTINCT q.normalized) FROM active_cased q
                         WHERE instr(q.normalized, '.') > 0
                           AND substr(q.normalized, 1, instr(q.normalized, '.')-1)
                               = substr(h.normalized, 1, instr(h.normalized, '.')-1)) = 1
                 THEN substr(h.normalized, 1, instr(h.normalized, '.')-1)
                 ELSE h.normalized END AS canonical
             FROM active_cased h
         ), known AS (
             SELECT hostname FROM hosts
             UNION SELECT hostname FROM forwarded_host_names
         ), cased AS (
             SELECT hostname, {normalized} AS normalized FROM known
         ), mapped AS (
             SELECT h.hostname,h.normalized,COALESCE(
                 (SELECT canonical FROM active_mapped a WHERE a.normalized=h.normalized LIMIT 1),
                 CASE WHEN instr(h.normalized,'.')>0
                     AND (substr(h.normalized,instr(h.normalized,'.'))='.local'
                          OR substr(h.normalized,-7)='.ts.net')
                     AND EXISTS(SELECT 1 FROM cased bare
                         WHERE bare.normalized=substr(h.normalized,1,instr(h.normalized,'.')-1))
                     AND NOT EXISTS(SELECT 1 FROM active_cased q
                         WHERE instr(q.normalized,'.')>0
                           AND substr(q.normalized,1,instr(q.normalized,'.')-1)
                               =substr(h.normalized,1,instr(h.normalized,'.')-1))
                     AND (EXISTS(SELECT 1 FROM active_cased bare
                         WHERE bare.normalized=substr(h.normalized,1,instr(h.normalized,'.')-1))
                       OR (SELECT COUNT(DISTINCT q.normalized) FROM cased q
                         WHERE instr(q.normalized,'.')>0
                           AND substr(q.normalized,1,instr(q.normalized,'.')-1)
                               =substr(h.normalized,1,instr(h.normalized,'.')-1)
                           AND (substr(q.normalized,instr(q.normalized,'.'))='.local'
                                OR substr(q.normalized,-7)='.ts.net'))=1)
                 THEN substr(h.normalized,1,instr(h.normalized,'.')-1)
                 ELSE h.normalized END) AS canonical
             FROM cased h
         ) SELECT hostname FROM mapped
           WHERE canonical = COALESCE(
               (SELECT canonical FROM mapped WHERE normalized = {requested} LIMIT 1),
               {requested})"
    )
}

#[cfg(test)]
#[path = "queries_hosts_tests.rs"]
mod tests;
