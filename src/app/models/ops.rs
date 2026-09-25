use super::*;

pub use crate::filetail::{
    FileTailAddRequest, FileTailOp, FileTailRequest, FileTailResponse, FileTailSource,
    FileTailStatus,
};

// ---------------------------------------------------------------------------
// Error Detection models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnaddressedErrorsRequest {
    /// Maximum number of signatures to return.
    pub limit: Option<u32>,
    /// Include already-acknowledged signatures in the result.
    pub include_acknowledged: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnaddressedErrorsResponse {
    pub signatures: Vec<ErrorSignatureEntry>,
    /// Warning-noise rows hidden by `is_unaddressed_warning_noise` while
    /// assembling this page (recorded but suppressed, not dropped at ingest).
    pub filtered_count: usize,
    /// Signature rows scanned (post-noise-filter candidates examined), bounded
    /// by `candidate_cap`.
    pub candidate_rows: usize,
    /// Hard scan ceiling (`UNADDRESSED_SCAN_CAP`); echoes the server's cap so
    /// consumers can recognize a truncated window.
    pub candidate_cap: usize,
    /// True when the scan hit `candidate_cap` before filling the requested
    /// limit — more candidates may exist beyond the scanned window. Always equal
    /// to `candidate_rows >= candidate_cap`.
    pub candidate_window_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSignatureEntry {
    pub signature_hash: String,
    pub template: String,
    pub sample_message: String,
    pub severity: String,
    pub sample_hostname: String,
    pub sample_app_name: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub total_count: i64,
    pub count_last_1h: i64,
    pub acknowledged_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckErrorRequest {
    pub signature_hash: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckErrorResponse {
    pub signature_hash: String,
    pub acknowledged_at: String,
    pub actor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnackErrorRequest {
    pub signature_hash: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnackErrorResponse {
    pub signature_hash: String,
    pub unacked_at: String,
    pub actor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsRecentRequest {
    pub limit: Option<i64>,
    pub rule_id: Option<String>,
    pub hostname: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

impl NotificationsRecentRequest {
    pub fn effective_limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 500)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmInvocationsRequest {
    pub limit: Option<i64>,
    pub since: Option<String>,
    pub action: Option<String>,
    pub status: Option<String>,
}

impl LlmInvocationsRequest {
    pub fn effective_limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 500)
    }
}

// ── AI checkpoint inventory + prune request structs (bead 0p8r.3) ────────────
//
// These are typed request shapes shared between the REST handlers in
// `src/api.rs` and the future HTTP client in bead 0p8r.5. The corresponding
// service methods (`list_ai_checkpoints`, `list_ai_parse_errors`,
// `prune_ai_checkpoints` in `src/app/service.rs:609,628,638`) keep their
// loose primitive signatures — handlers unpack the request into positional
// args before calling the service.
//
// `deny_unknown_fields` on all three: typo'd POST/JSON fields must surface
// as 400, not be silently dropped (eng-review #A1 echo).

/// Query parameters for `GET /api/sessions/checkpoints`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCheckpointsRequest {
    /// Restrict to checkpoints with persisted parse errors.
    #[serde(default)]
    pub errors_only: bool,
    /// Restrict to checkpoints whose source file is missing on disk.
    #[serde(default)]
    pub missing_only: bool,
    pub limit: Option<u32>,
}

/// Query parameters for `GET /api/sessions/errors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiParseErrorsRequest {
    pub limit: Option<u32>,
}

/// JSON body for `POST /api/sessions/prune-checkpoints`.
///
/// `dry_run` is intentionally `bool` (not `Option<bool>`): the handler
/// pre-validates the JSON body contains the key before deserialization
/// (eng-review C3). Defaulting silently to `false` would let `POST {}`
/// mass-delete checkpoints — instead the handler returns 400.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiPruneCheckpointsRequest {
    /// REQUIRED — must be specified explicitly. See struct docs.
    pub dry_run: bool,
    #[serde(default)]
    pub missing_only: bool,
    pub limit: Option<u32>,
}

impl AiPruneCheckpointsRequest {
    pub fn validate_admin(&self) -> crate::app::ServiceResult<()> {
        if !self.dry_run && !self.missing_only {
            return Err(crate::app::ServiceError::InvalidInput(
                "prune_ai_checkpoints requires missing_only=true for destructive runs".into(),
            ));
        }
        Ok(())
    }
}

/// Query parameters for `GET /api/db/integrity`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbIntegrityRequest {
    /// Use the fast `PRAGMA quick_check` path. `false` (or absent) runs full
    /// `PRAGMA integrity_check`.
    #[serde(default)]
    pub quick: bool,
}

/// JSON body for `POST /api/db/checkpoint`.
///
/// `mode` is validated at the handler entry against
/// `{passive, full, restart, truncate}` (bead 0p8r.4 #A17) — SQLite would
/// also reject unknown modes, but explicit handler-side validation produces
/// a clearer 400 with the allowed list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbCheckpointRequest {
    pub mode: String,
}

impl DbCheckpointRequest {
    pub const ALLOWED_MODES: &'static [&'static str] = &["passive", "full", "restart", "truncate"];

    pub fn normalized_mode(&self) -> crate::app::ServiceResult<String> {
        let mode = self.mode.to_ascii_lowercase();
        if Self::ALLOWED_MODES.contains(&mode.as_str()) {
            Ok(mode)
        } else {
            Err(crate::app::ServiceError::InvalidInput(format!(
                "mode must be one of: {}",
                Self::ALLOWED_MODES.join(", ")
            )))
        }
    }
}

/// JSON body for `POST /api/db/vacuum`.
///
/// `force` is intentionally `Option<bool>` (not `bool` with serde default):
/// the size pre-flight on `full == true` is bypassed ONLY when the body
/// explicitly carries `"force": true`. `None` and `Some(false)` both leave
/// the pre-flight in force, defending against accidental
/// `POST {"full":true}` on a multi-GB DB. See bead 0p8r.4 (eng-review C3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbVacuumRequest {
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub incremental_pages: u32,
    /// Must be `Some(true)` to bypass the 2 GB size pre-flight on full
    /// VACUUM. See struct docs.
    pub force: Option<bool>,
}

impl DbVacuumRequest {
    pub fn force_enabled(&self) -> bool {
        self.force == Some(true)
    }
}
