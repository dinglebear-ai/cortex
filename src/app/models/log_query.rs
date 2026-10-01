use super::*;
use crate::inventory::schema::{
    ArtifactRef, CollectionError, ComposeProject, InventoryFreshness, InventoryService,
    MediaService, NetworkSegment, ProjectRepo, ReverseProxyRoute, StorageSummary,
};
use std::collections::BTreeMap;

pub mod topology_findings {
    pub const TYPE_POTENTIAL_PUBLIC_ROUTE: &str = "potential_public_route";
    pub const TYPE_RISKY_MOUNTS: &str = "risky_mounts";
    pub const TYPE_COLLECTOR_HEALTH: &str = "collector_health";
    pub const TYPES: [&str; 3] = [
        TYPE_POTENTIAL_PUBLIC_ROUTE,
        TYPE_RISKY_MOUNTS,
        TYPE_COLLECTOR_HEALTH,
    ];

    pub const SEVERITY_CRITICAL: &str = "critical";
    pub const SEVERITY_HIGH: &str = "high";
    pub const SEVERITY_MEDIUM: &str = "medium";
    pub const SEVERITY_LOW: &str = "low";
    pub const SEVERITY_INFO: &str = "info";

    pub mod reason {
        pub const REVERSE_PROXY_ROUTE_CONFIGURED: &str = "reverse_proxy_route_configured";
        pub const REVERSE_PROXY_DOMAIN_WITHOUT_TARGET_PROOF: &str =
            "reverse_proxy_domain_without_target_proof";
        pub const DOCKER_SOCKET_MOUNT: &str = "docker_socket_mount";
        pub const HOST_ROOT_MOUNT: &str = "host_root_mount";
        pub const APPDATA_ROOT_MOUNT: &str = "appdata_root_mount";
        pub const MOUNT_MISSING_SOURCE_DETAIL: &str = "mount_missing_source_detail";
        pub const GRAPH_PROJECTION_NOT_READY: &str = "graph_projection_not_ready";
        pub const INVENTORY_CACHE_MISSING: &str = "inventory_cache_missing";
        pub const INVENTORY_CACHE_STALE: &str = "inventory_cache_stale";
        pub const INVENTORY_CACHE_UNREADABLE: &str = "inventory_cache_unreadable";
        pub const COLLECTION_STATE_UNAVAILABLE: &str = "collection_state_unavailable";
        pub const COLLECTOR_DEGRADED: &str = "collector_degraded";
        pub const COLLECTOR_PARTIAL: &str = "collector_partial";
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsRequest {
    pub query: Option<String>,
    pub host: Option<String>,
    pub source: Option<String>,
    pub severity: Option<String>,
    pub app: Option<String>,
    pub facility: Option<String>,
    pub exclude_facility: Option<String>,
    pub process_id: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub received_since: Option<String>,
    pub received_until: Option<String>,
    pub limit: Option<u32>,
    pub source_kind: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub container: Option<String>,
    pub docker_host: Option<String>,
    pub stream: Option<String>,
    pub event_action: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterLogsRequest {
    pub host: Option<String>,
    pub source: Option<String>,
    pub severity: Option<String>,
    pub app: Option<String>,
    pub facility: Option<String>,
    pub exclude_facility: Option<String>,
    pub process_id: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub received_since: Option<String>,
    pub received_until: Option<String>,
    pub limit: Option<u32>,
    pub source_kind: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub container: Option<String>,
    pub docker_host: Option<String>,
    pub stream: Option<String>,
    pub event_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchLogsResponse {
    pub count: usize,
    pub logs: Vec<LogEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailLogsRequest {
    pub host: Option<String>,
    pub source: Option<String>,
    pub app: Option<String>,
    /// Minimum severity to return (e.g. `warning` returns warning + worse).
    pub severity_min: Option<String>,
    pub n: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSummaryEntry {
    pub hostname: String,
    /// Optional secondary grouping key (e.g. app_name) when `group_by` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    pub severity: String,
    pub count: i64,
}

impl From<db::ErrorSummaryEntry> for ErrorSummaryEntry {
    fn from(value: db::ErrorSummaryEntry) -> Self {
        Self {
            hostname: value.hostname,
            app_name: value.app_name,
            severity: value.severity,
            count: value.count,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetErrorsRequest {
    pub since: Option<String>,
    pub until: Option<String>,
    /// Secondary grouping key. Currently supports `app_name`.
    pub group_by: Option<String>,
    /// Max summary rows to return. Defaults to all rows; clamped by service.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetErrorsResponse {
    pub summary: Vec<ErrorSummaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    pub hostname: String,
    #[serde(default)]
    pub host_id: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub log_count: i64,
    /// Raw stored spellings retained for exact filters; no evidence is rewritten.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub source_kind: db::HostSourceKind,
}

impl From<db::HostEntry> for HostEntry {
    fn from(value: db::HostEntry) -> Self {
        Self {
            hostname: value.hostname,
            first_seen: value.first_seen,
            last_seen: value.last_seen,
            log_count: value.log_count,
            aliases: value.aliases,
            source_kind: value.source_kind,
            host_id: value.host_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHostsResponse {
    pub hosts: Vec<HostEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomelabMapRequest {
    /// Optional graph-backed answer mode. Omit or use `snapshot` for the
    /// legacy inventory snapshot.
    pub mode: Option<String>,
    /// Target host for mode=host_services or service_dependencies.
    pub host: Option<String>,
    /// Target domain for mode=domain_routes.
    pub domain: Option<String>,
    /// Target service for mode=service_dependencies. Use `host:name` or pass
    /// `host` separately with a bare service name.
    pub service: Option<String>,
    /// Maximum host nodes to return. Default 100, max 500.
    pub host_limit: Option<u32>,
    /// Deprecated map v1 compatibility option. Map v2 ignores it and reports
    /// a request warning when collection_errors are included.
    pub per_host_limit: Option<u32>,
    /// Optional top-level inventory sections to include.
    pub include_sections: Option<Vec<String>>,
    /// Per-section item cap. Default 100, max 250.
    pub section_limit: Option<u32>,
    /// Graph relationship cap for graph-backed modes. Default 100, max 500.
    pub answer_limit: Option<u32>,
    /// Evidence samples per relationship for graph-backed modes. Default 3, max 5.
    pub evidence_sample_limit: Option<u32>,
    /// Approximate graph payload budget in bytes. Default 32768, max 65536.
    pub payload_budget: Option<u32>,
    /// Finding cap for mode=findings. Default 25, max 100.
    pub finding_limit: Option<u32>,
    /// Evidence samples per finding for mode=findings. Default 2, max 5.
    pub evidence_per_finding: Option<u32>,
    /// Optional finding types for mode=findings. Defaults to all supported
    /// finding types: potential_public_route, risky_mounts, collector_health.
    pub finding_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapResponse {
    pub schema: String,
    pub generated_at: String,
    pub cache_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<InventoryFreshness>,
    pub summary: HomelabMapSummary,
    pub nodes: Vec<HomelabMapNode>,
    pub services: Vec<InventoryService>,
    pub compose_projects: Vec<ComposeProject>,
    pub reverse_proxies: Vec<ReverseProxyRoute>,
    pub networks: Vec<NetworkSegment>,
    pub storage: Vec<StorageSummary>,
    pub media_services: Vec<MediaService>,
    pub projects: Vec<ProjectRepo>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub collection_errors: Vec<CollectionError>,
    pub cortex_overlay: CortexOverlaySummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_answer: Option<HomelabMapGraphAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapGraphAnswer {
    pub mode: String,
    pub answer_status: String,
    pub target: HomelabMapGraphTarget,
    pub rows: Vec<HomelabMapAnswerRow>,
    pub candidates: Vec<GraphEntityCandidate>,
    pub evidence: Vec<GraphEvidence>,
    pub metadata: GraphResponseMetadata,
    pub truncation: HomelabMapAnswerTruncation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub next_queries: Vec<HomelabMapNextQuery>,
    pub proof_queries: Vec<HomelabMapProofQuery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<TopologyFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapGraphTarget {
    pub entity_type: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapAnswerRow {
    pub entity_type: String,
    pub key: String,
    pub label: String,
    pub relationship_type: String,
    pub direction: String,
    pub trust_level: String,
    pub confidence: f64,
    pub evidence_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapAnswerTruncation {
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub limit: u32,
    pub evidence_sample_limit: u32,
    pub payload_budget: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapNextQuery {
    pub action: String,
    pub mode: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapProofQuery {
    pub action: String,
    pub mode: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyFinding {
    pub finding_type: String,
    pub severity: String,
    pub confidence: f64,
    pub reason_code: String,
    pub affected_entities: Vec<TopologyFindingEntity>,
    pub evidence: Vec<TopologyFindingEvidence>,
    /// Total safe evidence items available before per-finding and payload
    /// budget limits were applied.
    pub evidence_total: usize,
    /// True when safe evidence was omitted from this finding.
    pub evidence_truncated: bool,
    /// Number of safe evidence items omitted from this finding.
    pub evidence_omitted: usize,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyFindingEntity {
    pub entity_type: String,
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyFindingEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<i64>,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapSummary {
    pub hosts: usize,
    pub returned_hosts: usize,
    pub services: usize,
    pub compose_projects: usize,
    pub reverse_proxies: usize,
    pub projects: usize,
    pub artifacts: usize,
    pub collection_errors: usize,
    pub heartbeat_hosts: usize,
    pub truncated_hosts: bool,
    pub truncated_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapNode {
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub log_count: i64,
    pub source_ips: Vec<HomelabMapSourceIp>,
    pub apps: Vec<HomelabMapApp>,
    pub inventory_roles: Vec<String>,
    pub inventory_ips: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<FleetStateHostRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapSourceIp {
    pub source_ip: String,
    pub log_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomelabMapApp {
    pub app_name: String,
    pub log_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexOverlaySummary {
    pub log_hosts: usize,
    pub heartbeat_hosts: usize,
    pub overlay_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrelateEventsRequest {
    /// RFC3339 anchor time. Optional when `query` is given — the anchor is
    /// then derived from the top AI-transcript session match for `query`.
    pub reference_time: Option<String>,
    pub window_minutes: Option<u32>,
    pub severity_min: Option<String>,
    pub host: Option<String>,
    pub source: Option<String>,
    /// FTS5 filter over the correlated logs. When `reference_time` is absent,
    /// this is also used to locate the anchor time via an AI-session search.
    pub query: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelatedHost {
    pub hostname: String,
    pub event_count: usize,
    pub events: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelateEventsResponse {
    pub reference_time: String,
    pub window_minutes: u32,
    pub window_from: String,
    pub window_to: String,
    pub severity_min: String,
    pub total_events: usize,
    pub truncated: bool,
    pub hosts_count: usize,
    pub hosts: Vec<CorrelatedHost>,
    /// Present when `reference_time` was derived from `query` via an
    /// AI-transcript session search: the session that anchored the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_session: Option<SearchedSessionEntry>,
}
