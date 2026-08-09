use super::*;

impl CortexService {
    pub(super) async fn homelab_map_graph_answer(
        &self,
        req: &HomelabMapRequest,
    ) -> ServiceResult<Option<HomelabMapGraphAnswer>> {
        let Some(mode) = req
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
        else {
            return Ok(None);
        };
        if mode == "snapshot" {
            return Ok(None);
        }

        let (entity_type, key) = match mode {
            "host_services" => (
                "host".to_string(),
                required_map_target(req.host.as_deref(), "host", mode)?,
            ),
            "domain_routes" => (
                "domain".to_string(),
                required_map_target(req.domain.as_deref(), "domain", mode)?,
            ),
            "service_dependencies" => (
                "service_instance".to_string(),
                service_dependency_key(req.host.as_deref(), req.service.as_deref())?,
            ),
            "findings" => {
                return self.homelab_map_findings_answer(req).await.map(Some);
            }
            _ => {
                return Err(ServiceError::InvalidInput(format!(
                    "unsupported map mode `{mode}`; expected snapshot, host_services, domain_routes, service_dependencies, or findings"
                )));
            }
        };

        let graph = self
            .graph_around(GraphAroundRequest {
                mode: Some("around".to_string()),
                entity_type: Some(entity_type.clone()),
                key: Some(key.clone()),
                depth: Some(1),
                limit: req.answer_limit,
                evidence_sample_limit: req.evidence_sample_limit,
                payload_budget: req.payload_budget,
                ..Default::default()
            })
            .await?;
        let graph = if mode == "domain_routes" {
            self.expand_domain_route_graph(graph, req).await?
        } else {
            graph
        };

        Ok(Some(map_graph_answer(
            mode,
            HomelabMapGraphTarget { entity_type, key },
            graph,
        )))
    }

    async fn expand_domain_route_graph(
        &self,
        graph: GraphAroundResponse,
        req: &HomelabMapRequest,
    ) -> ServiceResult<GraphAroundResponse> {
        let proxy_ids = reverse_proxy_ids_for_domain(&graph);
        if proxy_ids.is_empty() {
            return Ok(graph);
        }

        let mut merged = graph;
        for proxy_id in proxy_ids.into_iter().take(10) {
            let proxy_graph = self
                .graph_around(GraphAroundRequest {
                    mode: Some("around".to_string()),
                    entity_id: Some(proxy_id),
                    depth: Some(1),
                    limit: req.answer_limit,
                    evidence_sample_limit: req.evidence_sample_limit,
                    payload_budget: req.payload_budget,
                    ..Default::default()
                })
                .await?;
            merge_domain_route_graph(&mut merged, proxy_graph);
        }
        Ok(merged)
    }
}

fn required_map_target(value: Option<&str>, field: &str, mode: &str) -> ServiceResult<String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServiceError::InvalidInput(format!("map mode `{mode}` requires `{field}`"))
        })?;
    Ok(value.to_string())
}

/// Resolve the `service_dependencies` target into a canonical
/// `service_instance` key (`host/service`). Legacy `host:service` /
/// `host:project:service` identities are rejected, never looked up.
fn service_dependency_key(host: Option<&str>, service: Option<&str>) -> ServiceResult<String> {
    let service = required_map_target(service, "service", "service_dependencies")?;
    if let Some(rejected) = super::graph_support::reject_legacy_service_identity(&service) {
        return Err(rejected);
    }
    if let Some((host_part, service_part)) = service.split_once('/') {
        // Canonicalize an explicit `host/service` target instead of passing
        // it through verbatim: a mixed-case `Nashost/Plex` would otherwise
        // silently miss the lowercase canonical instance key.
        return crate::db::entity_resolution::service_instance_key(host_part, service_part)
            .ok_or_else(|| {
                ServiceError::InvalidInput(format!(
                    "service_dependencies `service` value `{service}` does not canonicalize \
                     to a `host/service` instance key"
                ))
            });
    }
    let host = required_map_target(host, "host", "service_dependencies")?;
    crate::db::entity_resolution::service_instance_key(&host, service.as_str()).ok_or_else(|| {
        ServiceError::InvalidInput(
            "service_dependencies requires a non-empty host and service".into(),
        )
    })
}

