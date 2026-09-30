use super::*;

// ── graph v1 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEntityLookupRequest {
    /// Adapter hint for MCP/CLI shared payloads. Service methods ignore it.
    pub mode: Option<String>,
    pub entity_id: Option<i64>,
    pub entity_type: Option<String>,
    pub key: Option<String>,
    pub alias_type: Option<String>,
    pub alias_key: Option<String>,
    /// Candidate cap for alias lookups. Default 20, clamp 1..=100.
    pub limit: Option<u32>,
    /// Accepted for response metadata symmetry; entity lookup does not hydrate
    /// relationship evidence. Default 3, clamp 0..=5.
    pub evidence_sample_limit: Option<u32>,
    /// Approximate payload budget in bytes. Default 32768, clamp 4096..=65536.
    pub payload_budget: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphAroundRequest {
    /// Adapter hint for MCP/CLI shared payloads. Service methods ignore it.
    pub mode: Option<String>,
    pub entity_id: Option<i64>,
    pub entity_type: Option<String>,
    pub key: Option<String>,
    pub alias_type: Option<String>,
    pub alias_key: Option<String>,
    /// V1 supports one-hop neighborhoods only.
    pub depth: Option<u32>,
    /// Relationship cap. Default 100, clamp 1..=500.
    pub limit: Option<u32>,
    /// Evidence samples per relationship. Default 3, clamp 0..=5.
    pub evidence_sample_limit: Option<u32>,
    /// Approximate payload budget in bytes. Default 32768, clamp 4096..=65536.
    pub payload_budget: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphExplainRequest {
    /// Adapter hint for MCP/CLI shared payloads. Service methods ignore it.
    pub mode: Option<String>,
    pub entity_id: Option<i64>,
    pub entity_type: Option<String>,
    pub key: Option<String>,
    pub alias_type: Option<String>,
    pub alias_key: Option<String>,
    /// Narrative expansion depth. Default 2, hard max 3.
    pub depth: Option<u32>,
    /// Relationships fetched per frontier entity. Default 20, clamp 1..=100.
    pub beam_width: Option<u32>,
    /// Total candidate chain cap. Default 200, clamp 1..=200.
    pub max_chains: Option<u32>,
    /// Evidence samples per relationship. Default 2, clamp 0..=5.
    pub evidence_sample_limit: Option<u32>,
    /// Approximate payload budget in bytes. Default 32768, clamp 4096..=65536.
    pub payload_budget: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEntitiesRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub entity_type: Option<String>,
    pub source_kind: Option<String>,
    pub trust_level: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphRelationshipsRequest {
    pub cursor: Option<String>,
    pub snapshot_cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphChangesRequest {
    pub cursor: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphDiscoveryMetadata {
    pub projection_status: String,
    pub source_watermark: String,
    pub last_completed_at: Option<String>,
    pub is_degraded: bool,
    pub truncated: bool,
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntitiesResponse {
    pub entities: Vec<GraphEntity>,
    pub next_cursor: Option<String>,
    pub snapshot_cursor: String,
    pub metadata: GraphDiscoveryMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphRelationshipsResponse {
    pub relationships: Vec<GraphRelationship>,
    pub next_cursor: Option<String>,
    pub snapshot_cursor: String,
    pub metadata: GraphDiscoveryMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphChange {
    pub seq: i64,
    pub object_kind: String,
    pub operation: String,
    pub item_key: String,
    pub occurred_at: String,
    pub entity: Option<GraphEntity>,
    pub relationship: Option<GraphRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphChangesResponse {
    pub changes: Vec<GraphChange>,
    pub next_cursor: String,
    pub metadata: GraphDiscoveryMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphProjectionStatusResponse {
    pub projection_status: String,
    pub last_started_at: Option<String>,
    pub last_completed_at: Option<String>,
    pub source_watermark: String,
    pub source_row_count: i64,
    pub entity_count: i64,
    pub relationship_count: i64,
    pub evidence_count: i64,
    pub is_degraded: bool,
    pub last_error: Option<String>,
    pub last_runtime_ms: i64,
    pub last_chunk_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRebuildStatsResponse {
    pub source_row_count: i64,
    pub entity_count: i64,
    pub relationship_count: i64,
    pub evidence_count: i64,
    pub source_watermark: String,
    pub runtime_ms: i64,
    pub chunk_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRebuildResponse {
    pub outcome: String,
    pub stats: Option<GraphRebuildStatsResponse>,
    pub status: GraphProjectionStatusResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntity {
    pub id: i64,
    pub entity_type: String,
    pub canonical_key: String,
    pub display_label: String,
    pub source_kind: String,
    pub source_id: String,
    pub trust_level: String,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
}

impl From<db::graph::GraphEntityRow> for GraphEntity {
    fn from(value: db::graph::GraphEntityRow) -> Self {
        Self {
            id: value.id,
            entity_type: value.entity_type,
            canonical_key: value.canonical_key,
            display_label: value.display_label,
            source_kind: value.source_kind,
            source_id: value.source_id,
            trust_level: value.trust_level,
            first_seen_at: value.first_seen_at,
            last_seen_at: value.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntityCandidate {
    pub entity: GraphEntity,
    pub match_reason: String,
    pub alias_type: Option<String>,
    pub alias_key: Option<String>,
}

impl From<db::graph::GraphEntityCandidateRow> for GraphEntityCandidate {
    fn from(value: db::graph::GraphEntityCandidateRow) -> Self {
        Self {
            entity: value.entity.into(),
            match_reason: value.match_reason,
            alias_type: value.alias_type,
            alias_key: value.alias_key,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphRelationship {
    pub id: i64,
    pub relationship_key: String,
    pub src_entity_id: i64,
    pub dst_entity_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_entity: Option<GraphEntitySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_entity: Option<GraphEntitySummary>,
    pub relationship_type: String,
    pub reason_code: String,
    pub trust_level: String,
    pub confidence: f64,
    pub evidence_count: i64,
    pub evidence_ids: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_kinds: Vec<String>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntitySummary {
    pub id: i64,
    pub entity_type: String,
    pub canonical_key: String,
    pub display_label: String,
    pub trust_level: String,
}

impl From<&GraphEntity> for GraphEntitySummary {
    fn from(value: &GraphEntity) -> Self {
        Self {
            id: value.id,
            entity_type: value.entity_type.clone(),
            canonical_key: value.canonical_key.clone(),
            display_label: value.display_label.clone(),
            trust_level: value.trust_level.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEvidence {
    pub id: i64,
    pub relationship_id: i64,
    pub source_kind: String,
    pub source_id: String,
    pub source_log_id: Option<i64>,
    pub source_heartbeat_id: Option<i64>,
    pub source_signature_hash: Option<String>,
    pub observed_at: String,
    pub reason_code: String,
    pub reason_text: Option<String>,
    pub confidence_delta: f64,
    pub trust_level: String,
    pub safe_excerpt: Option<String>,
    pub metadata_path: Option<String>,
    pub evidence_count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphEvidenceLookupRequest {
    /// Adapter hint for MCP/CLI shared payloads. Service methods ignore it.
    pub mode: Option<String>,
    pub evidence_id: i64,
    /// Approximate payload budget in bytes. Default 32768, clamp 4096..=65536.
    pub payload_budget: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphSourceLogSummary {
    pub id: i64,
    pub timestamp: String,
    pub received_at: String,
    pub hostname: String,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub source_ip: String,
    pub message: String,
    pub message_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEvidenceLookupResponse {
    pub evidence: GraphEvidence,
    pub relationship: GraphRelationship,
    pub src_entity: GraphEntitySummary,
    pub dst_entity: GraphEntitySummary,
    pub source_log_summary: Option<GraphSourceLogSummary>,
    pub missing_source_reason: Option<String>,
    pub metadata: GraphResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNextQuery {
    pub mode: String,
    pub entity_id: i64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphResponseMetadata {
    pub truncated: bool,
    pub truncated_reason: Option<String>,
    pub limit: u32,
    pub depth: u32,
    pub evidence_sample_limit: u32,
    pub payload_budget: u32,
    pub projection_status: String,
    pub last_completed_at: Option<String>,
    pub source_watermark: String,
    pub last_error: Option<String>,
    pub is_degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntityLookupResponse {
    pub resolved_entity: Option<GraphEntity>,
    pub candidates: Vec<GraphEntityCandidate>,
    pub metadata: GraphResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphAroundResponse {
    pub resolved_entity: Option<GraphEntity>,
    pub entities: Vec<GraphEntity>,
    pub relationships: Vec<GraphRelationship>,
    pub evidence: Vec<GraphEvidence>,
    pub next_queries: Vec<GraphNextQuery>,
    pub candidates: Vec<GraphEntityCandidate>,
    pub metadata: GraphResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphExplainResponse {
    pub resolved_entity: Option<GraphEntity>,
    pub narrative: Option<GraphIncidentNarrative>,
    pub chains: Vec<GraphNarrativeChain>,
    pub evidence: Vec<GraphEvidence>,
    pub open_questions: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub next_queries: Vec<GraphNextQuery>,
    pub candidates: Vec<GraphEntityCandidate>,
    pub metadata: GraphResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphIncidentNarrative {
    pub title: String,
    pub summary: String,
    pub confidence: String,
    pub relationship_ids: Vec<i64>,
    pub evidence_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNarrativeChain {
    pub chain_id: String,
    pub confidence: String,
    pub score: f64,
    pub summary: String,
    pub entities: Vec<GraphEntity>,
    pub relationships: Vec<GraphRelationship>,
    pub evidence_ids: Vec<i64>,
    pub relationship_ids: Vec<i64>,
    pub open_questions: Vec<String>,
}
