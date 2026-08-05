use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HomelabInventory {
    pub schema: String,
    pub generated_at: String,
    pub run_id: String,
    pub freshness: InventoryFreshness,
    pub summary: InventorySummary,
    pub nodes: Vec<InventoryNode>,
    pub services: Vec<InventoryService>,
    pub compose_projects: Vec<ComposeProject>,
    pub reverse_proxies: Vec<ReverseProxyRoute>,
    pub networks: Vec<NetworkSegment>,
    pub storage: Vec<StorageSummary>,
    pub media_services: Vec<MediaService>,
    pub projects: Vec<ProjectRepo>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub collection_errors: Vec<CollectionError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_projection: Option<GraphProjectionSummary>,
}

impl HomelabInventory {
    pub fn empty(run_id: String, generated_at: String) -> Self {
        Self {
            schema: crate::inventory::limits::INVENTORY_SCHEMA.to_string(),
            generated_at: generated_at.clone(),
            run_id,
            freshness: InventoryFreshness {
                generated_at,
                stale_after_secs: 86_400,
                is_stale: false,
                cache_status: "generated".to_string(),
            },
            summary: InventorySummary::default(),
            nodes: Vec::new(),
            services: Vec::new(),
            compose_projects: Vec::new(),
            reverse_proxies: Vec::new(),
            networks: Vec::new(),
            storage: Vec::new(),
            media_services: Vec::new(),
            projects: Vec::new(),
            artifact_refs: Vec::new(),
            collection_errors: Vec::new(),
            graph_projection: None,
        }
    }

    pub fn recompute_summary(&mut self) {
        self.summary = InventorySummary {
            nodes: self.nodes.len(),
            services: self.services.len(),
            compose_projects: self.compose_projects.len(),
            reverse_proxies: self.reverse_proxies.len(),
            networks: self.networks.len(),
            storage: self.storage.len(),
            media_services: self.media_services.len(),
            projects: self.projects.len(),
            artifacts: self.artifact_refs.len(),
            errors: self.collection_errors.len(),
            truncated: self.artifact_refs.iter().any(|artifact| artifact.truncated)
                || self.collection_errors.iter().any(|error| error.truncated),
        };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct InventorySummary {
    pub nodes: usize,
    pub services: usize,
    pub compose_projects: usize,
    pub reverse_proxies: usize,
    pub networks: usize,
    pub storage: usize,
    pub media_services: usize,
    pub projects: usize,
    pub artifacts: usize,
    pub errors: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryFreshness {
    pub generated_at: String,
    pub stale_after_secs: u64,
    pub is_stale: bool,
    pub cache_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventoryNode {
    pub id: String,
    pub hostname: String,
    pub trust_level: TrustLevel,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listeners: Vec<ListenerFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<StorageSummary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventoryService {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub trust_level: TrustLevel,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeProject {
    pub name: String,
    pub provenance: Provenance,
    pub services: Vec<String>,
    pub compose_files: Vec<String>,
    pub domains: Vec<String>,
    pub ports: Vec<PortMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReverseProxyRoute {
    pub id: String,
    pub server_names: Vec<String>,
    pub upstreams: Vec<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSegment {
    pub name: String,
    pub kind: String,
    pub members: Vec<String>,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageSummary {
    pub id: String,
    pub mount: String,
    pub fs_type: Option<String>,
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaService {
    pub service: String,
    pub base_url: String,
    pub status: String,
    pub version: Option<String>,
    pub topology: BTreeMap<String, Value>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRepo {
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub worktrees: Vec<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenerFact {
    pub protocol: String,
    pub bind: String,
    pub port: Option<u16>,
    pub process: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortMapping {
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: Option<u16>,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountRef {
    pub source: Option<String>,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub id: String,
    pub kind: String,
    pub collector: String,
    pub source_host: Option<String>,
    pub source_path: Option<String>,
    pub cache_path: String,
    pub redaction: RedactionStatus,
    pub byte_len: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionError {
    pub collector: String,
    pub phase: String,
    pub severity: String,
    pub message: String,
    pub elapsed_ms: u128,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionState {
    pub schema: String,
    pub run_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub collectors: Vec<CollectorState>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub errors: Vec<CollectionError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectorState {
    pub name: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: String,
    pub elapsed_ms: u128,
    pub warnings: Vec<String>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GraphProjectionSummary {
    pub status: String,
    pub source_kinds_reserved: Vec<String>,
    pub next_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provenance {
    pub source: String,
    pub source_kind: String,
    pub collected_at: String,
    pub evidence: Vec<EvidenceRef>,
}

impl Provenance {
    pub fn new(
        source: impl Into<String>,
        source_kind: impl Into<String>,
        collected_at: String,
    ) -> Self {
        Self {
            source: source.into(),
            source_kind: source_kind.into(),
            collected_at,
            evidence: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRef {
    pub artifact_id: Option<String>,
    pub safe_excerpt: Option<String>,
    pub trust_level: TrustLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Verified,
    Observed,
    Claimed,
    Inferred,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStatus {
    Redacted,
    NoSecretsDetected,
}
