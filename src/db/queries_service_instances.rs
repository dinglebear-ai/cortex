//! Service-instance and topic-resolution query glue: the resolver-decision
//! consumers that turn canonical `logical_service` / `service_instance`
//! graph identity, and free-text topic terms, into log query predicates.
//!
//! Extracted from `queries.rs` (syslog-mcp-6ipjl). `topic_correlate_inputs`
//! (the single entry point that ties these helpers together with the
//! general-purpose graph-walk and graph-related-entities log fan-out) stays
//! in `queries.rs` because it is the natural integration point, not
//! resolver-specific glue in its own right.

use anyhow::Result;

use crate::enrich::parser::SourceKind;

use super::entity_resolution::{INCLUSION_SERVICE_INSTANCE, ResolverStatus};
use super::graph;
use super::models::{GraphRelatedLogEntry, LogEntry, ResolvedTopicEntity};
use super::pool::DbPool;
use super::queries::{FTS_SELECT_COLS, bind_in_list, map_row};

/// Escape SQL `LIKE` wildcards (`%`, `_`) and the escape character itself
/// (`\`) so a literal value can be embedded in a pattern used with
/// `LIKE ? ESCAPE '\'`.
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Build the shared per-arm `since`/`until`/`source_kind` filter tail used by
/// the UNION ALL fan-out queries. Returns the SQL fragment (leading ` AND …`)
/// and the bindings it consumes, in order.
fn log_window_filter_tail(
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::new();
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(since) = since {
        sql.push_str(" AND l.timestamp >= ?");
        bindings.push(rusqlite::types::Value::Text(since.to_string()));
    }
    if let Some(until) = until {
        sql.push_str(" AND l.timestamp <= ?");
        bindings.push(rusqlite::types::Value::Text(until.to_string()));
    }
    if let Some(kinds) = source_kinds
        && !kinds.is_empty()
    {
        let kind_strs: Vec<String> = kinds.iter().map(|k| k.as_str().to_string()).collect();
        let ph = bind_in_list(&mut bindings, &kind_strs);
        sql.push_str(&format!(
            " AND json_extract(l.metadata_json, '$.source_kind') IN ({ph})"
        ));
    }
    (sql, bindings)
}

/// Run per-arm `UNION ALL` log queries, then merge the arms newest-first,
/// drop duplicate ids, and bound to `limit`. Each arm carries its own
/// `LIMIT` pushdown, so this reconciles them into one ordered, bounded
/// result. Empty `arms` yields no rows.
fn run_union_all_log_arms(
    conn: &rusqlite::Connection,
    arms: &[String],
    bindings: &[rusqlite::types::Value],
    limit: usize,
) -> Result<Vec<LogEntry>> {
    if arms.is_empty() {
        return Ok(Vec::new());
    }
    let sql = arms.join(" UNION ALL ");
    let mut stmt = conn.prepare(&sql)?;
    let mut logs = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    logs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id)));
    logs.dedup_by_key(|entry| entry.id);
    logs.truncate(limit);
    Ok(logs)
}

/// Fetch logs that belong to specific service instances (`host/service`
/// keys) using indexed service-scoped predicates: exact hostname AND
/// (app label equal to the service name OR starting with `{service}/`, or
/// the structured agent-docker compose service matches). This is the
/// canonical service log fan-out — it never expands to all logs on the
/// host.
pub fn search_logs_for_service_instances(
    pool: &DbPool,
    service_instance_keys: &[String],
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
    limit: usize,
) -> Result<Vec<GraphRelatedLogEntry>> {
    if service_instance_keys.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;
    let (tail_sql, tail_bindings) = log_window_filter_tail(since, until, source_kinds);
    // Per-key UNION ALL arms, each with its own LIMIT pushdown so every arm
    // is an index search on `idx_logs_host_time` (hostname = ?, timestamp
    // descending) with no full-set temp b-tree. Rows merge in Rust below.
    let mut arms: Vec<String> = Vec::new();
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    for key in service_instance_keys {
        let Some((host, service)) = super::entity_resolution::split_service_instance_key(key)
        else {
            tracing::debug!(
                key = %key,
                "discarding non-canonical service_instance key in service log fan-out"
            );
            continue;
        };
        arms.push(format!(
            "SELECT * FROM (SELECT {FTS_SELECT_COLS}
               FROM logs l
              WHERE l.hostname = ? AND (l.app_name = ? OR l.app_name LIKE ? ESCAPE '\\' \
             OR json_extract(l.metadata_json, '$.agent_docker.compose_service') = ?){tail_sql}
              ORDER BY l.timestamp DESC, l.id DESC
              LIMIT ?)"
        ));
        bindings.push(host.to_string().into());
        bindings.push(service.to_string().into());
        bindings.push(format!("{}/%", escape_like(service)).into());
        bindings.push(service.to_string().into());
        bindings.extend(tail_bindings.iter().cloned());
        bindings.push((limit as i64).into());
    }
    let entries = run_union_all_log_arms(&conn, &arms, &bindings, limit)?;
    Ok(entries
        .into_iter()
        .map(|entry| GraphRelatedLogEntry {
            entry,
            inclusion_reason: INCLUSION_SERVICE_INSTANCE.to_string(),
            resolver_status: ResolverStatus::Resolved,
            fallback_kind: None,
        })
        .collect())
}

