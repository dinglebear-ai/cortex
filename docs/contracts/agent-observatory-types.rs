//! Contract-only Rust declarations for the proposed Agent Observatory.
//! These types intentionally use only the standard library so this fixture can
//! be compiled directly with rustc. Production copies add serde derives and
//! deny_unknown_fields where requests cross a trust boundary.

use std::collections::BTreeMap;

pub type Id = String;
pub type Timestamp = String;
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(JsonObject),
}

pub type JsonObject = BTreeMap<String, JsonValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Starting,
    Active,
    Waiting,
    Idle,
    Stale,
    Completed,
    Failed,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Verified,
    Claimed,
    Correlated,
    Inferred,
    Refuted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessState {
    Fresh,
    Delayed,
    Stale,
    NotObserved,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEventKind {
    Lifecycle,
    Transcript,
    Command,
    ShellHistory,
    GitStatus,
    GitHead,
    GitCommit,
    FileOperation,
    Mcp,
    Hook,
    Skill,
    Llm,
    OtlpLog,
    OtlpSpan,
    OtlpMetric,
    Heartbeat,
    Error,
    ProviderEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEventName {
    RunCreated,
    RunUpdated,
    RunStatus,
    RunEvent,
    WorktreeUpdated,
    RepositoryUpdated,
    TelemetryUpdated,
    ObservatoryReset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessLane {
    pub state: FreshnessState,
    pub last_observed_at: Option<Timestamp>,
    pub lag_seconds: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFreshness {
    pub transcript: FreshnessLane,
    pub command: FreshnessLane,
    pub git: FreshnessLane,
    pub otlp_log: FreshnessLane,
    pub trace: FreshnessLane,
    pub metric: FreshnessLane,
    pub lifecycle: FreshnessLane,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub kind: String,
    pub source: String,
    pub trust: TrustLevel,
    pub confidence: f64,
    pub first_seen_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySummary {
    pub id: Id,
    pub key: String,
    pub hostname: String,
    pub primary_path: String,
    pub name: String,
    pub first_seen_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub removed_at: Option<Timestamp>,
    pub worktree_count: u64,
    pub active_run_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSummary {
    pub id: Id,
    pub key: String,
    pub repository_id: Id,
    pub hostname: String,
    pub path: String,
    pub branch_ref: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub upstream_ref: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
    pub dirty: bool,
    pub staged: u64,
    pub unstaged: u64,
    pub untracked: u64,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub first_seen_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub removed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActor {
    pub key: String,
    pub native_id: String,
    pub actor_type: Option<String>,
    pub display_name: Option<String>,
    pub started_at: Option<Timestamp>,
    pub last_activity_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunSummary {
    pub id: Id,
    pub run_key: String,
    pub native_session_id: String,
    pub tool: String,
    pub provider_tool: Option<String>,
    pub hostname: String,
    pub parent_run_key: Option<String>,
    pub previous_run_key: Option<String>,
    pub status: RunStatus,
    pub status_reason: String,
    pub status_observed_at: Timestamp,
    pub started_at: Timestamp,
    pub last_activity_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub transcript_path: Option<String>,
    pub primary_worktree: Option<WorktreeSummary>,
    pub primary_branch: Option<String>,
    pub start_head_sha: Option<String>,
    pub current_head_sha: Option<String>,
    pub event_count: u64,
    pub error_count: u64,
    pub freshness: RunFreshness,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunDetail {
    pub run: AgentRunSummary,
    pub actors: Vec<AgentActor>,
    pub worktree_evidence: Vec<Evidence>,
    pub available_event_kinds: Vec<AgentEventKind>,
    pub commit_summary: Option<GitCommitSummary>,
    pub latest_stream_cursor: Id,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunEvent {
    pub id: Id,
    pub event_key: String,
    pub run_key: String,
    pub actor_key: Option<String>,
    pub worktree_id: Option<Id>,
    pub commit_sha: Option<String>,
    pub observed_at: Timestamp,
    pub ingested_at: Timestamp,
    pub kind: AgentEventKind,
    pub source_kind: String,
    pub source_id: String,
    pub source_log_id: Option<Id>,
    pub provider_sequence: Option<u64>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub payload: Option<JsonObject>,
    pub content_scrubbed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GitCommitSummary {
    pub sha: String,
    pub parent_shas: Vec<String>,
    pub author_name: Option<String>,
    pub authored_at: Option<Timestamp>,
    pub committed_at: Option<Timestamp>,
    pub subject: String,
    pub changed_files: Option<u64>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_paths: Vec<String>,
    pub first_observed_at: Timestamp,
    pub last_observed_at: Timestamp,
    pub reachable: bool,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanSummary {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: i32,
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub duration_nano: u64,
    pub status_code: i32,
    pub status_message: Option<String>,
    pub service_name: Option<String>,
    pub attributes_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricPoint {
    pub point_key: String,
    pub metric_name: String,
    pub description: String,
    pub unit: String,
    pub instrument_kind: String,
    pub start_time: Option<Timestamp>,
    pub time: Timestamp,
    pub value_json: String,
    pub attributes_json: String,
    pub exemplars_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunTelemetry {
    pub run_key: String,
    pub spans: Vec<SpanSummary>,
    pub metrics: Vec<MetricPoint>,
    pub summary: JsonObject,
    pub freshness: RunFreshness,
    pub span_pagination: Pagination,
    pub metric_pagination: Pagination,
    pub as_of: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pagination {
    pub limit: u32,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunPage {
    pub runs: Vec<AgentRunSummary>,
    pub pagination: Pagination,
    pub as_of: Timestamp,
    pub stream_cursor: Id,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventPage {
    pub run_key: String,
    pub events: Vec<AgentRunEvent>,
    pub pagination: Pagination,
    pub as_of: Timestamp,
    pub stream_cursor: Id,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamEnvelope {
    pub id: Id,
    pub event: StreamEventName,
    pub entity_type: String,
    pub entity_key: String,
    pub run_key: Option<String>,
    pub occurred_at: Timestamp,
    pub data: JsonObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillStatus {
    Accepted,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillJob {
    pub job_id: Id,
    pub source: String,
    pub from_id: Option<Id>,
    pub until_id: Option<Id>,
    pub status: BackfillStatus,
    pub processed: u64,
    pub total: Option<u64>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub error: Option<String>,
    pub restart_of_job_id: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionCursorStatus {
    pub source_name: String,
    pub last_source_id: Id,
    pub source_max_id: Id,
    pub lag_rows: u64,
    pub last_success_at: Option<Timestamp>,
    pub last_error_at: Option<Timestamp>,
    pub last_error: Option<String>,
    pub retry_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservatoryStatus {
    pub enabled: bool,
    pub schema_version: i64,
    pub projection_version: u32,
    pub projector_running: bool,
    pub cursors: Vec<ProjectionCursorStatus>,
    pub git_observer_json: String,
    pub stream_json: String,
    pub otlp_json: String,
    pub web_json: String,
    pub warnings: Vec<String>,
}

pub fn length_prefixed(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("|")
}

pub fn run_key(host: &str, tool: &str, session: &str) -> Result<String, &'static str> {
    let host = host.trim();
    let tool = tool.trim().to_ascii_lowercase();
    let session = session.trim();
    if host.is_empty() || tool.is_empty() || session.is_empty() {
        return Err("host, tool, and session must be non-empty");
    }
    Ok(format!("v1|{}", length_prefixed(&[host, &tool, session])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_key_is_length_prefixed_and_stable() {
        assert_eq!(
            run_key("devhost", "Claude", "session-1").unwrap(),
            "v1|7:devhost|6:claude|9:session-1"
        );
    }

    #[test]
    fn empty_run_key_part_is_rejected() {
        assert!(run_key("devhost", "", "session-1").is_err());
    }
}
