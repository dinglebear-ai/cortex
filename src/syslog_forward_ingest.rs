//! Receipt-backed syslog forwarding endpoint.
//!
//! The TCP syslog listener intentionally remains standards-compatible and
//! best-effort.  Agents that need replay send the same RFC5424 frame through
//! this authenticated endpoint, with a source-local sequence and stable
//! idempotency key.  The receiver commits the canonical log row and receipt in
//! one SQLite transaction, so a lost HTTP response is safe to retry.

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
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{self, DbPool};
use crate::enrich::{SourceKind, stamp_source_kind};
use crate::mcp::AuthPolicy;
use crate::receiver::parser::parse_syslog;

pub const SYSLOG_FORWARD_BODY_LIMIT_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS_PER_BATCH: usize = 200;
pub const MAX_GAPS_PER_BATCH: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SyslogForwardRecord {
    pub source_instance: String,
    pub source_epoch: u64,
    pub sequence: u64,
    pub idempotency_key: String,
    pub observed_at: String,
    /// An RFC5424 frame. Never include this in diagnostics or status output.
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SyslogForwardGap {
    pub source_instance: String,
    pub source_epoch: u64,
    pub from_sequence: u64,
    pub to_sequence: u64,
    pub idempotency_key: String,
    pub observed_at: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SyslogForwardRequest {
    #[serde(default)]
    pub records: Vec<SyslogForwardRecord>,
    #[serde(default)]
    pub gaps: Vec<SyslogForwardGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SyslogForwardResponse {
    pub receipts: Vec<String>,
}

#[derive(Clone)]
pub struct SyslogForwardIngestState {
    pool: Arc<DbPool>,
    api_token: Option<String>,
    auth_policy: AuthPolicy,
}

impl SyslogForwardIngestState {
    pub fn new(pool: Arc<DbPool>, api_token: Option<String>, auth_policy: AuthPolicy) -> Self {
        Self {
            pool,
            api_token,
            auth_policy,
        }
    }
}

pub fn router(state: SyslogForwardIngestState) -> Router {
    Router::new()
        .route("/v1/syslog-forward", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(SYSLOG_FORWARD_BODY_LIMIT_BYTES))
        .with_state(state)
}

async fn ingest_handler(
    State(state): State<SyslogForwardIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&state, &peer, &headers) {
        return unauthorized();
    }
    let request: SyslogForwardRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_payload"})),
            )
                .into_response();
        }
    };
    if request.records.len() > MAX_RECORDS_PER_BATCH || request.gaps.len() > MAX_GAPS_PER_BATCH {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "batch_too_large"})),
        )
            .into_response();
    }
    if request.records.iter().any(invalid_record) || request.gaps.iter().any(invalid_gap) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_record"})),
        )
            .into_response();
    }
    let pool = Arc::clone(&state.pool);
    let peer_ip = peer.ip().to_string();
    match tokio::task::spawn_blocking(move || persist_request(&pool, request, &peer_ip)).await {
        Ok(Ok(receipts)) => {
            (StatusCode::OK, Json(SyslogForwardResponse { receipts })).into_response()
        }
        Ok(Err(_)) => {
            tracing::error!(
                reason_code = "syslog_forward_ingest_failed",
                "syslog forwarding ingest failed"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(_) => {
            tracing::error!(
                reason_code = "syslog_forward_ingest_task_failed",
                "syslog forwarding ingest task failed"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ingest_task_failed"})),
            )
                .into_response()
        }
    }
}

fn invalid_record(record: &SyslogForwardRecord) -> bool {
    record.source_instance.is_empty()
        || record.source_instance.len() > 128
        || record.idempotency_key.is_empty()
        || record.idempotency_key.len() > 256
        || record.source_epoch > i64::MAX as u64
        || record.sequence > i64::MAX as u64
        || record.observed_at.len() > 64
        || record.line.is_empty()
        || record.line.len() > 64 * 1024
}

fn invalid_gap(gap: &SyslogForwardGap) -> bool {
    gap.source_instance.is_empty()
        || gap.source_instance.len() > 128
        || gap.idempotency_key.is_empty()
        || gap.idempotency_key.len() > 256
        || gap.source_epoch > i64::MAX as u64
        || gap.to_sequence > i64::MAX as u64
        || gap.observed_at.len() > 64
        || gap.reason_code.len() > 64
        || gap.from_sequence > gap.to_sequence
}

fn persist_request(
    pool: &DbPool,
    request: SyslogForwardRequest,
    peer_ip: &str,
) -> anyhow::Result<Vec<String>> {
    let mut conn = db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let mut receipts = Vec::with_capacity(request.records.len() + request.gaps.len());
    for record in request.records {
        let receipt_key = opaque_receipt_value(&record.idempotency_key);
        let source_identity = opaque_receipt_value(&record.source_instance);
        if receipt_exists(&tx, &receipt_key)? {
            receipts.push(record.idempotency_key);
            continue;
        }
        let mut entry = parse_syslog(&record.line, format!("agent-syslog://{peer_ip}"));
        stamp_source_kind(&mut entry, SourceKind::SyslogTcp);
        let ids = db::insert_logs_batch_in_tx(&tx, &[entry])?;
        insert_receipt(
            &tx,
            &receipt_key,
            &source_identity,
            record.source_epoch,
            record.sequence,
            ids[0],
            "record",
        )?;
        receipts.push(record.idempotency_key);
    }
    for gap in request.gaps {
        let receipt_key = opaque_receipt_value(&gap.idempotency_key);
        let source_identity = opaque_receipt_value(&gap.source_instance);
        if receipt_exists(&tx, &receipt_key)? {
            receipts.push(gap.idempotency_key);
            continue;
        }
        // This is deliberately payload-free: it makes the exact loss window
        // queryable without leaking a dropped record into status/diagnostics.
        let entry = crate::db::LogBatchEntry {
            timestamp: gap.observed_at.clone(),
            hostname: source_identity.clone(),
            facility: Some("local0".into()),
            severity: "warning".into(),
            app_name: Some("cortex-agent-forward".into()),
            process_id: None,
            message: format!(
                "syslog forwarding retention gap: sequence {} through {} ({})",
                gap.from_sequence, gap.to_sequence, gap.reason_code
            ),
            raw: String::new(),
            source_ip: format!("agent-syslog://{peer_ip}"),
            docker_checkpoint: None,
            ai_tool: None,
            ai_project: None,
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: Some(crate::ingest_metadata::bounded_metadata_json(
                json!({"source_kind":"syslog-forward-gap", "reason_code": gap.reason_code, "from_sequence": gap.from_sequence, "to_sequence": gap.to_sequence}),
            )),
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        };
        let ids = db::insert_logs_batch_in_tx(&tx, &[entry])?;
        insert_receipt(
            &tx,
            &receipt_key,
            &source_identity,
            gap.source_epoch,
            gap.to_sequence,
            ids[0],
            "gap",
        )?;
        receipts.push(gap.idempotency_key);
    }
    tx.commit()?;
    crate::db::agent_observatory::notify_projection_work();
    Ok(receipts)
}

fn opaque_receipt_value(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(&digest[..16])
}

fn receipt_exists(tx: &rusqlite::Transaction<'_>, key: &str) -> anyhow::Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM syslog_forward_receipts WHERE idempotency_key = ?1)",
        [key],
        |row| row.get(0),
    )?)
}

fn insert_receipt(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    source: &str,
    epoch: u64,
    sequence: u64,
    log_id: i64,
    kind: &str,
) -> anyhow::Result<()> {
    tx.execute("INSERT INTO syslog_forward_receipts (idempotency_key, source_instance, source_epoch, sequence, canonical_log_id, receipt_kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![key, source, epoch as i64, sequence as i64, log_id, kind])?;
    Ok(())
}

fn is_authorized(state: &SyslogForwardIngestState, peer: &SocketAddr, headers: &HeaderMap) -> bool {
    if matches!(state.auth_policy, AuthPolicy::LoopbackDev) {
        return peer.ip().is_loopback();
    }
    let Some(expected) = state.api_token.as_deref() else {
        return false;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer_token)
        .is_some_and(|token| tokens_equal(&token, expected))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "syslog_forward_ingest_tests.rs"]
mod tests;
