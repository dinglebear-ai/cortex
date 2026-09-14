//! Remote shell-history ingest (`POST /v1/shell-history`) — receives a batch
//! of pre-parsed zsh/bash extended-history or atuin records forwarded by a
//! satellite host's `cortex agent` (see `agent::shell_history`) and inserts
//! them into this server's own log store via `db::insert_logs_batch`, the
//! same path `cortex ingest shell user index`/`atuinindex` use locally.
//!
//! Local `cortex ingest shell user index`/`atuinindex` have no forward
//! mode at all — this endpoint exists so a host's own interactive command
//! history reaches wherever the shared cortex server actually lives, the
//! same way syslog/Docker/heartbeat/AI-transcript/agent-command data does.
//!
//! Mounted on the shared HTTP listener (port 3100) next to MCP, OTLP,
//! heartbeats, agent-commands, and AI-transcripts. Auth mirrors those:
//! static `CORTEX_TOKEN` bearer when configured, loopback-only otherwise.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::surfaces::post;
use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use lab_auth::middleware::{parse_bearer_token, tokens_equal};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{self, DbPool, LogBatchEntry};
use crate::mcp::AuthPolicy;
use crate::syslog_forward_ingest::ForwardingPrincipal;

pub const SHELL_HISTORY_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Caps record *count* per request, matching the reasoning used by the
/// agent-command and AI-transcript ingest endpoints.
pub const MAX_RECORDS_PER_BATCH: usize = 2_000;

/// One shell-history entry, forwarded by an agent's shell-history watcher.
/// Already scrubbed of common credential patterns agent-side (see
/// `command_log::scrub_command`) before it ever reaches the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellHistoryRecord {
    /// Stable source-local record identity used for safe HTTP retries. Older
    /// agents omit this field and retain the legacy at-least-once behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// `"zsh"`, `"bash"`, or `"atuin"`.
    pub source: String,
    pub hostname: String,
    /// RFC3339 timestamp the command started.
    pub timestamp: String,
    pub duration_ms: Option<u64>,
    /// Already-scrubbed command text.
    pub command: String,
    pub cwd: Option<String>,
    pub exit_status: Option<i32>,
    /// Atuin session id, when the source is atuin.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellHistoryIngestRequest {
    pub records: Vec<ShellHistoryRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellHistoryIngestResponse {
    pub accepted: usize,
}

#[derive(Clone)]
pub struct ShellHistoryIngestState {
    pool: Arc<DbPool>,
    api_token: Option<String>,
    forwarding_agent_tokens: Arc<HashMap<String, String>>,
    auth_policy: AuthPolicy,
    storage: Arc<parking_lot::Mutex<Option<crate::db::StorageBudgetState>>>,
}

impl ShellHistoryIngestState {
    pub fn new(
        pool: Arc<DbPool>,
        api_token: Option<String>,
        forwarding_agent_tokens: HashMap<String, String>,
        auth_policy: AuthPolicy,
        storage: Arc<parking_lot::Mutex<Option<crate::db::StorageBudgetState>>>,
    ) -> Self {
        Self {
            pool,
            api_token,
            forwarding_agent_tokens: Arc::new(forwarding_agent_tokens),
            auth_policy,
            storage,
        }
    }
}

pub fn router(state: ShellHistoryIngestState) -> Router {
    use crate::surfaces::ContractRouterExt as _;
    Router::new()
        .contract_route("POST /v1/shell-history", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(SHELL_HISTORY_BODY_LIMIT_BYTES))
        .with_state(state)
}

