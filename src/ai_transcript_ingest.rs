//! Remote AI-transcript ingest (`POST /v1/ai-transcripts`) — receives a batch
//! of already-parsed AI transcript records forwarded by a satellite host's
//! `cortex agent` (see `agent::ai_transcript`) and inserts them into this
//! server's own log store via the same `db::insert_logs_batch` path used by
//! local `cortex sessions add`/`sessions watch`.
//!
//! This exists because AI transcript ingestion historically wrote directly to
//! a local SQLite file co-located with the server (`cortex::scanner` +
//! `cortex::ai_watch`), which only works when the watcher and the server run
//! on the same host. Once the server moves to a different host than the one
//! running Claude/Codex/Gemini, that local-write path silently orphans all
//! new transcript data. This endpoint gives every fleet host a way to forward
//! its transcripts to wherever the server actually lives, the same way
//! syslog/Docker/heartbeat data already does.
//!
//! Mounted on the shared HTTP listener (port 3100) next to MCP, OTLP,
//! heartbeats, and agent-commands. Auth mirrors heartbeats/agent-commands
//! (`src/heartbeat.rs`): static `CORTEX_TOKEN` bearer when configured,
//! loopback-only otherwise.

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
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{self, DbPool, LogBatchEntry};
use crate::mcp::AuthPolicy;

pub const AI_TRANSCRIPT_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// Caps record *count* per request, independent of the byte-size limit above,
/// following the same reasoning as `agent_command_ingest::MAX_RECORDS_PER_BATCH`:
/// bounds per-request DB work regardless of how small individual records are.
pub const MAX_RECORDS_PER_BATCH: usize = 2_000;

/// One parsed AI transcript line/event, forwarded by an agent's transcript
/// watcher. Mirrors the subset of `db::LogBatchEntry` relevant to AI
/// transcripts; the server fills in the rest (raw/facility/severity/etc.)
/// when mapping into `LogBatchEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiTranscriptRecord {
    /// RFC3339 timestamp; falls back to receipt time server-side if absent.
    pub timestamp: Option<String>,
    pub hostname: String,
    pub ai_tool: String,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub ai_transcript_path: String,
    #[serde(default)]
    pub event_kind: Option<String>,
    /// Scrubbed transcript message text (credential/token scrubbing happens
    /// agent-side before forwarding, same as the local scanner does today).
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiTranscriptIngestRequest {
    pub records: Vec<AiTranscriptRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiTranscriptIngestResponse {
    pub accepted: usize,
}

#[derive(Clone)]
pub struct AiTranscriptIngestState {
    pool: Arc<DbPool>,
    api_token: Option<String>,
    auth_policy: AuthPolicy,
}

impl AiTranscriptIngestState {
    pub fn new(pool: Arc<DbPool>, api_token: Option<String>, auth_policy: AuthPolicy) -> Self {
        Self {
            pool,
            api_token,
            auth_policy,
        }
    }
}

pub fn router(state: AiTranscriptIngestState) -> Router {
    use crate::surfaces::ContractRouterExt as _;
    Router::new()
        .contract_route("POST /v1/ai-transcripts", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(AI_TRANSCRIPT_BODY_LIMIT_BYTES))
        .with_state(state)
}

fn to_log_batch_entry(record: AiTranscriptRecord) -> LogBatchEntry {
    let timestamp = record
        .timestamp
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let source_ip = format!("agent-ai-transcript://{}", record.hostname);
    let metadata_json = crate::ingest_metadata::bounded_metadata_json(serde_json::json!({
        "source_type": "transcript",
        "event_kind": record.event_kind.as_deref().unwrap_or("unknown"),
        "content_scrubbed": true,
    }));
    LogBatchEntry {
        timestamp,
        hostname: record.hostname,
        facility: None,
        severity: "info".to_string(),
        app_name: Some(format!("{}-transcript", record.ai_tool)),
        process_id: None,
        message: record.message,
        raw: String::new(),
        source_ip,
        docker_checkpoint: None,
        ai_tool: Some(record.ai_tool),
        ai_project: record.ai_project,
        ai_session_id: record.ai_session_id,
        ai_transcript_path: Some(record.ai_transcript_path),
        metadata_json: Some(metadata_json),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

async fn ingest_handler(
    State(state): State<AiTranscriptIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&state, &peer, &headers) {
        return unauthorized();
    }

    let request: AiTranscriptIngestRequest = match serde_json::from_slice(&body) {
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

    let pool = Arc::clone(&state.pool);
    let entries: Vec<LogBatchEntry> = request
        .records
        .into_iter()
        .map(to_log_batch_entry)
        .collect();
    let join_result =
        tokio::task::spawn_blocking(move || db::insert_logs_batch(&pool, &entries)).await;

    match join_result {
        Ok(Ok(accepted)) => (
            StatusCode::OK,
            Json(AiTranscriptIngestResponse { accepted }),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "ai transcript forward ingest failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(join_error) => {
            tracing::error!(error = %join_error, "ai transcript ingest task panicked or was cancelled");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ingest_task_failed", "message": join_error.to_string()})),
            )
                .into_response()
        }
    }
}

fn is_authorized(state: &AiTranscriptIngestState, peer: &SocketAddr, headers: &HeaderMap) -> bool {
    if matches!(state.auth_policy, AuthPolicy::LoopbackDev) {
        return peer.ip().is_loopback();
    }
    let Some(expected) = state.api_token.as_deref() else {
        return false;
    };
    let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    parse_bearer_token(auth).is_some_and(|token| tokens_equal(&token, expected))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "ai_transcript_ingest_tests.rs"]
mod tests;