/// Resolve the `service_instance` keys linked to the given logical services
/// via non-refuted `instance_of` edges.
pub(super) fn service_instances_of_logical_services(
    conn: &rusqlite::Connection,
    logical_keys: &[String],
) -> Result<Vec<String>> {
    if logical_keys.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; logical_keys.len()].join(", ");
    let sql = format!(
        "SELECT inst.canonical_key
           FROM graph_relationships r
           JOIN graph_entities inst ON inst.id = r.src_entity_id
           JOIN graph_entities logical ON logical.id = r.dst_entity_id
          WHERE r.relationship_type = ?
            AND r.trust_level != 'refuted'
            AND inst.entity_type = ?
            AND logical.entity_type = ?
            AND logical.canonical_key IN ({placeholders})"
    );
    let mut bindings: Vec<rusqlite::types::Value> = vec![
        graph::REL_INSTANCE_OF.to_string().into(),
        graph::ENTITY_TYPE_SERVICE_INSTANCE.to_string().into(),
        graph::ENTITY_TYPE_LOGICAL_SERVICE.to_string().into(),
    ];
    bindings.extend(
        logical_keys
            .iter()
            .map(|key| rusqlite::types::Value::Text(key.clone())),
    );
    let mut stmt = conn.prepare(&sql)?;
    let keys = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(keys)
}

/// Fetch logs by exact hostname list (bounded), used only by the explicit
/// degraded host-context fallback.
pub(super) fn search_logs_by_hostnames(
    pool: &DbPool,
    hostnames: &[String],
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
    limit: usize,
) -> Result<Vec<LogEntry>> {
    if hostnames.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;
    let (tail_sql, tail_bindings) = log_window_filter_tail(since, until, source_kinds);
    // Per-hostname UNION ALL arms with LIMIT pushdown (same shape as
    // `search_logs_for_service_instances`): each arm is an index search on
    // `idx_logs_host_time`, merged and re-truncated in Rust.
    let mut arms: Vec<String> = Vec::new();
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    for hostname in hostnames {
        arms.push(format!(
            "SELECT * FROM (SELECT {FTS_SELECT_COLS}
               FROM logs l
              WHERE l.hostname = ?{tail_sql}
              ORDER BY l.timestamp DESC, l.id DESC
              LIMIT ?)"
        ));
        bindings.push(hostname.clone().into());
        bindings.extend(tail_bindings.iter().cloned());
        bindings.push((limit as i64).into());
    }
    run_union_all_log_arms(&conn, &arms, &bindings, limit)
}

/// Escape a term for safe embedding in a SQLite `GLOB` prefix pattern, then
/// append the trailing `*` wildcard. GLOB's own metacharacters (`*`, `?`,
/// `[`) are wrapped in a single-character bracket class (e.g. `*` -> `[*]`)
/// so they match literally instead of acting as wildcards.
fn glob_prefix_pattern(term: &str) -> String {
    let mut pattern = String::with_capacity(term.len() + 1);
    for ch in term.chars() {
        match ch {
            '*' | '?' | '[' => {
                pattern.push('[');
                pattern.push(ch);
                pattern.push(']');
            }
            _ => pattern.push(ch),
        }
    }
    pattern.push('*');
    pattern
}