fn to_log_batch_entry(record: ShellHistoryRecord) -> LogBatchEntry {
    let source_ip = format!("agent-shell-history://{}", record.hostname);
    let severity = match record.exit_status {
        Some(0) => "info",
        Some(_) => "warning",
        None => "info",
    };
    let metadata_json = crate::ingest_metadata::bounded_metadata_json(serde_json::json!({
        "source_type": "shell_history",
        "shell": record.source,
        "cwd": record.cwd,
        "exit_status": record.exit_status,
        "duration_ms": record.duration_ms,
        "content_scrubbed": true,
    }));
    LogBatchEntry {
        timestamp: record.timestamp,
        hostname: record.hostname,
        facility: Some("shell".to_string()),
        severity: severity.to_string(),
        app_name: Some(record.source),
        process_id: None,
        message: record.command.clone(),
        raw: record.command,
        source_ip,
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: record.session_id,
        ai_transcript_path: None,
        metadata_json: Some(metadata_json),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

async fn ingest_handler(
    State(state): State<ShellHistoryIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(principal) = authenticated_forwarder(&state, &peer, &headers) else {
        return unauthorized();
    };

    let request: ShellHistoryIngestRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_payload", "message": error.to_string()})),
            )
                .into_response();
        }
    };

    if request.records.len() > MAX_RECORDS_PER_BATCH {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "batch_too_large",
                "message": format!(
                    "batch has {} records, exceeds the {MAX_RECORDS_PER_BATCH}-record limit per request",
                    request.records.len()
                ),
            })),
        )
            .into_response();
    }
    if let Err(message) = validate_idempotency_keys(&request.records) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_payload", "message": message})),
        )
            .into_response();
    }
    let pool = Arc::clone(&state.pool);
    let storage = Arc::clone(&state.storage);
    let entries: Vec<LogBatchEntry> = request
        .records
        .iter()
        .cloned()
        .map(to_log_batch_entry)
        .collect();
    let records = request.records;
    let receipt_namespace = principal.receipt_namespace();
    let join_result = tokio::task::spawn_blocking(move || {
        persist_idempotent(&pool, &storage, &entries, &records, &receipt_namespace)
    })
    .await;

    match join_result {
        Ok(Ok(accepted)) => (
            StatusCode::OK,
            Json(ShellHistoryIngestResponse { accepted }),
        )
            .into_response(),
        Ok(Err(error)) if error.downcast_ref::<StorageBlocked>().is_some() => (
            StatusCode::SERVICE_UNAVAILABLE,
            [("retry-after", "5")],
            Json(json!({"error": "storage_write_blocked"})),
        )
            .into_response(),
        Ok(Err(error)) if error.downcast_ref::<IdempotencyConflict>().is_some() => (
            StatusCode::CONFLICT,
            Json(json!({"error": "idempotency_conflict"})),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "shell history forward ingest failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(join_error) => {
            tracing::error!(error = %join_error, "shell history ingest task panicked or was cancelled");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ingest_task_failed", "message": join_error.to_string()})),
            )
                .into_response()
        }
    }
}

fn validate_idempotency_keys(records: &[ShellHistoryRecord]) -> Result<(), &'static str> {
    for record in records {
        if let Some(key) = record.idempotency_key.as_deref()
            && (key.is_empty() || key.len() > 256)
        {
            return Err("idempotency_key must contain between 1 and 256 bytes");
        }
    }
    Ok(())
}

