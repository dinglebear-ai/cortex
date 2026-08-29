use serde::{Deserialize, Serialize};

/// Named struct for a log entry used in batch insertion and the syslog parse pipeline.
///
/// Replaces the former 8-tuple type alias; named fields prevent silent data corruption
/// from positional swaps between structurally identical `String`/`Option<String>` fields.
///
/// For syslog input, `source_ip` records the actual network sender address (IP:port)
/// independent of the hostname claimed in the syslog message body. OTLP stores the
/// peer IP without the ephemeral port. Docker ingest uses configured
/// `docker://host/container/stream` and `docker-event://host/container/action`
/// source identifiers instead.
#[derive(Debug, Clone)]
pub struct LogBatchEntry {
    pub timestamp: String,
    pub hostname: String,
    pub facility: Option<String>,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub message: String,
    pub raw: String,
    /// Source identifier. Syslog input uses the actual network sender address
    /// (IP:port); OTLP uses peer IP; Docker ingest uses
    /// docker://host/container/stream and docker-event://host/container/action.
    pub source_ip: String,
    pub docker_checkpoint: Option<DockerCheckpoint>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_transcript_path: Option<String>,
    pub metadata_json: Option<String>,
    /// HTTP status code (3 digits). Indexed column. Set by `swag` parser.
    pub http_status: Option<i32>,

    /// Authentication outcome ("success" | "failure" | "denied" | "challenge").
    /// Indexed column. Set by `authelia` parser.
    pub auth_outcome: Option<&'static str>,

    /// DNS block decision. `Some(true)` = filtered/blocked, `Some(false)` = explicit
    /// allow, `None` = N/A (rewrites and non-DNS rows). Indexed column.
    pub dns_blocked: Option<bool>,

    /// Normalised event verb (closed enum per parser). Indexed column.
    pub event_action: Option<String>,

    /// Per-row parser diagnostic: "{parser_name}: {ParserError::Display}",
    /// truncated to 512 bytes. No index — diagnostic only.
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DockerCheckpoint {
    pub host_name: String,
    pub container_id: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Default)]
