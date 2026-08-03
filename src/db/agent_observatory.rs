#![allow(dead_code)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[path = "agent_observatory_observations.rs"]
mod observations;
pub use observations::{
    RepositoryObservationInput, list_repository_observations,
    record_repository_observations_if_changed,
};

#[path = "agent_observatory_queries.rs"]
mod queries;
pub use queries::{
    RepositoryReconcileResult, RepositoryUpsert, RepositoryWorktreeUpsert, get_repository_by_key,
    get_worktree_by_key, list_repository_worktrees, mark_repository_removed, mark_worktree_removed,
    reconcile_repository,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumParseError {
    type_name: &'static str,
    value: String,
}

impl EnumParseError {
    pub(crate) fn new(type_name: &'static str, value: &str) -> Self {
        Self {
            type_name,
            value: value.to_string(),
        }
    }
}

impl fmt::Display for EnumParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} value: {}",
            self.type_name, self.value
        )
    }
}

impl std::error::Error for EnumParseError {}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = EnumParseError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(EnumParseError::new(stringify!($name), value)),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where S: Serializer {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where D: Deserializer<'de> {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

string_enum!(RunStatus {
    Starting => "starting", Active => "active", Waiting => "waiting", Idle => "idle",
    Stale => "stale", Completed => "completed", Failed => "failed", Abandoned => "abandoned",
});

string_enum!(AgentEventKind {
    Lifecycle => "lifecycle", Transcript => "transcript", Command => "command",
    ShellHistory => "shell_history", GitStatus => "git_status", GitHead => "git_head",
    GitCommit => "git_commit", FileOperation => "file_operation", Mcp => "mcp", Hook => "hook",
    Skill => "skill", Llm => "llm", OtlpLog => "otlp_log", OtlpSpan => "otlp_span",
    OtlpMetric => "otlp_metric", Heartbeat => "heartbeat", Error => "error",
    ProviderEvent => "provider_event",
});

string_enum!(EvidenceTrustLevel {
    Verified => "verified", Claimed => "claimed", Correlated => "correlated",
    Inferred => "inferred", Refuted => "refuted",
});

string_enum!(RepositoryObservationKind {
    Discovered => "discovered", Status => "status", Head => "head", Branch => "branch",
    WorktreeAdded => "worktree_added", WorktreeRemoved => "worktree_removed",
    OverflowReconcile => "overflow_reconcile", PeriodicReconcile => "periodic_reconcile",
    Error => "error",
});

string_enum!(StreamEventName {
    RunCreated => "run.created", RunUpdated => "run.updated", RunStatus => "run.status",
    RunEvent => "run.event", WorktreeUpdated => "worktree.updated",
    RepositoryUpdated => "repository.updated", TelemetryUpdated => "telemetry.updated",
    ObservatoryReset => "observatory.reset",
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepositoryRow {
    pub id: i64,
    pub repository_key: String,
    pub hostname: String,
    pub common_git_dir: String,
    pub primary_path: String,
    pub display_name: String,
    pub remote_url_hash: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub removed_at: Option<String>,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepositoryWorktreeRow {
    pub id: i64,
    pub worktree_key: String,
    pub repository_id: i64,
    pub hostname: String,
    pub path: String,
    pub git_dir: String,
    pub branch_ref: Option<String>,
    pub branch_name: Option<String>,
    pub head_sha: Option<String>,
    pub upstream_ref: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
    pub dirty: bool,
    pub staged_count: i64,
    pub unstaged_count: i64,
    pub untracked_count: i64,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub status_hash: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub removed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepositoryObservationRow {
    pub id: i64,
    pub observation_key: String,
    pub repository_id: i64,
    pub worktree_id: Option<i64>,
    pub observed_at: String,
    pub observation_kind: RepositoryObservationKind,
    pub old_head_sha: Option<String>,
    pub new_head_sha: Option<String>,
    pub summary: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunRow {
    pub id: i64,
    pub run_key: String,
    pub native_session_id: String,
    pub tool: String,
    pub provider_tool: Option<String>,
    pub hostname: String,
    pub parent_run_id: Option<i64>,
    pub previous_run_id: Option<i64>,
    pub primary_worktree_id: Option<i64>,
    pub transcript_path: Option<String>,
    pub process_id: Option<String>,
    pub status: RunStatus,
    pub status_reason: String,
    pub status_observed_at: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub ended_at: Option<String>,
    pub first_source_log_id: Option<i64>,
    pub last_source_log_id: Option<i64>,
    pub last_event_id: Option<i64>,
    pub event_count: i64,
    pub error_count: i64,
    pub primary_branch: Option<String>,
    pub start_head_sha: Option<String>,
    pub current_head_sha: Option<String>,
    pub projection_version: i64,
    pub freshness_json: String,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunEventRow {
    pub id: i64,
    pub event_key: String,
    pub run_id: i64,
    pub actor_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub commit_id: Option<i64>,
    pub observed_at: String,
    pub ingested_at: String,
    pub event_kind: AgentEventKind,
    pub source_kind: String,
    pub source_id: String,
    pub source_log_id: Option<i64>,
    pub provider_sequence: Option<i64>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub payload_json: String,
    pub content_scrubbed: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunWorktreeEvidenceRow {
    pub id: i64,
    pub relation_key: String,
    pub run_id: i64,
    pub worktree_id: i64,
    pub evidence_kind: String,
    pub evidence_source: String,
    pub trust_level: EvidenceTrustLevel,
    pub confidence: f64,
    pub is_primary: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunCommitEvidenceRow {
    pub id: i64,
    pub relation_key: String,
    pub run_id: i64,
    pub commit_id: i64,
    pub worktree_id: Option<i64>,
    pub evidence_kind: String,
    pub evidence_source: String,
    pub trust_level: EvidenceTrustLevel,
    pub confidence: f64,
    pub observed_at: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCursorRow {
    pub source_name: String,
    pub last_source_id: i64,
    pub source_max_id: i64,
    pub projection_version: i64,
    pub last_success_at: Option<String>,
    pub last_error_at: Option<String>,
    pub last_error: Option<String>,
    pub retry_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOutboxRow {
    pub id: i64,
    pub event_name: StreamEventName,
    pub entity_type: String,
    pub entity_key: String,
    pub run_id: Option<i64>,
    pub payload_json: String,
    pub created_at: String,
    pub expires_at: String,
}
