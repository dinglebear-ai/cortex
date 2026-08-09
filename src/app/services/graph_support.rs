use super::graph_limits::{ExplainPath, GraphLimits, GraphRowsModels};
use super::graph_safety::*;
use super::*;

pub(super) fn graph_rows_to_models(
    rows: db::graph::GraphAroundRows,
    payload_budget: u32,
) -> GraphRowsModels {
    let evidence: Vec<GraphEvidence> = rows
        .evidence
        .into_iter()
        .map(|row| graph_evidence_safe(row, payload_budget))
        .collect();
    let mut evidence_ids_by_relationship: HashMap<i64, Vec<i64>> = HashMap::new();
    for item in &evidence {
        evidence_ids_by_relationship
            .entry(item.relationship_id)
            .or_default()
            .push(item.id);
    }
    let entities: Vec<GraphEntity> = rows
        .entities
        .into_iter()
        .map(|row| graph_entity_safe(row.into()))
        .collect();
    let entity_summaries: HashMap<i64, GraphEntitySummary> = entities
        .iter()
        .map(|entity| (entity.id, GraphEntitySummary::from(entity)))
        .collect();
    let relationships = rows
        .relationships
        .into_iter()
        .map(|row| {
            let src_entity = entity_summaries.get(&row.src_entity_id).cloned();
            let dst_entity = entity_summaries.get(&row.dst_entity_id).cloned();
            let evidence_ids = evidence_ids_by_relationship
                .remove(&row.id)
                .unwrap_or_default();
            graph_relationship_safe(graph_relationship_to_model(
                row,
                src_entity,
                dst_entity,
                evidence_ids,
            ))
        })
        .collect();
    GraphRowsModels {
        relationships,
        entities,
        evidence,
    }
}

pub(super) fn graph_relationship_to_model(
    row: db::graph::GraphRelationshipRow,
    src_entity: Option<GraphEntitySummary>,
    dst_entity: Option<GraphEntitySummary>,
    evidence_ids: Vec<i64>,
) -> GraphRelationship {
    GraphRelationship {
        id: row.id,
        relationship_key: row.relationship_key,
        src_entity_id: row.src_entity_id,
        dst_entity_id: row.dst_entity_id,
        src_entity,
        dst_entity,
        relationship_type: row.relationship_type,
        reason_code: row.reason_code,
        trust_level: row.trust_level,
        confidence: row.confidence,
        evidence_count: row.evidence_count,
        evidence_ids,
        first_seen_at: row.first_seen_at,
        last_seen_at: row.last_seen_at,
    }
}

/// Per-observation corroboration confidence used to fold same-source evidence
/// repetition into a relationship's score (BEWA diminishing returns).
const CORROBORATION_PER_OBSERVATION: f64 = 0.3;

pub(super) fn relationship_score(
    relationship: &GraphRelationship,
    now: chrono::DateTime<Utc>,
) -> f64 {
    let trust_weight = match relationship.trust_level.as_str() {
        db::graph::TRUST_VERIFIED => 1.0,
        db::graph::TRUST_INFERRED => 0.75,
        db::graph::TRUST_CLAIMED => 0.55,
        _ => 0.4,
    };
    // The trust-weighted, temporally-decayed confidence is one independent
    // signal; same-source evidence repetition (BEWA diminishing returns) is
    // another. Combine them with noisy-OR so corroboration lifts the score
    // without ever exceeding 1.0, and stale volatile edges fall behind
    // structural ones.
    let decayed = relationship_effective_confidence(relationship, now) * trust_weight;
    let corroboration = db::graph_confidence::confidence_from_repeated(
        CORROBORATION_PER_OBSERVATION,
        relationship.evidence_count,
    );
    db::graph_confidence::noisy_or_combine(&[decayed, corroboration])
}

/// Query-time effective confidence for a relationship: the stored peak decayed
/// by its reason-code half-life since `last_seen_at`. Structural edges (λ=0) and
/// edges with an unparseable `last_seen_at` keep their stored confidence.
pub(super) fn relationship_effective_confidence(
    relationship: &GraphRelationship,
    now: chrono::DateTime<Utc>,
) -> f64 {
    let delta_hours = relationship
        .last_seen_at
        .as_deref()
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|seen| (now - seen.with_timezone(&Utc)).num_seconds().max(0) as f64 / 3600.0)
        .unwrap_or(0.0);
    let effective = db::graph_confidence::compute_effective_confidence(
        relationship.confidence,
        &relationship.reason_code,
        delta_hours,
    );
    // correlated edges are capped, refuted edges contribute nothing.
    db::graph_confidence::apply_trust_ceiling(effective, &relationship.trust_level)
}

