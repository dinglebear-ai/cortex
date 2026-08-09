use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    pub log_id: Option<i64>,
    pub host: Option<String>,
    pub timestamp: Option<String>,
    pub before: Option<u32>,
    pub after: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResponse {
    pub reference: LogEntry,
    pub before: Vec<LogEntry>,
    pub after: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetLogRequest {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLogResponse {
    pub log: LogEntryWithRaw,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedLogsRequest {
    /// Last Cortex log ID successfully consumed. When omitted, the feed starts
    /// at the current high-water mark and returns only subsequently ingested rows.
    pub after_id: Option<i64>,
    pub host: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedLogsResponse {
    pub logs: Vec<LogEntryWithRaw>,
    pub next_after_id: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntryWithRaw {
    pub id: i64,
    pub timestamp: String,
    pub hostname: String,
    pub facility: Option<String>,
    pub severity: String,
    pub app_name: Option<String>,
    pub process_id: Option<String>,
    pub message: String,
    pub raw: String,
    pub received_at: String,
    pub source_ip: String,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_transcript_path: Option<String>,
    pub metadata_json: Option<String>,
}

impl From<db::LogEntryWithRaw> for LogEntryWithRaw {
    fn from(value: db::LogEntryWithRaw) -> Self {
        Self {
            id: value.id,
            timestamp: value.timestamp,
            hostname: value.hostname,
            facility: value.facility,
            severity: value.severity,
            app_name: value.app_name,
            process_id: value.process_id,
            message: value.message,
            raw: value.raw,
            received_at: value.received_at,
            source_ip: value.source_ip,
            ai_tool: value.ai_tool,
            ai_project: value.ai_project,
            ai_session_id: value.ai_session_id,
            ai_transcript_path: value.ai_transcript_path,
            metadata_json: value.metadata_json,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestRateRequest {
    pub by_host: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRateResponse {
    pub now: String,
    pub buckets: IngestRateBuckets,
    pub write_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_host: Option<Vec<IngestRatePerHost>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRateBuckets {
    pub last_1m: i64,
    pub last_5m: i64,
    pub last_15m: i64,
    pub per_sec_1m: f64,
    pub per_sec_5m: f64,
    pub per_sec_15m: f64,
}

impl From<db::IngestRateBuckets> for IngestRateBuckets {
    fn from(value: db::IngestRateBuckets) -> Self {
        Self {
            last_1m: value.last_1m,
            last_5m: value.last_5m,
            last_15m: value.last_15m,
            per_sec_1m: value.per_sec_1m,
            per_sec_5m: value.per_sec_5m,
            per_sec_15m: value.per_sec_15m,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRatePerHost {
    pub hostname: String,
    pub last_1m: i64,
    pub last_5m: i64,
    pub last_15m: i64,
}

impl From<db::IngestRatePerHost> for IngestRatePerHost {
    fn from(value: db::IngestRatePerHost) -> Self {
        Self {
            hostname: value.hostname,
            last_1m: value.last_1m,
            last_5m: value.last_5m,
            last_15m: value.last_15m,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SilentHostsRequest {
    /// Hosts whose `last_seen` is older than `silent_minutes` ago (default 30).
    pub silent_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilentHostsResponse {
    pub silent_minutes: u32,
    pub cutoff: String,
    pub now: String,
    pub hosts: Vec<SilentHostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilentHostEntry {
    pub hostname: String,
    pub first_seen: String,
    pub last_seen: String,
    pub log_count: i64,
    pub typical_interval_secs: Option<f64>,
    pub silent_for_secs: i64,
}

impl From<db::SilentHostEntry> for SilentHostEntry {
    fn from(value: db::SilentHostEntry) -> Self {
        Self {
            hostname: value.hostname,
            first_seen: value.first_seen,
            last_seen: value.last_seen,
            log_count: value.log_count,
            typical_interval_secs: value.typical_interval_secs,
            silent_for_secs: value.silent_for_secs,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSkewRequest {
    pub since: Option<String>,
    /// Max host rows to return, sorted by absolute average skew.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockSkewResponse {
    pub since: String,
    pub hosts: Vec<ClockSkewEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockSkewEntry {
    pub hostname: String,
    pub samples: i64,
    pub avg_skew_secs: f64,
    pub min_skew_secs: f64,
    pub max_skew_secs: f64,
}

impl From<db::ClockSkewEntry> for ClockSkewEntry {
    fn from(value: db::ClockSkewEntry) -> Self {
        Self {
            hostname: value.hostname,
            samples: value.samples,
            avg_skew_secs: value.avg_skew_secs,
            min_skew_secs: value.min_skew_secs,
            max_skew_secs: value.max_skew_secs,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnomaliesRequest {
    /// Recent window in minutes (default 15, max 1440).
    pub recent_minutes: Option<u32>,
    /// Baseline window in minutes preceding the recent window (default 360, max 10080).
    pub baseline_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomaliesResponse {
    pub recent_from: String,
    pub recent_to: String,
    pub baseline_from: String,
    pub baseline_to: String,
    pub recent_minutes: u32,
    pub baseline_minutes: u32,
    pub hosts: Vec<AnomalyEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyEntry {
    pub hostname: String,
    pub recent_count: i64,
    pub baseline_count: i64,
    pub recent_per_min: f64,
    pub baseline_per_min: f64,
    pub ratio: Option<f64>,
    pub z_score: Option<f64>,
    pub recent_errors: i64,
    pub baseline_errors: i64,
}

impl From<db::AnomalyEntry> for AnomalyEntry {
    fn from(value: db::AnomalyEntry) -> Self {
        Self {
            hostname: value.hostname,
            recent_count: value.recent_count,
            baseline_count: value.baseline_count,
            recent_per_min: value.recent_per_min,
            baseline_per_min: value.baseline_per_min,
            ratio: value.ratio,
            z_score: value.z_score,
            recent_errors: value.recent_errors,
            baseline_errors: value.baseline_errors,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    pub a_from: Option<String>,
    pub a_to: Option<String>,
    pub b_from: Option<String>,
    pub b_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareResponse {
    pub a: RangeSummary,
    pub b: RangeSummary,
    pub delta_total_logs: i64,
    pub delta_total_errors: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeSummary {
    pub from: String,
    pub to: String,
    pub total_logs: i64,
    pub total_errors: i64,
    pub by_severity: Vec<(String, i64)>,
    pub top_hosts: Vec<(String, i64)>,
    pub top_apps: Vec<(String, i64)>,
}

impl From<db::RangeSummary> for RangeSummary {
    fn from(value: db::RangeSummary) -> Self {
        Self {
            from: value.from,
            to: value.to,
            total_logs: value.total_logs,
            total_errors: value.total_errors,
            by_severity: value.by_severity,
            top_hosts: value.top_hosts,
            top_apps: value.top_apps,
        }
    }
}