fn persist_idempotent(
    pool: &DbPool,
    storage: &parking_lot::Mutex<Option<crate::db::StorageBudgetState>>,
    entries: &[LogBatchEntry],
    records: &[ShellHistoryRecord],
    receipt_namespace: &str,
) -> anyhow::Result<usize> {
    let mut conn = db::write_conn(pool)?;
    if storage
        .lock()
        .as_ref()
        .is_some_and(|state| state.write_blocked)
    {
        return Err(StorageBlocked.into());
    }
    let tx = conn.transaction()?;
    let mut pending = Vec::with_capacity(entries.len());
    let mut request_receipts: HashMap<String, (String, String)> = HashMap::new();

    for (index, record) in records.iter().enumerate() {
        let Some(key) = record.idempotency_key.as_deref() else {
            pending.push((index, None));
            continue;
        };
        debug_assert!(!key.is_empty() && key.len() <= 256);
        let source_identity = opaque_receipt_value(&format!(
            "{receipt_namespace}\0{}\0{}",
            record.hostname, record.source
        ));
        let receipt_key = opaque_receipt_value(&format!("shell-history\0{source_identity}\0{key}"));
        let fingerprint = request_fingerprint(receipt_namespace, record)?;
        tx.execute(
            "DELETE FROM syslog_forward_receipts
             WHERE idempotency_key = ?1
               AND NOT EXISTS (SELECT 1 FROM logs WHERE id = canonical_log_id)",
            [&receipt_key],
        )?;
        let stored = tx
            .query_row(
                "SELECT source_instance, receipt_kind, request_fingerprint
                 FROM syslog_forward_receipts WHERE idempotency_key = ?1",
                [&receipt_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        match stored {
            None => {}
            Some((stored_source, stored_kind, stored_fingerprint))
                if stored_source == source_identity
                    && stored_kind == "record"
                    && stored_fingerprint == fingerprint =>
            {
                continue;
            }
            Some(_) => return Err(IdempotencyConflict.into()),
        }

        if let Some((pending_source, pending_fingerprint)) = request_receipts.get(&receipt_key) {
            if pending_source == &source_identity && pending_fingerprint == &fingerprint {
                continue;
            }
            return Err(IdempotencyConflict.into());
        }
        request_receipts.insert(
            receipt_key.clone(),
            (source_identity.clone(), fingerprint.clone()),
        );
        pending.push((
            index,
            Some(PendingReceipt {
                receipt_key,
                source_identity,
                fingerprint,
            }),
        ));
    }

    let pending_entries: Vec<_> = pending.iter().map(|(index, _)| &entries[*index]).collect();
    let ids = db::insert_logs_batch_in_tx(&tx, &pending_entries)?;
    for ((_, receipt), canonical_log_id) in pending.iter().zip(&ids) {
        let Some(receipt) = receipt else {
            continue;
        };
        tx.execute(
            "INSERT INTO syslog_forward_receipts
             (idempotency_key, source_instance, source_epoch, sequence,
              canonical_log_id, receipt_kind, request_fingerprint)
             VALUES (?1, ?2, 0, 0, ?3, 'record', ?4)",
            rusqlite::params![
                receipt.receipt_key,
                receipt.source_identity,
                canonical_log_id,
                receipt.fingerprint
            ],
        )?;
    }
    let accepted = ids.len();
    tx.commit()?;
    if accepted > 0 {
        crate::db::agent_observatory::notify_projection_work();
    }
    Ok(accepted)
}

struct PendingReceipt {
    receipt_key: String,
    source_identity: String,
    fingerprint: String,
}

fn opaque_receipt_value(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn request_fingerprint(
    receipt_namespace: &str,
    record: &ShellHistoryRecord,
) -> anyhow::Result<String> {
    Ok(opaque_receipt_value(&format!(
        "{receipt_namespace}\0{}",
        serde_json::to_string(record)?
    )))
}

#[derive(Debug)]
struct IdempotencyConflict;

impl std::fmt::Display for IdempotencyConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("idempotency_conflict")
    }
}

impl std::error::Error for IdempotencyConflict {}

#[derive(Debug)]
struct StorageBlocked;

impl std::fmt::Display for StorageBlocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("storage_write_blocked")
    }
}

impl std::error::Error for StorageBlocked {}

fn authenticated_forwarder(
    state: &ShellHistoryIngestState,
    peer: &SocketAddr,
    headers: &HeaderMap,
) -> Option<ForwardingPrincipal> {
    if matches!(state.auth_policy, AuthPolicy::LoopbackDev) {
        return peer
            .ip()
            .is_loopback()
            .then_some(ForwardingPrincipal::Loopback);
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer_token)?;
    if let Some(identity) = state
        .forwarding_agent_tokens
        .iter()
        .find_map(|(expected, identity)| tokens_equal(&token, expected).then(|| identity.clone()))
    {
        return Some(ForwardingPrincipal::Named(identity));
    }
    state
        .api_token
        .as_deref()
        .filter(|expected| tokens_equal(&token, expected))
        .map(|_| ForwardingPrincipal::SharedBearer)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "shell_history_ingest_tests.rs"]
mod tests;