pub(super) fn narrative_chain_from_path(
    index: usize,
    path: &ExplainPath,
    entity_map: &HashMap<i64, GraphEntity>,
    relationship_map: &HashMap<i64, GraphRelationship>,
) -> GraphNarrativeChain {
    let relationships = path
        .relationship_ids
        .iter()
        .filter_map(|id| relationship_map.get(id).cloned())
        .collect::<Vec<_>>();
    let mut entity_ids = relationships
        .iter()
        .flat_map(|rel| [rel.src_entity_id, rel.dst_entity_id])
        .collect::<Vec<_>>();
    entity_ids.sort_unstable();
    entity_ids.dedup();
    let entities = entity_ids
        .iter()
        .filter_map(|id| entity_map.get(id).cloned())
        .collect::<Vec<_>>();
    let mut evidence_ids = relationships
        .iter()
        .flat_map(|rel| rel.evidence_ids.clone())
        .collect::<Vec<_>>();
    evidence_ids.sort_unstable();
    evidence_ids.dedup();
    let confidence = confidence_from_score(path.score, relationships.len());
    let summary = chain_summary(&entities, &relationships, &confidence);
    let open_questions = if relationships
        .iter()
        .any(|rel| rel.trust_level != db::graph::TRUST_VERIFIED)
    {
        vec!["Confirm claimed or inferred identities before treating this as causal.".to_string()]
    } else {
        Vec::new()
    };
    GraphNarrativeChain {
        chain_id: format!("chain-{index}"),
        confidence,
        score: path.score,
        summary,
        entities,
        relationship_ids: relationships.iter().map(|rel| rel.id).collect(),
        relationships,
        evidence_ids,
        open_questions,
    }
}

fn confidence_from_score(score: f64, relationship_count: usize) -> String {
    // Per-relationship score is the noisy-OR of the decayed, trust-weighted
    // confidence and same-source corroboration — bounded to [0, 1].
    let normalized = if relationship_count == 0 {
        0.0
    } else {
        score / relationship_count as f64
    };
    if normalized >= 0.7 {
        "high".to_string()
    } else if normalized >= 0.45 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn chain_summary(
    entities: &[GraphEntity],
    relationships: &[GraphRelationship],
    confidence: &str,
) -> String {
    let first = entities
        .first()
        .map(entity_debug_label)
        .unwrap_or_else(|| "unknown entity".to_string());
    let last = entities
        .last()
        .map(entity_debug_label)
        .unwrap_or_else(|| "unknown entity".to_string());
    let reasons = relationships
        .iter()
        .map(|rel| rel.reason_code.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{confidence}-confidence graph evidence links {first} and {last} through {} relationship(s): {reasons}. Treat this as an evidence-backed connection, not a proven root cause.",
        relationships.len()
    )
}

fn entity_debug_label(entity: &GraphEntity) -> String {
    format!("{}:{}", entity.entity_type, entity.display_label)
}

pub(super) fn evidence_for_chains(
    chains: &[GraphNarrativeChain],
    evidence_map: &HashMap<i64, GraphEvidence>,
) -> Vec<GraphEvidence> {
    let mut ids = chains
        .iter()
        .flat_map(|chain| chain.evidence_ids.clone())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| evidence_map.get(&id).cloned())
        .collect()
}

pub(super) fn build_graph_narrative(
    root: &GraphEntity,
    chains: &[GraphNarrativeChain],
) -> Option<GraphIncidentNarrative> {
    let strongest = chains.first()?;
    if strongest.relationship_ids.is_empty() || strongest.evidence_ids.is_empty() {
        return None;
    }
    let relationship_ids = strongest.relationship_ids.clone();
    let evidence_ids = strongest.evidence_ids.clone();
    Some(GraphIncidentNarrative {
        title: format!("Graph explanation for {}", entity_debug_label(root)),
        summary: format!(
            "{} Follow the cited relationship and evidence ids, then inspect the suggested graph queries before making a causal claim.",
            strongest.summary
        ),
        confidence: strongest.confidence.clone(),
        relationship_ids,
        evidence_ids,
    })
}

pub(super) fn graph_explain_open_questions(chains: &[GraphNarrativeChain]) -> Vec<String> {
    if chains.is_empty() {
        return Vec::new();
    }
    let mut questions = Vec::new();
    if chains.iter().any(|chain| chain.confidence == "low") {
        questions.push(
            "Is there corroborating verified evidence for the low-confidence link?".to_string(),
        );
    }
    if chains
        .iter()
        .flat_map(|chain| &chain.relationships)
        .any(|rel| rel.trust_level == db::graph::TRUST_CLAIMED)
    {
        questions.push(
            "Does source_ip or heartbeat evidence corroborate the claimed hostname?".to_string(),
        );
    }
    questions
}

pub(super) fn graph_explain_missing_evidence(chains: &[GraphNarrativeChain]) -> Vec<String> {
    let mut missing = Vec::new();
    if chains.is_empty() {
        return missing;
    }
    if !chains
        .iter()
        .flat_map(|chain| &chain.relationships)
        .any(|rel| rel.trust_level == db::graph::TRUST_VERIFIED)
    {
        missing.push("verified relationship evidence".to_string());
    }
    missing
}

pub(super) fn graph_explain_next_queries(
    root: &GraphEntity,
    entity_map: &HashMap<i64, GraphEntity>,
) -> Vec<GraphNextQuery> {
    entity_map
        .values()
        .filter(|entity| entity.id != root.id)
        .take(10)
        .map(|entity| GraphNextQuery {
            mode: "around".to_string(),
            entity_id: entity.id,
            label: entity.display_label.clone(),
        })
        .collect()
}

