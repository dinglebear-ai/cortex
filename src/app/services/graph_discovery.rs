use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::graph_safety::{graph_entity_safe, graph_relationship_safe, redact_graph_text};
use super::graph_support::graph_relationship_to_model;
use super::*;

const MAX_PAGE: u32 = 25;
const MAX_RESPONSE_BYTES: usize = 65_536;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    kind: String,
    anchor: i64,
    after: i64,
    filter: String,
}

fn encode_cursor(kind: &str, anchor: i64, after: i64, filter: &str) -> String {
    hex::encode(
        serde_json::to_vec(&Cursor {
            version: 1,
            kind: kind.into(),
            anchor,
            after,
            filter: filter.into(),
        })
        .expect("cursor is serializable"),
    )
}

fn decode_cursor(encoded: &str, kind: &str, filter: &str) -> ServiceResult<Cursor> {
    if encoded.len() > 512 {
        return Err(ServiceError::InvalidInput("graph cursor too long".into()));
    }
    let raw = hex::decode(encoded)
        .map_err(|_| ServiceError::InvalidInput("invalid graph cursor".into()))?;
    let cursor: Cursor = serde_json::from_slice(&raw)
        .map_err(|_| ServiceError::InvalidInput("invalid graph cursor".into()))?;
    if cursor.version != 1
        || cursor.kind != kind
        || cursor.filter != filter
        || cursor.anchor < 0
        || cursor.after < 0
    {
        return Err(ServiceError::InvalidInput(
            "graph cursor does not match this request".into(),
        ));
    }
    Ok(cursor)
}

fn page_limit(limit: Option<u32>) -> ServiceResult<u32> {
    let limit = limit.unwrap_or(MAX_PAGE);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(ServiceError::InvalidInput(
            "graph page limit must be 1..=25".into(),
        ));
    }
    Ok(limit)
}

fn metadata(
    projection_status: String,
    source_watermark: String,
    last_completed_at: Option<String>,
    is_degraded: bool,
    truncated: bool,
) -> GraphDiscoveryMetadata {
    GraphDiscoveryMetadata {
        projection_status,
        source_watermark,
        last_completed_at,
        is_degraded,
        truncated,
        recovery: None,
    }
}

fn relation(item: db::graph_discovery::RelationshipItem) -> GraphRelationship {
    let mut relationship = graph_relationship_to_model(item.row, None, None, item.evidence_ids);
    relationship.source_kinds = item.source_kinds;
    graph_relationship_safe(relationship)
}

