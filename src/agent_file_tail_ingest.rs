//! Authenticated ingest for configured file tails forwarded by fleet agents.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::post,
};
use bytes::Bytes;
use lab_auth::middleware::{parse_bearer_token, tokens_equal};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{self, DbPool, LogBatchEntry};
use crate::enrich::{SourceKind, stamp_source_kind};
use crate::mcp::AuthPolicy;
use crate::syslog_forward_ingest::ForwardingPrincipal;

pub const BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RECORDS_PER_BATCH: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileTailRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub hostname: String,
    pub source_id: String,
    pub tag: String,
    pub path_basename: String,
    pub timestamp: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileTailIngestRequest {
    pub records: Vec<AgentFileTailRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentFileTailIngestResponse {
    pub accepted: usize,
}

#[derive(Clone)]
pub struct AgentFileTailIngestState {
    pool: Arc<DbPool>,
    api_token: Option<String>,
    forwarding_agent_tokens: Arc<HashMap<String, String>>,
    auth_policy: AuthPolicy,
    storage: Arc<parking_lot::Mutex<Option<crate::db::StorageBudgetState>>>,
    enrichment: crate::receiver::enrichment::EnrichmentConfig,
    pipeline: Arc<crate::enrich::EnrichmentPipeline>,
}

impl AgentFileTailIngestState {
    pub fn new(
        pool: Arc<DbPool>,
        api_token: Option<String>,
        forwarding_agent_tokens: HashMap<String, String>,
        auth_policy: AuthPolicy,
        storage: Arc<parking_lot::Mutex<Option<crate::db::StorageBudgetState>>>,
        enrichment: crate::receiver::enrichment::EnrichmentConfig,
    ) -> Self {
        Self {
            pool,
            api_token,
            forwarding_agent_tokens: Arc::new(forwarding_agent_tokens),
            auth_policy,
            storage,
            enrichment,
            pipeline: Arc::new(crate::enrich::EnrichmentPipeline::new()),
        }
    }
}

pub fn router(state: AgentFileTailIngestState) -> Router {
    Router::new()
        .route("/v1/file-tails", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES))
        .with_state(state)
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn to_log_batch_entry(record: AgentFileTailRecord) -> Result<LogBatchEntry, &'static str> {
    if !valid_identity(&record.hostname)
        || !valid_identity(&record.source_id)
        || record
            .idempotency_key
            .as_deref()
            .is_some_and(|key| !valid_identity(key))
        || record.tag.trim().is_empty()
        || record.tag.len() > 255
        || record.path_basename.trim().is_empty()
        || record.path_basename.len() > 255
    {
        return Err("invalid file-tail identity");
    }
    let metadata_json = crate::ingest_metadata::bounded_metadata_json(json!({
        "source_type": "agent_file_tail",
        "source_kind": SourceKind::AgentFileTail.as_str(),
        "file_tail_id": record.source_id,
        "tag": record.tag,
        "path_basename": record.path_basename,
    }));
    let mut entry = LogBatchEntry {
        timestamp: record.timestamp,
        hostname: record.hostname.clone(),
        facility: Some("local0".to_string()),
        severity: "info".to_string(),
        app_name: Some(record.tag),
        process_id: None,
        message: record.message.clone(),
        raw: record.message,
        source_ip: format!("agent-file-tail://{}/{}", record.hostname, record.source_id),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(metadata_json),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    };
    stamp_source_kind(&mut entry, SourceKind::AgentFileTail);
    Ok(entry)
}

async fn ingest_handler(
    State(state): State<AgentFileTailIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(principal) = authenticated_forwarder(&state, &peer, &headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    };
    let receipt_namespace = principal.receipt_namespace();
    let request: AgentFileTailIngestRequest = match serde_json::from_slice(&body) {
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
            Json(json!({"error": "batch_too_large"})),
        )
            .into_response();
    }
    let mut entries: Vec<_> = match request
        .records
        .iter()
        .cloned()
        .map(to_log_batch_entry)
        .collect()
    {
        Ok(entries) => entries,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_payload", "message": message})),
            )
                .into_response();
        }
    };
    for entry in &mut entries {
        *entry = crate::receiver::enrichment::enrich_entry(entry.clone(), &state.enrichment);
        state.pipeline.dispatch(entry);
    }
    let pool = Arc::clone(&state.pool);
    let storage = Arc::clone(&state.storage);
    let records = request.records;
    match tokio::task::spawn_blocking(move || {
        persist_idempotent(&pool, &storage, &entries, &records, &receipt_namespace)
    })
    .await
    {
        Ok(Ok(accepted)) => (
            StatusCode::OK,
            Json(AgentFileTailIngestResponse { accepted }),
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
            tracing::error!(error = %error, "agent file-tail ingest failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "agent file-tail ingest task failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ingest_task_failed"})),
            )
                .into_response()
        }
    }
}

fn persist_idempotent(
    pool: &DbPool,
    storage: &parking_lot::Mutex<Option<crate::db::StorageBudgetState>>,
    entries: &[LogBatchEntry],
    records: &[AgentFileTailRecord],
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
    let mut pending_receipts: HashMap<String, (String, String)> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        let Some(key) = record.idempotency_key.as_deref() else {
            pending.push((index, None));
            continue;
        };
        let source_identity = opaque_receipt_value(&format!(
            "{receipt_namespace}\0{}\0{}",
            record.hostname, record.source_id
        ));
        let receipt_key =
            opaque_receipt_value(&format!("agent-file-tail\0{source_identity}\0{key}"));
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
        if let Some((pending_source, pending_fingerprint)) = pending_receipts.get(&receipt_key) {
            if pending_source == &source_identity && pending_fingerprint == &fingerprint {
                continue;
            }
            return Err(IdempotencyConflict.into());
        }
        pending_receipts.insert(
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
            "INSERT INTO syslog_forward_receipts (idempotency_key, source_instance, source_epoch, sequence, canonical_log_id, receipt_kind, request_fingerprint) VALUES (?1, ?2, 0, 0, ?3, 'record', ?4)",
            rusqlite::params![
                receipt.receipt_key,
                receipt.source_identity,
                canonical_log_id,
                receipt.fingerprint
            ],
        )?;
    }
    tx.commit()?;
    if !ids.is_empty() {
        crate::db::agent_observatory::notify_projection_work();
    }
    Ok(ids.len())
}

struct PendingReceipt {
    receipt_key: String,
    source_identity: String,
    fingerprint: String,
}

fn opaque_receipt_value(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[derive(Debug)]
struct StorageBlocked;

impl std::fmt::Display for StorageBlocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("storage_write_blocked")
    }
}

impl std::error::Error for StorageBlocked {}

fn request_fingerprint(
    receipt_namespace: &str,
    record: &AgentFileTailRecord,
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

fn authenticated_forwarder(
    state: &AgentFileTailIngestState,
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

#[cfg(test)]
#[path = "agent_file_tail_ingest_tests.rs"]
mod tests;