pub(super) fn estimated_graph_explain_payload_bytes(
    chains: &[GraphNarrativeChain],
    evidence: &[GraphEvidence],
) -> usize {
    let chain_bytes: usize = chains
        .iter()
        .map(|chain| {
            chain.summary.len()
                + chain
                    .entities
                    .iter()
                    .map(|entity| entity.display_label.len() + entity.canonical_key.len())
                    .sum::<usize>()
                + chain
                    .relationships
                    .iter()
                    .map(|rel| {
                        rel.relationship_type.len()
                            + rel.reason_code.len()
                            + rel
                                .src_entity
                                .as_ref()
                                .map_or(0, estimated_entity_summary_bytes)
                            + rel
                                .dst_entity
                                .as_ref()
                                .map_or(0, estimated_entity_summary_bytes)
                    })
                    .sum::<usize>()
        })
        .sum();
    let evidence_bytes: usize = evidence
        .iter()
        .map(|item| {
            item.source_id.len()
                + item.reason_text.as_deref().unwrap_or("").len()
                + item.safe_excerpt.as_deref().unwrap_or("").len()
        })
        .sum();
    chain_bytes + evidence_bytes
}

pub(super) fn graph_projection_status_response(
    status: db::graph::GraphProjectionStatus,
) -> GraphProjectionStatusResponse {
    GraphProjectionStatusResponse {
        projection_status: status.projection_status,
        last_started_at: status.last_started_at,
        last_completed_at: status.last_completed_at,
        source_watermark: status.source_watermark,
        source_row_count: status.source_row_count,
        entity_count: status.entity_count,
        relationship_count: status.relationship_count,
        evidence_count: status.evidence_count,
        is_degraded: status.is_degraded,
        last_error: status.last_error.map(redact_graph_text),
        last_runtime_ms: status.last_runtime_ms,
        last_chunk_count: status.last_chunk_count,
    }
}

pub(super) fn graph_rebuild_stats_response(
    stats: db::graph::GraphRebuildStats,
) -> GraphRebuildStatsResponse {
    GraphRebuildStatsResponse {
        source_row_count: stats.source_row_count,
        entity_count: stats.entity_count,
        relationship_count: stats.relationship_count,
        evidence_count: stats.evidence_count,
        source_watermark: stats.source_watermark,
        runtime_ms: stats.runtime_ms,
        chunk_count: stats.chunk_count,
    }
}

/// Reject legacy nested service identity keys (`nashost:plex`,
/// `nashost:plex:plex`, `plex/plex/plex`) on service-identity lookups before
/// any graph query runs. The check is scoped to service-identity entity
/// types: other entity types (`ai_session`, `user`, `git_commit`, `storage`)
/// legitimately use `:`-separated key grammars and must not be rejected.
pub(super) fn legacy_service_identity_rejection(
    entity_type: &str,
    key: &str,
) -> Option<ServiceError> {
    let service_identity = matches!(
        entity_type,
        db::graph::ENTITY_TYPE_LOGICAL_SERVICE
            | db::graph::ENTITY_TYPE_SERVICE_INSTANCE
            | db::graph::ENTITY_TYPE_SERVICE
            | db::graph::ENTITY_TYPE_APP
    );
    if !service_identity {
        return None;
    }
    reject_legacy_service_identity(key)
}

/// Canonical rejection for legacy (pre entity-resolution) service identity
/// shapes: the single source of the `rejected_legacy_shape` error wording.
/// Returns `None` for canonical keys and free text. Callers that only reject
/// specific entity types must gate before calling (see
/// [`legacy_service_identity_rejection`]).
pub(super) fn reject_legacy_service_identity(key: &str) -> Option<ServiceError> {
    let diagnostic = db::entity_resolution::diagnose_lookup_input(key);
    (diagnostic.status == db::entity_resolution::ResolverStatus::RejectedLegacyShape).then(|| {
        ServiceError::InvalidInput(format!(
            "unsupported legacy graph service identity `{key}`: rejected_legacy_shape"
        ))
    })
}

pub(super) fn validate_graph_entity_type(entity_type: &str) -> ServiceResult<()> {
    if db::graph::is_known_entity_type(entity_type) {
        Ok(())
    } else {
        Err(ServiceError::InvalidInput(format!(
            "unsupported graph entity_type '{entity_type}'"
        )))
    }
}

pub(super) fn graph_metadata(
    status: &db::graph::GraphProjectionStatus,
    limits: GraphLimits,
    truncated: bool,
    truncated_reason: Option<String>,
) -> GraphResponseMetadata {
    GraphResponseMetadata {
        truncated,
        truncated_reason,
        limit: limits.limit,
        depth: limits.depth,
        evidence_sample_limit: limits.evidence_sample_limit,
        payload_budget: limits.payload_budget,
        projection_status: status.projection_status.clone(),
        last_completed_at: status.last_completed_at.clone(),
        source_watermark: status.source_watermark.clone(),
        last_error: status.last_error.clone().map(redact_graph_text),
        is_degraded: status.is_degraded,
    }
}