fn map_graph_answer(
    mode: &str,
    target: HomelabMapGraphTarget,
    graph: GraphAroundResponse,
) -> HomelabMapGraphAnswer {
    let answer_status = if graph.resolved_entity.is_some() {
        "ok"
    } else if graph.candidates.is_empty() {
        "not_found"
    } else {
        "ambiguous"
    }
    .to_string();
    let mut rows = map_answer_rows(mode, &graph);
    let mut evidence = evidence_for_rows(graph.evidence.clone(), &rows);
    let mut metadata = graph.metadata.clone();
    apply_map_payload_budget(&mut rows, &mut evidence, &mut metadata);
    let next_queries = map_next_queries(mode, &target, &rows);
    let proof_queries = map_proof_queries(&graph, &evidence);
    HomelabMapGraphAnswer {
        mode: mode.to_string(),
        answer_status,
        target,
        rows,
        candidates: graph.candidates,
        evidence,
        metadata: metadata.clone(),
        truncation: HomelabMapAnswerTruncation {
            truncated: metadata.truncated,
            reason: metadata.truncated_reason.clone(),
            limit: metadata.limit,
            evidence_sample_limit: metadata.evidence_sample_limit,
            payload_budget: metadata.payload_budget,
        },
        degraded_reason: metadata.is_degraded.then(|| {
            metadata
                .last_error
                .clone()
                .unwrap_or_else(|| "graph_degraded".to_string())
        }),
        next_queries,
        proof_queries,
        findings: Vec::new(),
    }
}

fn map_answer_rows(mode: &str, graph: &GraphAroundResponse) -> Vec<HomelabMapAnswerRow> {
    let Some(resolved) = &graph.resolved_entity else {
        return Vec::new();
    };
    graph
        .relationships
        .iter()
        .filter_map(|relationship| map_answer_row(mode, resolved.id, relationship))
        .collect()
}

fn map_answer_row(
    mode: &str,
    resolved_entity_id: i64,
    relationship: &GraphRelationship,
) -> Option<HomelabMapAnswerRow> {
    let (entity, direction) = match mode {
        "host_services" => host_services_row(resolved_entity_id, relationship)?,
        "domain_routes" => domain_routes_row(resolved_entity_id, relationship)?,
        "service_dependencies" => service_dependencies_row(resolved_entity_id, relationship)?,
        _ => return None,
    };
    Some(HomelabMapAnswerRow {
        entity_type: entity.entity_type.clone(),
        key: entity.canonical_key.clone(),
        label: entity.display_label.clone(),
        relationship_type: relationship.relationship_type.clone(),
        direction: direction.to_string(),
        trust_level: relationship.trust_level.clone(),
        confidence: relationship.confidence,
        evidence_ids: relationship.evidence_ids.clone(),
    })
}

fn host_services_row(
    resolved_entity_id: i64,
    relationship: &GraphRelationship,
) -> Option<(&GraphEntitySummary, &'static str)> {
    if relationship.relationship_type != db::graph::REL_RUNS_ON {
        return None;
    }
    adjacent_row(resolved_entity_id, relationship)
}

