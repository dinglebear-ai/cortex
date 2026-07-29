//! Canonical-entity-resolution projection glue.
//!
//! Converts deterministic entity-resolver decisions (canonical
//! `logical_service` / `service_instance` identity derived from structured
//! agent-Docker metadata) into investigation-graph rows, plus the two other
//! genuinely resolver-specific, cleanly-separable pieces that consume that
//! projection: the bounded service-topic graph walk and the legacy
//! pre-resolver topology cleanup.
//!
//! Extracted from `graph.rs` (syslog-mcp-6ipjl). The shared extraction
//! machinery this glue calls into — `EntityMemo`, `ensure_entity_memoized`,
//! `ensure_relationship_with_evidence`, `LogGraphRow`, `EvidenceInput`, and
//! the rest of the `extract_*_row` dispatch family — stays in `graph.rs`
//! because it is general-purpose (shared by every extractor, not just the
//! resolver path); moving it here would drag along the whole dispatch tree.
//! This module owns only the pieces that are cleanly separable from that
//! machinery and specific to projecting resolver decisions.

use anyhow::Result;
use serde_json::Value;

use crate::db::graph::{self, EntityMemo, EvidenceInput, GraphWalkEntity, LogGraphRow};
use crate::db::write_lock;

/// Bounded entity cap for service-topic walks (final result LIMIT).
pub const GRAPH_SERVICE_TOPIC_ENTITY_CAP: usize = 250;
/// Per-depth allowance folded into the aggregate CTE row budget of
/// service-topic walks (`ENTITY_CAP + HOP_CAP * GRAPH_WALK_MAX_DEPTH`).
/// SQLite's recursive CTE `LIMIT` is a single overall budget — there is no
/// per-level cap.
pub const GRAPH_SERVICE_TOPIC_HOP_CAP: usize = 50;

/// Relationship types a service-topic walk may traverse: only the edges
/// needed for the canonical service proof. Deliberately excludes the broad
/// log-identity edges (`observed_as`, `emitted_by`) so a service topic never
/// silently expands to all logs for the host running the service.
pub const GRAPH_SERVICE_TOPIC_RELATIONSHIPS: &[&str] = &[
    graph::REL_INSTANCE_OF,
    graph::REL_RUNS_ON,
    graph::REL_DEFINES_SERVICE,
    graph::REL_ROUTES_TO,
    graph::REL_EXPOSES_DOMAIN,
    graph::REL_MOUNTS,
    graph::REL_HAS_ARTIFACT,
    graph::REL_MATCHES_SIGNATURE,
    graph::REL_WORKED_ON,
];

