use super::*;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookBackfillRequest {
    pub since: Option<String>,
    pub limit: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookBackfillResult {
    pub scanned: u64,
    pub inserted: u64,
    pub skipped_duplicates: u64,
    pub parse_errors: u64,
    /// Original transcript path/line could not be recovered.
    pub source_unavailable: u64,
    pub truncated: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListHookEventsRequest {
    pub hook_event: Option<String>,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub status: Option<String>,
    pub evidence_kind: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    #[serde(alias = "since")]
    pub from: Option<String>,
    #[serde(alias = "until")]
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventEntry {
    pub id: i64,
    pub log_id: Option<i64>,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub hostname: String,
    pub timestamp: String,
    pub hook_event: String,
    pub hook_name: Option<String>,
    pub hook_source: Option<String>,
    pub hook_command: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub persisted_output_path: Option<String>,
    pub trusted_hash: Option<String>,
    pub evidence_kind: String,
    pub metadata_json: Option<String>,
}

impl From<db::AiHookEventEntry> for HookEventEntry {
    fn from(value: db::AiHookEventEntry) -> Self {
        Self {
            id: value.id,
            log_id: value.log_id,
            ai_tool: value.ai_tool,
            ai_project: value.ai_project,
            ai_session_id: value.ai_session_id,
            hostname: value.hostname,
            timestamp: value.timestamp,
            hook_event: value.hook_event,
            hook_name: value.hook_name,
            hook_source: value.hook_source,
            hook_command: value.hook_command,
            status: value.status,
            exit_code: value.exit_code,
            duration_ms: value.duration_ms,
            stdout_preview: value.stdout_preview,
            stderr_preview: value.stderr_preview,
            persisted_output_path: value.persisted_output_path,
            trusted_hash: value.trusted_hash,
            evidence_kind: value.evidence_kind,
            metadata_json: value.metadata_json,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHookEventsResponse {
    pub total: usize,
    pub truncated: bool,
    pub events: Vec<HookEventEntry>,
}

impl From<db::ListHookEventsResult> for ListHookEventsResponse {
    fn from(value: db::ListHookEventsResult) -> Self {
        Self {
            total: value.total,
            truncated: value.truncated,
            events: value.events.into_iter().map(Into::into).collect(),
        }
    }
}