impl CortexService {
    pub async fn graph_entities(
        &self,
        req: GraphEntitiesRequest,
    ) -> ServiceResult<GraphEntitiesResponse> {
        let limit = page_limit(req.limit)?;
        let kind = req.entity_type.unwrap_or_default();
        let source = req.source_kind.unwrap_or_default();
        let trust = req.trust_level.unwrap_or_default();
        let query = req.query.unwrap_or_default().trim().to_owned();
        if (!kind.is_empty() && !db::graph::is_known_entity_type(&kind))
            || (!trust.is_empty()
                && !["verified", "claimed", "inferred", "correlated", "refuted"]
                    .contains(&trust.as_str()))
            || source.len() > 64
            || !source
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_:-.".contains(c))
            || query.len() > 80
            || query.chars().any(char::is_control)
        {
            return Err(ServiceError::InvalidInput(
                "invalid graph inventory filter".into(),
            ));
        }
        let fingerprint = hex::encode(Sha256::digest(
            format!("{kind}\0{source}\0{trust}\0{query}").as_bytes(),
        ));
        let cursor = req
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, "entities", &fingerprint))
            .transpose()?;
        let after = cursor.as_ref().map_or(0, |cursor| cursor.after);
        let expected_anchor = cursor.as_ref().map(|cursor| cursor.anchor);
        self.with_heavy_read_permit("graph.entities", || async move {
            let page = self
                .run_db("graph.entities", move |pool| {
                    db::graph_discovery::entities(
                        pool,
                        after,
                        expected_anchor,
                        limit,
                        db::graph_discovery::EntityFilters {
                            entity_type: &kind,
                            source_kind: &source,
                            trust_level: &trust,
                            query: &query,
                        },
                    )
                })
                .await?;
            if page.changed {
                return Err(ServiceError::Conflict(
                    "graph snapshot changed during pagination".into(),
                ));
            }
            let next_cursor = page.has_more.then(|| {
                encode_cursor(
                    "entities",
                    page.anchor,
                    page.rows.last().expect("nonempty when more").id,
                    &fingerprint,
                )
            });
            let mut response = GraphEntitiesResponse {
                entities: page
                    .rows
                    .into_iter()
                    .map(|row| graph_entity_safe(row.into()))
                    .collect(),
                next_cursor,
                snapshot_cursor: encode_cursor("changes", page.anchor, page.anchor, ""),
                metadata: metadata(
                    page.projection_status,
                    page.source_watermark,
                    page.last_completed_at,
                    page.is_degraded,
                    page.has_more,
                ),
            };
            while serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() > MAX_RESPONSE_BYTES)
            {
                response.entities.pop();
                let last = response.entities.last().ok_or_else(|| {
                    ServiceError::Internal(anyhow::anyhow!("graph entity exceeds response budget"))
                })?;
                response.next_cursor = Some(encode_cursor(
                    "entities",
                    page.anchor,
                    last.id,
                    &fingerprint,
                ));
                response.metadata.truncated = true;
            }
            Ok(response)
        })
        .await
    }

    pub async fn graph_relationships(
        &self,
        req: GraphRelationshipsRequest,
    ) -> ServiceResult<GraphRelationshipsResponse> {
        let limit = page_limit(req.limit)?;
        let cursor = req
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, "relationships", ""))
            .transpose()?;
        let snapshot = req
            .snapshot_cursor
            .as_deref()
            .map(|value| decode_cursor(value, "changes", ""))
            .transpose()?;
        if let (Some(cursor), Some(snapshot)) = (&cursor, &snapshot)
            && cursor.anchor != snapshot.anchor
        {
            return Err(ServiceError::InvalidInput(
                "graph snapshot cursors disagree".into(),
            ));
        }
        let after = cursor.as_ref().map_or(0, |cursor| cursor.after);
        let expected_anchor = cursor
            .as_ref()
            .map(|cursor| cursor.anchor)
            .or_else(|| snapshot.as_ref().map(|cursor| cursor.anchor));
        self.with_heavy_read_permit("graph.relationships", || async move {
            let page = self
                .run_db("graph.relationships", move |pool| {
                    db::graph_discovery::relationships(pool, after, expected_anchor, limit)
                })
                .await?;
            if page.changed {
                return Err(ServiceError::Conflict(
                    "graph snapshot changed during pagination".into(),
                ));
            }
            let next_cursor = page.has_more.then(|| {
                encode_cursor(
                    "relationships",
                    page.anchor,
                    page.rows.last().expect("nonempty when more").row.id,
                    "",
                )
            });
            let mut response = GraphRelationshipsResponse {
                relationships: page.rows.into_iter().map(relation).collect(),
                next_cursor,
                snapshot_cursor: encode_cursor("changes", page.anchor, page.anchor, ""),
                metadata: metadata(
                    page.projection_status,
                    page.source_watermark,
                    page.last_completed_at,
                    page.is_degraded,
                    page.has_more,
                ),
            };
            while serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() > MAX_RESPONSE_BYTES)
            {
                response.relationships.pop();
                let last = response.relationships.last().ok_or_else(|| {
                    ServiceError::Internal(anyhow::anyhow!(
                        "graph relationship exceeds response budget"
                    ))
                })?;
                response.next_cursor =
                    Some(encode_cursor("relationships", page.anchor, last.id, ""));
                response.metadata.truncated = true;
            }
            Ok(response)
        })
        .await
    }

    pub async fn graph_changes(
        &self,
        req: GraphChangesRequest,
    ) -> ServiceResult<GraphChangesResponse> {
        let limit = page_limit(req.limit)?;
        let cursor = decode_cursor(&req.cursor, "changes", "")?;
        if cursor.after != cursor.anchor {
            return Err(ServiceError::InvalidInput("invalid change cursor".into()));
        }
        self.with_heavy_read_permit("graph.changes", || async move {
            let page = self
                .run_db("graph.changes", move |pool| {
                    db::graph_discovery::changes(pool, cursor.after, limit)
                })
                .await?;
            if page.expired {
                return Err(ServiceError::Gone("graph change cursor expired".into()));
            }
            let next = page.rows.last().map_or(page.latest, |event| event.seq);
            let mut response = GraphChangesResponse {
                changes: page
                    .rows
                    .into_iter()
                    .map(|event| GraphChange {
                        seq: event.seq,
                        object_kind: event.object_kind,
                        operation: event.operation,
                        item_key: redact_graph_text(event.item_key),
                        occurred_at: event.occurred_at,
                        entity: event.entity.map(|row| graph_entity_safe(row.into())),
                        relationship: event.relationship.map(|row| {
                            relation(db::graph_discovery::RelationshipItem {
                                row,
                                evidence_ids: event.evidence_ids,
                                source_kinds: event.source_kinds,
                            })
                        }),
                    })
                    .collect(),
                next_cursor: encode_cursor("changes", next, next, ""),
                metadata: metadata(
                    page.projection_status,
                    page.source_watermark,
                    page.last_completed_at,
                    page.is_degraded,
                    page.has_more,
                ),
            };
            while serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() > MAX_RESPONSE_BYTES)
            {
                response.changes.pop();
                let last = response.changes.last().ok_or_else(|| {
                    ServiceError::Internal(anyhow::anyhow!("graph change exceeds response budget"))
                })?;
                response.next_cursor = encode_cursor("changes", last.seq, last.seq, "");
                response.metadata.truncated = true;
            }
            Ok(response)
        })
        .await
    }
}
