//! Receipt-backed syslog forwarding endpoint.
//!
//! The TCP syslog listener intentionally remains standards-compatible and
//! best-effort.  Agents that need replay send the same RFC5424 frame through
//! this authenticated endpoint, with a source-local sequence and stable
//! idempotency key.  The receiver commits the canonical log row and receipt in
//! one SQLite transaction, so a lost HTTP response is safe to retry.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use lab_auth::middleware::{parse_bearer_token, tokens_equal};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{self, DbPool};
use crate::enrich::{SourceKind, stamp_source_kind};
use crate::mcp::AuthPolicy;
use crate::receiver::parser::parse_syslog;
use crate::surfaces::post;

pub const SYSLOG_FORWARD_BODY_LIMIT_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS_PER_BATCH: usize = 200;
pub const MAX_GAPS_PER_BATCH: usize = 50;

#[derive(Debug)]
struct IdempotencyConflict;

impl std::fmt::Display for IdempotencyConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("idempotency_conflict")
    }
}

impl std::error::Error for IdempotencyConflict {}

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
    forwarding_agent_tokens: Arc<HashMap<String, String>>,
    auth_policy: AuthPolicy,
}

impl SyslogForwardIngestState {
    pub fn new(
        pool: Arc<DbPool>,
        api_token: Option<String>,
        forwarding_agent_tokens: HashMap<String, String>,
        auth_policy: AuthPolicy,
    ) -> Self {
        Self {
            pool,
            api_token,
            forwarding_agent_tokens: Arc::new(forwarding_agent_tokens),
            auth_policy,
        }
    }
}

pub fn router(state: SyslogForwardIngestState) -> Router {
    use crate::surfaces::ContractRouterExt as _;
    Router::new()
        .contract_route("POST /v1/syslog-forward", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(SYSLOG_FORWARD_BODY_LIMIT_BYTES))
        .with_state(state)
}