/// Resolve topic terms to graph entities by exact / prefix / label / alias
/// match. `terms` must already be lowercased. Strongest match wins per entity
/// (exact > prefix > label > alias). Capped per term and overall.
///
/// Query-plan / complexity notes (syslog-mcp-csukc):
/// - The exact (`canonical_key = term`) and prefix (`canonical_key` starts
///   with `term`) tiers run as a single statement using `GLOB` rather than
///   `LIKE ?1 || '%'` for the prefix condition. SQLite's LIKE-to-range-scan
///   optimization only activates under `PRAGMA case_sensitive_like = ON`
///   (a connection-wide setting we don't want to flip for one query), while
///   `GLOB` gets the equivalent range-scan optimization unconditionally.
///   `canonical_key` is always ASCII-lowercased at write time
///   (`normalize_key` in `graph.rs`) and callers already lowercase `terms`,
///   so GLOB's case sensitivity is a non-issue here. Both disjuncts hit
///   `idx_graph_entities_canonical_key` via SQLite's `MULTI-INDEX OR` plan —
///   confirmed via `EXPLAIN QUERY PLAN` — so this tier is an indexed lookup,
///   not a table scan.
/// - The label tier (`lower(display_label) LIKE '%term%'`) is a genuine
///   substring match. SQLite cannot use *any* B-tree index for a
///   leading-wildcard LIKE — that's a fundamental limitation of B-tree
///   indexes, not a missing-index problem, and no index we could add here
///   changes that. This tier is therefore an O(n) scan of `graph_entities`
///   per term where it runs.
/// - To bound the damage, the label tier's query only executes when the
///   indexed tier didn't already fill `PER_TERM_CAP` matches for that term
///   (its own `LIMIT` is `PER_TERM_CAP` minus however many indexed hits were
///   found). A term that resolves cleanly via exact/prefix match on
///   `canonical_key` skips the full scan entirely; only terms that are
///   genuinely fuzzy, or typos with few/no key matches, still pay the O(n)
///   cost — same worst case as before, no longer paid unconditionally by
///   every term regardless of match quality.
/// - This still degrades linearly with `graph_entities` row count in the
///   fuzzy case. Removing that residual cost would require a trigram/FTS5
///   index over `display_label`, which is a larger structural change than
///   this hardening pass covers.
pub(super) fn resolve_topic_entities(
    conn: &rusqlite::Connection,
    terms: &[String],
) -> Result<Vec<ResolvedTopicEntity>> {
    const PER_TERM_CAP: usize = 25;
    const TOTAL_CAP: usize = 100;
    // (entity_type, canonical_key) -> match priority (lower = stronger).
    let mut best: std::collections::HashMap<(String, String), u8> =
        std::collections::HashMap::new();

    // Tier 0/1 (exact / prefix): index-backed, see doc comment above.
    let mut key_stmt = conn.prepare(
        "SELECT entity_type, canonical_key,
                CASE WHEN canonical_key = ?1 THEN 0 ELSE 1 END AS pri
         FROM graph_entities
         WHERE canonical_key = ?1 OR canonical_key GLOB ?2
         ORDER BY pri
         LIMIT ?3",
    )?;
    // Tier 2 (label substring, fallback-only): unavoidable full scan, see
    // doc comment above. Only invoked when the indexed tier above didn't
    // already fill PER_TERM_CAP for the current term.
    let mut label_stmt = conn.prepare(
        "SELECT entity_type, canonical_key
         FROM graph_entities
         WHERE lower(display_label) LIKE '%' || ?1 || '%' ESCAPE '\\'
         LIMIT ?2",
    )?;
    let mut alias = conn.prepare(
        "SELECT e.entity_type, e.canonical_key
         FROM graph_entity_aliases a
         JOIN graph_entities e ON e.id = a.entity_id
         WHERE a.alias_key = ?1
         LIMIT ?2",
    )?;

    for term in terms {
        let glob_pattern = glob_prefix_pattern(term);
        let mut key_hits = 0usize;
        let rows = key_stmt.query_map(
            rusqlite::params![term, glob_pattern, PER_TERM_CAP as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u8,
                ))
            },
        )?;
        for row in rows {
            let (entity_type, key, pri) = row?;
            key_hits += 1;
            let slot = best.entry((entity_type, key)).or_insert(u8::MAX);
            *slot = (*slot).min(pri);
        }

        // Only pay for the unindexable substring scan when the indexed tier
        // left room under the per-term cap.
        let label_limit = PER_TERM_CAP.saturating_sub(key_hits);
        if label_limit > 0 {
            let escaped_term = escape_like(term);
            let label_rows = label_stmt
                .query_map(rusqlite::params![escaped_term, label_limit as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
            for row in label_rows {
                let (entity_type, key) = row?;
                let slot = best.entry((entity_type, key)).or_insert(u8::MAX);
                *slot = (*slot).min(2);
            }
        }

        let alias_rows = alias.query_map(rusqlite::params![term, PER_TERM_CAP as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in alias_rows {
            let (entity_type, key) = row?;
            // Alias match has priority 3 (weakest), only fills if nothing stronger.
            let slot = best.entry((entity_type, key)).or_insert(u8::MAX);
            *slot = (*slot).min(3);
        }
    }

    let mut resolved: Vec<ResolvedTopicEntity> = best
        .into_iter()
        .map(|((entity_type, canonical_key), pri)| ResolvedTopicEntity {
            entity_type,
            canonical_key,
            match_kind: match pri {
                0 => "exact",
                1 => "prefix",
                2 => "label",
                _ => "alias",
            },
            // Weak prefix/label candidates surface for the caller but never
            // drive log fan-out (deterministic resolution only).
            resolver_status: match pri {
                0 | 3 => ResolverStatus::Resolved,
                _ => ResolverStatus::Ambiguous,
            },
        })
        .collect();
    // Stable, deterministic ordering: strongest match first, then key.
    resolved.sort_by(|a, b| {
        let rank = |m: &str| match m {
            "exact" => 0,
            "prefix" => 1,
            "label" => 2,
            _ => 3,
        };
        rank(a.match_kind)
            .cmp(&rank(b.match_kind))
            .then_with(|| a.canonical_key.cmp(&b.canonical_key))
    });
    resolved.truncate(TOTAL_CAP);
    Ok(resolved)
}

#[cfg(test)]
#[path = "queries_service_instances_tests.rs"]
mod tests;