pub struct ListAiSessionsParams {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub host: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiSessionEntry {
    pub ai_project: String,
    pub ai_tool: String,
    pub ai_session_id: String,
    pub ai_transcript_path: Option<String>,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub event_count: i64,
}

#[derive(Debug, Clone)]
pub struct RenderedSessionEventRow {
    pub id: i64,
    pub timestamp: String,
    pub message: String,
    pub metadata_json: Option<String>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RenderedSessionPageParams {
    pub ai_project: String,
    pub ai_tool: String,
    pub ai_session_id: String,
    pub host: String,
    pub after_id: i64,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct DurableStreamRow {
    pub id: i64,
    pub timestamp: String,
    pub hostname: String,
    pub severity: String,
    pub app_name: Option<String>,
    pub message: String,
    pub metadata_json: Option<String>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DurableStreamParams {
    pub after_id: i64,
    pub high_watermark: Option<i64>,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub severity: Option<String>,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_session_id: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct DurableStreamPage {
    pub rows: Vec<DurableStreamRow>,
    pub minimum_watermark: Option<i64>,
    pub high_watermark: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SearchAiSessionsParams {
    pub query: String,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    /// Filter AI transcript sessions to those where the session's host matches.
    pub host: Option<String>,
    /// Filter AI transcript sessions to those where the session's app matches.
    pub app: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
}

/// Error/warning summary entry (one row per hostname+severity, plus optional app_name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSummaryEntry {
    pub hostname: String,
    /// Populated when the summary was requested with `group_by=app_name`.
    pub app_name: Option<String>,
    pub severity: String,
    pub count: i64,
}

/// Host registry entry with first/last seen and log count
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub log_count: i64,
}

/// Database statistics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStats {
    pub total_logs: i64,
    pub total_hosts: i64,
    pub oldest_log: Option<String>,
    pub newest_log: Option<String>,
    /// Formatted as "X.XX" MB
    pub logical_db_size_mb: String,
    /// Formatted as "X.XX" MB
    pub physical_db_size_mb: String,
    /// Formatted as "X.XX" MB when available
    pub free_disk_mb: Option<String>,
    pub max_db_size_mb: u64,
    pub min_free_disk_mb: u64,
    pub write_blocked: bool,
    /// Phantom FTS rows: entries in logs_fts that no longer have a matching log row.
    /// Accumulate between merge cycles; non-zero value is normal and cleaned up by
    /// periodic fts_incremental_merge. High values indicate merge is falling behind.
    ///
    /// `None` when the FTS diagnostic was skipped: computing it requires
    /// `COUNT(*) FROM logs_fts`, an external-content FTS5 index scan that is
    /// expensive on very large databases. The default `stats` path skips it;
    /// pass `include_fts_diagnostics` to compute it explicitly.
    pub phantom_fts_rows: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchedAiSessionEntry {
    pub ai_project: String,
    pub ai_tool: String,
    pub ai_session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub event_count: i64,
    pub match_count: i64,
    pub best_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchAiSessionsResult {
    pub total_candidates: usize,
    pub candidate_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
    pub sessions: Vec<SearchedAiSessionEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct AiAbuseParams {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub before: Option<u32>,
    pub after: Option<u32>,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAbuseMatch {
    pub term: String,
    pub entry: LogEntry,
    pub before: Vec<LogEntry>,
    pub after: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAbuseResult {
    pub terms: Vec<String>,
    pub candidate_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
    pub matches: Vec<AiAbuseMatch>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiCorrelateParams {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_query: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiRelatedWindow {
    pub anchor_index: usize,
    pub anchor_time: String,
    pub window_from: String,
    pub window_to: String,
}

/// DB-layer carrier for graph-anchored session correlation: the session time
/// bounds, the entities/hosts discovered by traversing the graph from the
/// session entity, and the fanned-out logs. `used_graph` is false when no
/// `ai_session` graph entity exists for the session (time-windowed fallback).
#[derive(Debug, Clone, Default)]
pub struct SessionGraphInputs {
    pub bounds: Option<(String, String)>,
    pub discovered_hosts: Vec<String>,
    pub discovered_entities: Vec<String>,
    pub used_graph: bool,
    pub logs: Vec<LogEntry>,
}

/// A graph entity matched while resolving a topic string, with how it matched
/// (`exact` canonical key, `prefix` of a key, or `alias`).
#[derive(Debug, Clone)]
pub struct ResolvedTopicEntity {
    pub entity_type: String,
    pub canonical_key: String,
    pub match_kind: &'static str,
    /// Resolver outcome: `Resolved` for exact canonical-key and alias
    /// identity matches, `Ambiguous` for weak prefix/label candidates that
    /// never drive log fan-out. Stringified via
    /// [`super::entity_resolution::ResolverStatus::as_str`] only at the serde
    /// boundary.
    pub resolver_status: super::entity_resolution::ResolverStatus,
}

/// One correlated log row annotated with why it was included and the
/// resolver outcome for its inclusion path.
#[derive(Debug, Clone)]
pub struct GraphRelatedLogEntry {
    pub entry: LogEntry,
    pub inclusion_reason: String,
    pub resolver_status: super::entity_resolution::ResolverStatus,
    pub fallback_kind: Option<String>,
}

/// DB-layer carrier for topic correlation: the entities the topic resolved to,
/// the entities/hosts reached by graph expansion, and the fanned-out logs.
#[derive(Debug, Clone, Default)]
pub struct TopicGraphInputs {
    pub resolved: Vec<ResolvedTopicEntity>,
    /// Entities reached by traversal that were not themselves resolved seeds.
    pub expansion: Vec<(String, String)>,
    pub discovered_hosts: Vec<String>,
    pub logs: Vec<GraphRelatedLogEntry>,
    /// `true` when the service-topic graph walk
    /// ([`super::graph_resolver_projection::graph_walk_service_topic`]) hit
    /// `GRAPH_SERVICE_TOPIC_ENTITY_CAP` and the reached neighborhood was cut
    /// off rather than exhaustive.
    pub graph_walk_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiRelatedLogsParams {
    pub windows: Vec<AiRelatedWindow>,
    pub query: Option<String>,
    pub host: Option<String>,
    pub source: Option<String>,
    pub severity_in: Vec<String>,
    pub app: Option<String>,
    pub limit_per_anchor: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiRelatedLogsForAnchor {
    pub anchor_index: usize,
    pub logs: Vec<LogEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiUsageBlocksParams {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiUsageBlock {
    pub bucket_start: String,
    pub bucket_end: String,
    pub project: String,
    pub tool: String,
    pub session_count: i64,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiUsageBlocksResult {
    pub total_blocks: usize,
    pub truncated: bool,
    pub blocks: Vec<AiUsageBlock>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiProjectContextParams {
    pub project: String,
    pub ai_tool: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProjectContext {
    pub project: String,
    pub tools: Vec<String>,
    pub sessions: Vec<String>,
    pub hostnames: Vec<String>,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub event_count: i64,
    pub recent_entries_truncated: bool,
    pub recent_entries: Vec<LogEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAiToolsParams {
    pub ai_project: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolInventoryEntry {
    pub tool: String,
    pub event_count: i64,
    pub session_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiToolsResult {
    pub total_tools: usize,
    pub truncated: bool,
    pub tools: Vec<AiToolInventoryEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAiProjectsParams {
    pub ai_tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProjectInventoryEntry {
    pub project: String,
    pub tools: Vec<String>,
    pub event_count: i64,
    pub session_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiProjectsResult {
    pub total_projects: usize,
    pub truncated: bool,
    pub projects: Vec<AiProjectInventoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub logical_db_size_bytes: u64,
    pub physical_db_size_bytes: u64,
    pub free_disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRecovery {
    pub logical_db_size_bytes: u64,
    pub free_disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEnforcementOutcome {
    pub metrics: StorageMetrics,
    pub recovery: StorageRecovery,
    pub deleted_rows: usize,
    pub write_blocked: bool,
}

#[derive(Debug, Clone)]
pub struct StorageBudgetState {
    pub metrics: StorageMetrics,
    pub write_blocked: bool,
}

/// A parsed and stored log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: String,
    pub hostname: String,
    pub facility: Option<String>,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub message: String,
    pub received_at: String,
    /// Source identifier. Syslog entries use verified network sender address
    /// (IP:port); OTLP entries use peer IP; Docker ingest entries use
    /// docker://host/container/stream or docker-event://host/container/action.
    /// Empty string for legacy rows inserted before this column was added.
    pub source_ip: String,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_transcript_path: Option<String>,
    pub metadata_json: Option<String>,
}

/// Parameters for searching logs
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchParams {
    /// Full-text search query (FTS5 syntax)
    pub query: Option<String>,
    /// Filter by hostname
    pub host: Option<String>,
    /// Filter by source identifier. Syslog uses verified network sender address
    /// (IP:port); OTLP uses peer IP; Docker ingest uses
    /// docker://host/container/stream or docker-event://host/container/action.
    pub source: Option<String>,
    /// Filter by source identifier prefix using an indexed range predicate.
    pub source_ip_prefix: Option<String>,
    /// Filter by severity (exact match: emerg, alert, crit, err, warning, notice, info, debug)
    pub severity: Option<String>,
    /// Filter by one of a set of severity levels (for threshold queries)
    pub severity_in: Option<Vec<String>>,
    /// Filter by app name
    pub app: Option<String>,
    /// Filter by syslog facility name (e.g. `kern`, `auth`, `daemon`)
    pub facility: Option<String>,
    /// Exclude a syslog facility while keeping rows with unknown facility.
    pub exclude_facility: Option<String>,
    /// Filter by process_id (exact match)
    pub process_id: Option<String>,
    /// Start of time range (ISO 8601)
    pub since: Option<String>,
    /// End of time range (ISO 8601)
    pub until: Option<String>,
    /// Start of receive-time range (ISO 8601)
    pub received_since: Option<String>,
    /// End of receive-time range (ISO 8601)
    pub received_until: Option<String>,
    /// Max results to return
    pub limit: Option<u32>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub event_action: Option<String>,
    pub exclude_ai: bool,
}

impl SearchParams {
    /// True when a filter is set on a column backed by a `(col, timestamp)`
    /// index AND whose partitions are small enough for the index-led plan
    /// (hostname, source_ip, app_name, event_action, ai_project). The FTS
    /// search uses this to choose the index-led intersect plan — which leads
    /// with the filter's composite index and intersects the FTS match set —
    /// instead of scanning the entire match set and filtering post-hoc (the
    /// pathology that made `search <q> --host <h>` ~200s).
    ///
    /// `severity`/`severity_in` are deliberately EXCLUDED: a single severity
    /// can be >90% of the table, so leading with `idx_logs_sev_time` for a
    /// rare term walks nearly the entire partition before LIMIT fills
    /// (full-review PH1). Severity-only searches take the capped-candidate
    /// path instead; severity combined with a selective filter still uses the
    /// fast path via the selective column's index.
    pub(crate) fn has_indexed_equality_filter(&self) -> bool {
        self.host.is_some()
            || self.source.is_some()
            || self.source_ip_prefix.is_some()
            || self.app.is_some()
            || self.event_action.is_some()
            || self.ai_project.is_some()
    }
}

// ---------------------------------------------------------------------------
// Abuse incident grouping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiIncidentParams {
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Max incidents to return. Default 20, clamp 1..=100.
    pub limit: Option<u32>,
    /// Grouping window in minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbuseIncident {
    /// Stable synthetic ID: sha256 of "project|tool|session|host|first_anchor_id".
    pub incident_id: String,
    pub project: String,
    pub tool: String,
    pub session_id: String,
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub duration_secs: i64,
    pub abuse_count: usize,
    /// Distinct normalized abuse terms found, sorted.
    pub terms: Vec<String>,
    /// Sorted anchor log IDs.
    pub anchor_ids: Vec<i64>,
    pub priority_score: f64,
    /// "low" | "medium" | "high" | "critical"
    pub priority_label: String,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiIncidentResult {
    pub incidents: Vec<AbuseIncident>,
    pub total_incidents: usize,
    pub candidate_rows: usize,
    pub candidate_cap: usize,
    pub candidate_window_truncated: bool,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// AI investigate — evidence bundle layer (kmib.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiInvestigateParams {
    /// Optional exact incident ID. When present, locate one matching incident
    /// within the incident-list cap instead of only the top investigation page.
    pub incident_id: Option<String>,
    pub ai_project: Option<String>,
    pub ai_tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Max incidents to investigate. Default 3, clamp 1..=10.
    pub limit: Option<u32>,
    /// Incident grouping window minutes. Default 10, clamp 1..=120.
    pub window_minutes: Option<u32>,
    /// Correlation window minutes around incident. Default 5, clamp 1..=120.
    pub correlation_window_minutes: Option<u32>,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentEvidence {
    pub incident: AbuseIncident,
    /// Transcript entries before first anchor (same session), capped at 20.
    pub transcript_before: Vec<LogEntry>,
    pub transcript_before_truncated: bool,
    /// Transcript entries after last anchor (same session), capped at 20.
    pub transcript_after: Vec<LogEntry>,
    pub transcript_after_truncated: bool,
    /// The abuse anchor log entries.
    pub anchors: Vec<LogEntry>,
    /// Non-AI syslog/Docker logs in the correlation window, capped at 50.
    pub nearby_logs: Vec<LogEntry>,
    pub nearby_logs_truncated: bool,
    /// Subset of nearby_logs with severity warning or above.
    pub nearby_errors: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiInvestigateResult {
    pub evidence: Vec<IncidentEvidence>,
    pub total_incidents: usize,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// RAG v1: similar_incidents, incident_context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SimilarIncidentsParams {
    pub query: String,
    pub host: Option<String>,
    pub app: Option<String>,
    /// Minimum severity (e.g. "warning"). None = all severities.
    pub severity_min: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Cluster grouping window in minutes. Default 30, clamp 5..=120.
    pub window_minutes: Option<u32>,
    /// Max clusters to return. Default 10, clamp 1..=50.
    pub limit: Option<u32>,
}

/// A time-windowed cluster of log hits (one "incident").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentCluster {
    pub hostname: String,
    pub app_name: Option<String>,
    /// RFC 3339 timestamp of the first matching log in this cluster.
    pub window_start: String,
    /// RFC 3339 timestamp of the last matching log in this cluster.
    pub window_end: String,
    pub log_count: i64,
    /// Highest severity in this cluster (emerg > alert > ... > debug).
    pub severity_peak: String,
    /// Up to 3 representative message snippets (first 256 chars each).
    pub representative_messages: Vec<String>,
    /// AI sessions whose transcript entries overlap this cluster's time window.
    pub correlated_sessions: Vec<CorrelatedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelatedSession {
    pub session_id: String,
    pub project: String,
    pub tool: String,
    pub match_count: i64,
    pub best_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarIncidentsResult {
    pub query: String,
    pub total_clusters: usize,
    pub truncated: bool,
    pub clusters: Vec<IncidentCluster>,
}

#[derive(Debug, Clone, Default)]
pub struct IncidentContextParams {
    pub since: String,
    pub until: String,
    pub host: Option<String>,
    pub app: Option<String>,
    /// Optional FTS5 query applied to returned error logs.
    pub query: Option<String>,
    pub severity_min: Option<String>,
    /// Max error log rows to return. Default 50, clamp 1..=200.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeverityCount {
    pub severity: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogCount {
    pub app_name: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentContextResult {
    pub window_from: String,
    pub window_to: String,
    pub total_logs: i64,
    pub by_severity: Vec<SeverityCount>,
    pub by_app: Vec<AppLogCount>,
    /// Logs at or above severity_min (default: warning) within the window.
    pub error_logs: Vec<LogEntry>,
    pub error_logs_truncated: bool,
    /// AI sessions active in this window (have transcript entries between from..to).
    pub ai_sessions: Vec<AiSessionEntry>,
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