async fn ingest_handler(
    State(state): State<SyslogForwardIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(forwarder_identity) = authenticated_forwarder(&state, &peer, &headers) else {
        return unauthorized();
    };
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
    let record_count = request.records.len();
    let gap_count = request.gaps.len();
    let diagnostic_identity = forwarder_identity.clone();
    match tokio::task::spawn_blocking(move || {
        persist_request(&pool, request, &peer_ip, &forwarder_identity)
    })
    .await
    {
        Ok(Ok(receipts)) => {
            (StatusCode::OK, Json(SyslogForwardResponse { receipts })).into_response()
        }
        Ok(Err(error)) if error.downcast_ref::<IdempotencyConflict>().is_some() => (
            StatusCode::CONFLICT,
            Json(json!({"error": "idempotency_conflict"})),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::error!(
                reason_code = "syslog_forward_ingest_failed",
                error = %error,
                forwarder = %diagnostic_identity,
                peer = %peer,
                record_count,
                gap_count,
                "syslog forwarding ingest failed"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(join_error) => {
            tracing::error!(
                reason_code = "syslog_forward_ingest_task_failed",
                error = %join_error,
                forwarder = %diagnostic_identity,
                peer = %peer,
                record_count,
                gap_count,
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
        || !valid_observed_at(&record.observed_at)
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
        || !valid_observed_at(&gap.observed_at)
        || !matches!(
            gap.reason_code.as_str(),
            "local_retention_quota" | "aggregate_retention_quota" | "record_too_large"
        )
        || gap.from_sequence > gap.to_sequence
}

fn valid_observed_at(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn persist_request(
    pool: &DbPool,
    request: SyslogForwardRequest,
    peer_ip: &str,
    forwarder_identity: &str,
) -> anyhow::Result<Vec<String>> {
    let mut conn = db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let mut receipts = Vec::with_capacity(request.records.len() + request.gaps.len());
    for record in request.records {
        let receipt_key = opaque_receipt_value(&record.idempotency_key);
        let source_identity =
            opaque_receipt_value(&format!("{forwarder_identity}:{}", record.source_instance));
        let fingerprint = request_fingerprint(forwarder_identity, &record)?;
        if receipt_replay(
            &tx,
            &receipt_key,
            &source_identity,
            record.source_epoch,
            record.sequence,
            "record",
            &fingerprint,
        )? {
            receipts.push(record.idempotency_key);
            continue;
        }
        let mut entry = parse_syslog(&record.line, format!("agent-syslog://{peer_ip}"));
        let claimed_hostname = entry.hostname.clone();
        entry.hostname = format!("agent-{forwarder_identity}");
        entry.metadata_json = Some(forwarded_metadata(
            entry.metadata_json.as_deref(),
            forwarder_identity,
            peer_ip,
            claimed_hostname,
        )?);
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
            &fingerprint,
        )?;
        receipts.push(record.idempotency_key);
    }
    for gap in request.gaps {
        let receipt_key = opaque_receipt_value(&gap.idempotency_key);
        let source_identity =
            opaque_receipt_value(&format!("{forwarder_identity}:{}", gap.source_instance));
        let fingerprint = request_fingerprint(forwarder_identity, &gap)?;
        if receipt_replay(
            &tx,
            &receipt_key,
            &source_identity,
            gap.source_epoch,
            gap.to_sequence,
            "gap",
            &fingerprint,
        )? {
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
            &fingerprint,
        )?;
        receipts.push(gap.idempotency_key);
    }
    tx.commit()?;
    crate::db::agent_observatory::notify_projection_work();
    Ok(receipts)
}

fn forwarded_metadata(
    existing: Option<&str>,
    forwarder_identity: &str,
    peer_ip: &str,
    hostname_claim: String,
) -> anyhow::Result<String> {
    let mut metadata = match existing {
        Some(encoded) => serde_json::from_str::<Value>(encoded)?
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("parsed syslog metadata is not a JSON object"))?,
        None => serde_json::Map::new(),
    };
    metadata.insert(
        "forwarded_provenance".into(),
        json!({
            "authenticated_forwarder": forwarder_identity,
            "transport_peer": peer_ip,
            "hostname_claim": hostname_claim,
            "trust": if forwarder_identity == "shared_bearer" { "claimed" } else { "verified_forwarder_claimed_host" },
        }),
    );
    Ok(crate::ingest_metadata::bounded_metadata_json(
        Value::Object(metadata),
    ))
}

fn opaque_receipt_value(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(&digest[..16])
}

fn request_fingerprint<T: Serialize>(identity: &str, value: &T) -> anyhow::Result<String> {
    Ok(opaque_receipt_value(&format!(
        "{identity}:{}",
        serde_json::to_string(value)?
    )))
}

fn receipt_replay(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    source: &str,
    epoch: u64,
    sequence: u64,
    kind: &str,
    fingerprint: &str,
) -> anyhow::Result<bool> {
    let stored = tx
        .query_row(
            "SELECT source_instance, source_epoch, sequence, receipt_kind, request_fingerprint FROM syslog_forward_receipts WHERE idempotency_key = ?1",
            [key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)),
        )
        .optional()?;
    match stored {
        None => Ok(false),
        Some((stored_source, stored_epoch, stored_sequence, stored_kind, stored_fingerprint))
            if stored_source == source
                && stored_epoch == epoch as i64
                && stored_sequence == sequence as i64
                && stored_kind == kind
                && (stored_fingerprint.is_empty() || stored_fingerprint == fingerprint) =>
        {
            Ok(true)
        }
        Some(_) => Err(anyhow::Error::new(IdempotencyConflict)),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_receipt(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    source: &str,
    epoch: u64,
    sequence: u64,
    log_id: i64,
    kind: &str,
    fingerprint: &str,
) -> anyhow::Result<()> {
    tx.execute("INSERT INTO syslog_forward_receipts (idempotency_key, source_instance, source_epoch, sequence, canonical_log_id, receipt_kind, request_fingerprint) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![key, source, epoch as i64, sequence as i64, log_id, kind, fingerprint])?;
    Ok(())
}

fn authenticated_forwarder(
    state: &SyslogForwardIngestState,
    peer: &SocketAddr,
    headers: &HeaderMap,
) -> Option<String> {
    if matches!(state.auth_policy, AuthPolicy::LoopbackDev) {
        return peer.ip().is_loopback().then(|| "loopback".to_string());
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
        return Some(identity);
    }
    state
        .api_token
        .as_deref()
        .filter(|expected| tokens_equal(&token, expected))
        .map(|_| "shared_bearer".to_string())
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
