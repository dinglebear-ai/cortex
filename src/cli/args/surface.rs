use cortex::artifact_evidence::ArtifactEvidenceKind;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SilentHostsArgs {
    pub silent_minutes: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClockSkewArgs {
    pub since: Option<String>,
    pub limit: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AnomaliesArgs {
    pub recent_minutes: Option<u32>,
    pub baseline_minutes: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompareArgs {
    pub a_from: Option<String>,
    pub a_to: Option<String>,
    pub b_from: Option<String>,
    pub b_to: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ArtifactEvidenceArgs {
    pub event_kind: Option<ArtifactEvidenceKind>,
    pub artifact_id: Option<String>,
    pub revision_id: Option<String>,
    pub content_digest: Option<String>,
    pub correlation_id: Option<String>,
    pub request_id: Option<String>,
    pub target_id: Option<String>,
    pub source_system: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppsArgs {
    pub host: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub json: bool,
}

// ─── Heartbeat fleet state (cxih.4) ─────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HostStateArgs {
    pub host_id: Option<String>,
    pub host: Option<String>,
    pub since: Option<String>,
    pub limit: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FleetStateArgs {
    pub include_ok: Option<bool>,
    pub sort: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CorrelateStateArgs {
    pub reference_time: Option<String>,
    pub window_minutes: Option<u32>,
    pub host: Option<String>,
    pub severity_min: Option<String>,
    pub limit: Option<u32>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TopicCorrelateArgs {
    pub topic: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub depth: Option<u8>,
    pub source_kinds: Option<Vec<String>>,
    pub limit: Option<u32>,
    pub json: bool,
}