/// Bounded breadth-first walk for service-topic lookups: traverses only
/// [`GRAPH_SERVICE_TOPIC_RELATIONSHIPS`], bounds the whole recursive
/// expansion at an aggregate CTE row budget of
/// `GRAPH_SERVICE_TOPIC_ENTITY_CAP + GRAPH_SERVICE_TOPIC_HOP_CAP *
/// GRAPH_WALK_MAX_DEPTH` (a single overall `LIMIT`, not a per-level cap),
/// and caps the final result at [`GRAPH_SERVICE_TOPIC_ENTITY_CAP`] entities.
///
/// Returns `(entities, truncated)`: `truncated` is `true` when the walk
/// actually reached more than [`GRAPH_SERVICE_TOPIC_ENTITY_CAP`] distinct
/// entities, so callers can tell a silently-capped neighborhood apart from an
/// exhaustive one (mirrors [`crate::db::graph::graph_around_entity`]'s
/// `truncated` signal, detected the same way: fetch one row past the cap and
/// check whether it was there).
pub fn graph_walk_service_topic(
    conn: &rusqlite::Connection,
    start_keys: &[String],
    max_depth: u8,
) -> Result<(Vec<GraphWalkEntity>, bool)> {
    if start_keys.is_empty() {
        return Ok((Vec::new(), false));
    }
    let depth = i64::from(max_depth.clamp(1, graph::GRAPH_WALK_MAX_DEPTH));
    let placeholders = vec!["?"; start_keys.len()].join(", ");
    let rel_placeholders = vec!["?"; GRAPH_SERVICE_TOPIC_RELATIONSHIPS.len()].join(", ");
    let sql = format!(
        "WITH RECURSIVE graph_walk(entity_id, depth) AS (
             SELECT id, 0 FROM graph_entities WHERE canonical_key IN ({placeholders})
             UNION
             SELECT CASE WHEN r.src_entity_id = gw.entity_id
                         THEN r.dst_entity_id ELSE r.src_entity_id END,
                    gw.depth + 1
             FROM graph_relationships r
             JOIN graph_walk gw
               ON r.src_entity_id = gw.entity_id OR r.dst_entity_id = gw.entity_id
             WHERE gw.depth < ?
               AND r.trust_level != 'refuted'
               AND r.relationship_type IN ({rel_placeholders})
             LIMIT ?
         )
         SELECT DISTINCT e.entity_type, e.canonical_key
         FROM graph_entities e
         JOIN graph_walk gw ON e.id = gw.entity_id
         LIMIT ?"
    );

    let mut bindings: Vec<rusqlite::types::Value> = start_keys
        .iter()
        .map(|k| rusqlite::types::Value::Text(k.clone()))
        .collect();
    bindings.push(rusqlite::types::Value::Integer(depth));
    for rel in GRAPH_SERVICE_TOPIC_RELATIONSHIPS {
        bindings.push(rusqlite::types::Value::Text((*rel).to_string()));
    }
    bindings.push(rusqlite::types::Value::Integer(
        (GRAPH_SERVICE_TOPIC_ENTITY_CAP
            + GRAPH_SERVICE_TOPIC_HOP_CAP * graph::GRAPH_WALK_MAX_DEPTH as usize) as i64,
    ));
    // Fetch one row past the cap so we can detect truncation (see doc
    // comment), then trim back down to the advertised cap below.
    bindings.push(rusqlite::types::Value::Integer(
        (GRAPH_SERVICE_TOPIC_ENTITY_CAP + 1) as i64,
    ));

    let mut stmt = conn.prepare(&sql)?;
    let mut entities = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok(GraphWalkEntity {
                entity_type: row.get(0)?,
                canonical_key: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = entities.len() > GRAPH_SERVICE_TOPIC_ENTITY_CAP;
    entities.truncate(GRAPH_SERVICE_TOPIC_ENTITY_CAP);
    Ok((entities, truncated))
}

/// Rows deleted per chunk by [`cleanup_legacy_service_topology`]. Mirrors the
/// repo's chunked-deletion convention (`purge_old_logs` chunks, the storage
/// enforcement `cleanup_chunk_size` default) so the write lock is released
/// between chunks instead of held across one unbounded transaction.
const LEGACY_TOPOLOGY_CLEANUP_CHUNK: i64 = 2_000;

/// Subquery selecting the stale pre-resolver entity ids: every `service`
/// entity (old `host:name` / `host:project:service` canonical keys) and
/// nested `app` labels shaped like `plex/plex/plex`.
const LEGACY_TOPOLOGY_ENTITY_IDS: &str = "SELECT id FROM graph_entities
      WHERE entity_type = 'service'
         OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')";

/// Remove stale pre-resolver service topology rows from the graph projection:
/// every `service` entity (old `host:name` / `host:project:service` canonical
/// keys) and nested `app` labels shaped like `plex/plex/plex`, plus their
/// aliases, relationships, and evidence. The canonical replacement is the
/// resolver-owned `logical_service` / `service_instance` projection; old keys
/// are deleted, never migrated.
///
/// Deletes run in [`LEGACY_TOPOLOGY_CLEANUP_CHUNK`]-row chunks, committing
/// and releasing [`write_lock`] between chunks so a large legacy projection
/// never pins the writer. The phase order (evidence → relationships →
/// aliases → entities) keeps every commit boundary referentially safe: a
/// child row is always gone before its parent.
pub fn cleanup_legacy_service_topology(conn: &mut rusqlite::Connection) -> Result<()> {
    // Same `src IN (…) OR dst IN (…)` shape for evidence and relationships:
    // an inner JOIN on both endpoints would skip relationships whose other
    // endpoint dangles, orphaning their evidence.
    let phases = [
        format!(
            "DELETE FROM graph_relationship_evidence
              WHERE id IN (
                  SELECT id FROM graph_relationship_evidence
                   WHERE relationship_id IN (
                       SELECT id FROM graph_relationships
                        WHERE src_entity_id IN ({LEGACY_TOPOLOGY_ENTITY_IDS})
                           OR dst_entity_id IN ({LEGACY_TOPOLOGY_ENTITY_IDS})
                   )
                   LIMIT ?1
              )"
        ),
        format!(
            "DELETE FROM graph_relationships
              WHERE id IN (
                  SELECT id FROM graph_relationships
                   WHERE src_entity_id IN ({LEGACY_TOPOLOGY_ENTITY_IDS})
                      OR dst_entity_id IN ({LEGACY_TOPOLOGY_ENTITY_IDS})
                   LIMIT ?1
              )"
        ),
        format!(
            "DELETE FROM graph_entity_aliases
              WHERE id IN (
                  SELECT id FROM graph_entity_aliases
                   WHERE entity_id IN ({LEGACY_TOPOLOGY_ENTITY_IDS})
                   LIMIT ?1
              )"
        ),
        format!(
            "DELETE FROM graph_entities
              WHERE id IN (SELECT id FROM ({LEGACY_TOPOLOGY_ENTITY_IDS}) LIMIT ?1)"
        ),
    ];
    for sql in &phases {
        loop {
            let deleted = {
                let _guard = write_lock();
                let tx = conn.transaction()?;
                let deleted = tx.execute(sql, [LEGACY_TOPOLOGY_CLEANUP_CHUNK])?;
                tx.commit()?;
                deleted
            };
            if (deleted as i64) < LEGACY_TOPOLOGY_CLEANUP_CHUNK {
                break;
            }
        }
    }
    Ok(())
}

/// Project canonical service identity from structured agent Docker metadata
/// (`metadata_json.agent_docker`) through the deterministic resolver. This is
/// the supported Docker identity source for the `logical_service` /
/// `service_instance` graph contract; central-pull `docker://` /
/// `docker-event://` rows are not resolver proof and are skipped here.
pub(crate) fn extract_agent_docker_row(
    conn: &rusqlite::Connection,
    row: &LogGraphRow,
    meta: Option<&Value>,
    memo: &mut EntityMemo,
) -> Result<()> {
    let observations = agent_docker_observations_from_log_row(row, meta);
    if observations.is_empty() {
        return Ok(());
    }
    let decisions = crate::db::entity_resolution::resolve_observations(&observations);
    project_resolver_decisions(conn, row, &decisions, memo)
}

/// Read `metadata_json.agent_docker` into resolver observations. Returns
/// empty when the row has no structured agent identity or is a central-pull
/// Docker row (`docker://` / `docker-event://`), which is not proof.
///
/// `meta` is the already-parsed `metadata_json` from the dispatcher
/// (`extract_log_row`) — no re-parse here. The former "cheap prefilter" byte
/// scan for `"agent_docker"` before parsing is gone because the parse now
/// always happens exactly once upstream regardless of whether this function
/// needs it.
fn agent_docker_observations_from_log_row(
    row: &LogGraphRow,
    meta: Option<&Value>,
) -> Vec<crate::db::entity_resolution::ResolverObservation> {
    if row.source_ip.starts_with("docker://") || row.source_ip.starts_with("docker-event://") {
        return Vec::new();
    }
    let Some(agent) = meta
        .and_then(|value| value.get("agent_docker"))
        .filter(|value| value.is_object())
    else {
        return Vec::new();
    };
    let text = |field: &str| {
        agent
            .get(field)
            .and_then(Value::as_str)
            .and_then(graph::normalized_value)
            .map(str::to_string)
    };
    let (Some(agent_host), Some(container_id), Some(container_name), Some(stream)) = (
        text("host"),
        text("container_id"),
        text("container_name"),
        text("stream"),
    ) else {
        return Vec::new();
    };
    let identity = crate::db::entity_resolution::AgentDockerIdentity {
        agent_host,
        container_id,
        container_name,
        compose_project: text("compose_project"),
        compose_service: text("compose_service"),
        image: text("image"),
        stream,
        observed_at: row.timestamp.clone(),
    };
    crate::db::entity_resolution::observations_from_agent_docker_identity(&identity)
}

/// Store resolver decisions as graph entities and link each
/// `service_instance` to its `logical_service` with an `instance_of` edge.
fn project_resolver_decisions(
    conn: &rusqlite::Connection,
    row: &LogGraphRow,
    decisions: &[crate::db::entity_resolution::ResolvedEntityDecision],
    memo: &mut EntityMemo,
) -> Result<()> {
    let source_id = row.id.to_string();
    let mut logical_ids = std::collections::BTreeMap::new();
    let mut instance_ids = std::collections::BTreeMap::new();
    for decision in decisions {
        let entity_id = graph::ensure_entity_memoized(
            conn,
            memo,
            decision.entity_type,
            &decision.canonical_key,
            &decision.display_label,
            graph::SOURCE_KIND_LOG,
            &source_id,
            trust_to_graph(decision.trust),
            Some(&row.timestamp),
            Some(&row.timestamp),
        )?;
        if decision.entity_type == graph::ENTITY_TYPE_LOGICAL_SERVICE {
            logical_ids.insert(decision.canonical_key.clone(), entity_id);
        } else if decision.entity_type == graph::ENTITY_TYPE_SERVICE_INSTANCE {
            instance_ids.insert(decision.canonical_key.clone(), entity_id);
        }
    }
    for (instance_key, instance_id) in instance_ids {
        if let Some((_, service)) =
            crate::db::entity_resolution::split_service_instance_key(&instance_key)
            && let Some(logical_id) = logical_ids.get(service)
        {
            graph::ensure_relationship_with_evidence(
                conn,
                instance_id,
                *logical_id,
                graph::REL_INSTANCE_OF,
                graph::REASON_RESOLVER_INSTANCE_OF,
                graph::TRUST_VERIFIED,
                1.0,
                EvidenceInput {
                    evidence_key: graph::evidence_bucket_key(
                        "log",
                        row.id,
                        graph::REASON_RESOLVER_INSTANCE_OF,
                        &row.timestamp,
                    ),
                    source_kind: graph::SOURCE_KIND_LOG,
                    source_id: &source_id,
                    source_log_id: Some(row.id),
                    source_heartbeat_id: None,
                    source_signature_hash: None,
                    observed_at: &row.timestamp,
                    reason_text: Some("resolver linked service instance to logical service"),
                    confidence_delta: 1.0,
                    trust_level: graph::TRUST_VERIFIED,
                    safe_excerpt: Some(&instance_key),
                    metadata_path: Some("metadata_json.agent_docker"),
                },
            )?;
        }
    }
    Ok(())
}

/// Map resolver trust levels onto graph trust vocabulary.
pub(crate) fn trust_to_graph(trust: crate::db::entity_resolution::ResolverTrust) -> &'static str {
    match trust {
        crate::db::entity_resolution::ResolverTrust::Verified => graph::TRUST_VERIFIED,
        crate::db::entity_resolution::ResolverTrust::Claimed => graph::TRUST_CLAIMED,
        crate::db::entity_resolution::ResolverTrust::Inferred => graph::TRUST_INFERRED,
    }
}

#[cfg(test)]
#[path = "graph_resolver_projection_tests.rs"]
mod tests;