fn domain_routes_row(
    resolved_entity_id: i64,
    relationship: &GraphRelationship,
) -> Option<(&GraphEntitySummary, &'static str)> {
    match relationship.relationship_type.as_str() {
        db::graph::REL_EXPOSES_DOMAIN => adjacent_row(resolved_entity_id, relationship),
        db::graph::REL_ROUTES_TO => {
            if relationship
                .src_entity
                .as_ref()
                .is_some_and(|entity| entity.entity_type == db::graph::ENTITY_TYPE_REVERSE_PROXY)
            {
                relationship
                    .dst_entity
                    .as_ref()
                    .map(|entity| (entity, "outgoing"))
            } else if relationship
                .dst_entity
                .as_ref()
                .is_some_and(|entity| entity.entity_type == db::graph::ENTITY_TYPE_REVERSE_PROXY)
            {
                relationship
                    .src_entity
                    .as_ref()
                    .map(|entity| (entity, "incoming"))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn service_dependencies_row(
    resolved_entity_id: i64,
    relationship: &GraphRelationship,
) -> Option<(&GraphEntitySummary, &'static str)> {
    let allowed = matches!(
        relationship.relationship_type.as_str(),
        db::graph::REL_DEFINES_SERVICE
            | db::graph::REL_ROUTES_TO
            | db::graph::REL_ATTACHED_TO
            | db::graph::REL_MOUNTS
            | db::graph::REL_BACKED_BY
            | db::graph::REL_HAS_ARTIFACT
    );
    allowed
        .then(|| adjacent_row(resolved_entity_id, relationship))
        .flatten()
}

fn adjacent_row(
    resolved_entity_id: i64,
    relationship: &GraphRelationship,
) -> Option<(&GraphEntitySummary, &'static str)> {
    if relationship.src_entity_id == resolved_entity_id {
        Some((relationship.dst_entity.as_ref()?, "outgoing"))
    } else if relationship.dst_entity_id == resolved_entity_id {
        Some((relationship.src_entity.as_ref()?, "incoming"))
    } else {
        None
    }
}

fn map_next_queries(
    mode: &str,
    target: &HomelabMapGraphTarget,
    rows: &[HomelabMapAnswerRow],
) -> Vec<HomelabMapNextQuery> {
    std::iter::once(target_next_query(mode, target))
        .flatten()
        .chain(
            rows.iter()
                .filter_map(|row| match row.entity_type.as_str() {
                    "host" => Some(HomelabMapNextQuery {
                        action: "map".to_string(),
                        mode: "host_services".to_string(),
                        host: Some(row.key.clone()),
                        domain: None,
                        service: None,
                        reason: format!("inspect services on {}", row.label),
                    }),
                    "domain" => Some(HomelabMapNextQuery {
                        action: "map".to_string(),
                        mode: "domain_routes".to_string(),
                        host: None,
                        domain: Some(row.key.clone()),
                        service: None,
                        reason: format!("inspect route for {}", row.label),
                    }),
                    "service" => Some(HomelabMapNextQuery {
                        action: "map".to_string(),
                        mode: "service_dependencies".to_string(),
                        host: None,
                        domain: None,
                        service: Some(row.key.clone()),
                        reason: format!("inspect dependencies for {}", row.label),
                    }),
                    _ => None,
                }),
        )
        .take(10)
        .collect()
}

fn target_next_query(mode: &str, target: &HomelabMapGraphTarget) -> Option<HomelabMapNextQuery> {
    match (mode, target.entity_type.as_str()) {
        ("host_services", "host") => Some(HomelabMapNextQuery {
            action: "map".to_string(),
            mode: mode.to_string(),
            host: Some(target.key.clone()),
            domain: None,
            service: None,
            reason: "refresh this host service view".to_string(),
        }),
        ("domain_routes", "domain") => Some(HomelabMapNextQuery {
            action: "map".to_string(),
            mode: mode.to_string(),
            host: None,
            domain: Some(target.key.clone()),
            service: None,
            reason: "refresh this domain route view".to_string(),
        }),
        ("service_dependencies", "service") => Some(HomelabMapNextQuery {
            action: "map".to_string(),
            mode: mode.to_string(),
            host: None,
            domain: None,
            service: Some(target.key.clone()),
            reason: "refresh this service dependency view".to_string(),
        }),
        _ => None,
    }
}

fn map_proof_queries(
    graph: &GraphAroundResponse,
    evidence: &[GraphEvidence],
) -> Vec<HomelabMapProofQuery> {
    let mut proof_queries = Vec::new();
    if let Some(entity) = &graph.resolved_entity {
        proof_queries.push(HomelabMapProofQuery {
            action: "graph".to_string(),
            mode: "around".to_string(),
            entity_id: Some(entity.id),
            evidence_id: None,
            label: format!("graph around {}", entity.display_label),
        });
    }
    proof_queries.extend(
        evidence
            .iter()
            .take(5)
            .map(|evidence| HomelabMapProofQuery {
                action: "graph".to_string(),
                mode: "evidence".to_string(),
                entity_id: None,
                evidence_id: Some(evidence.id),
                label: format!("evidence {}", evidence.id),
            }),
    );
    proof_queries
}

fn reverse_proxy_ids_for_domain(graph: &GraphAroundResponse) -> Vec<i64> {
    let Some(domain) = &graph.resolved_entity else {
        return Vec::new();
    };
    graph
        .relationships
        .iter()
        .filter(|rel| rel.relationship_type == db::graph::REL_EXPOSES_DOMAIN)
        .filter_map(|rel| {
            if rel.src_entity_id == domain.id
                && rel.dst_entity.as_ref().is_some_and(|entity| {
                    entity.entity_type == db::graph::ENTITY_TYPE_REVERSE_PROXY
                })
            {
                Some(rel.dst_entity_id)
            } else if rel.dst_entity_id == domain.id
                && rel.src_entity.as_ref().is_some_and(|entity| {
                    entity.entity_type == db::graph::ENTITY_TYPE_REVERSE_PROXY
                })
            {
                Some(rel.src_entity_id)
            } else {
                None
            }
        })
        .collect()
}

fn merge_domain_route_graph(base: &mut GraphAroundResponse, extra: GraphAroundResponse) {
    merge_entities(&mut base.entities, extra.entities);
    merge_relationships(&mut base.relationships, extra.relationships);
    merge_evidence(&mut base.evidence, extra.evidence);
    base.metadata.truncated |= extra.metadata.truncated;
    if base.metadata.truncated_reason.is_none() {
        base.metadata.truncated_reason = extra.metadata.truncated_reason;
    }
    base.metadata.is_degraded |= extra.metadata.is_degraded;
    if base.metadata.last_error.is_none() {
        base.metadata.last_error = extra.metadata.last_error;
    }
}

fn merge_entities(base: &mut Vec<GraphEntity>, extra: Vec<GraphEntity>) {
    let mut seen = base.iter().map(|entity| entity.id).collect::<HashSet<_>>();
    base.extend(extra.into_iter().filter(|entity| seen.insert(entity.id)));
}

fn merge_relationships(base: &mut Vec<GraphRelationship>, extra: Vec<GraphRelationship>) {
    let mut seen = base.iter().map(|rel| rel.id).collect::<HashSet<_>>();
    base.extend(extra.into_iter().filter(|rel| seen.insert(rel.id)));
}

fn merge_evidence(base: &mut Vec<GraphEvidence>, extra: Vec<GraphEvidence>) {
    let mut seen = base
        .iter()
        .map(|evidence| evidence.id)
        .collect::<HashSet<_>>();
    base.extend(
        extra
            .into_iter()
            .filter(|evidence| seen.insert(evidence.id)),
    );
}

fn evidence_for_rows(
    evidence: Vec<GraphEvidence>,
    rows: &[HomelabMapAnswerRow],
) -> Vec<GraphEvidence> {
    let row_evidence_ids = rows
        .iter()
        .flat_map(|row| row.evidence_ids.iter().copied())
        .collect::<HashSet<_>>();
    evidence
        .into_iter()
        .filter(|item| row_evidence_ids.contains(&item.id))
        .collect()
}

fn apply_map_payload_budget(
    rows: &mut Vec<HomelabMapAnswerRow>,
    evidence: &mut Vec<GraphEvidence>,
    metadata: &mut GraphResponseMetadata,
) {
    if estimated_map_answer_bytes(rows, evidence) <= metadata.payload_budget as usize {
        return;
    }
    metadata.truncated = true;
    metadata.truncated_reason = Some("payload_budget".to_string());
    while !rows.is_empty()
        && estimated_map_answer_bytes(rows, evidence) > metadata.payload_budget as usize
    {
        rows.pop();
        let ids = rows
            .iter()
            .flat_map(|row| row.evidence_ids.iter().copied())
            .collect::<HashSet<_>>();
        evidence.retain(|item| ids.contains(&item.id));
    }
}

fn estimated_map_answer_bytes(rows: &[HomelabMapAnswerRow], evidence: &[GraphEvidence]) -> usize {
    let row_bytes = rows
        .iter()
        .map(|row| {
            row.entity_type.len()
                + row.key.len()
                + row.label.len()
                + row.relationship_type.len()
                + row.direction.len()
                + row.trust_level.len()
                + row.evidence_ids.len() * std::mem::size_of::<i64>()
        })
        .sum::<usize>();
    let evidence_bytes = evidence
        .iter()
        .map(|item| {
            item.source_kind.len()
                + item.source_id.len()
                + item.reason_code.len()
                + item.reason_text.as_ref().map_or(0, String::len)
                + item.safe_excerpt.as_ref().map_or(0, String::len)
                + item.metadata_path.as_ref().map_or(0, String::len)
        })
        .sum::<usize>();
    row_bytes + evidence_bytes
}

#[cfg(test)]
#[path = "map_answers_tests.rs"]
mod tests;
