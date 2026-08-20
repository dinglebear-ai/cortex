//! OTLP/HTTP trace request handling and Agent Observatory persistence.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Json},
};
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use parking_lot::Mutex;
use prost::Message;
use serde_json::json;

use crate::config::AgentObservatoryPrivacyConfig;
use crate::db::{DbPool, StorageBudgetState};

use super::OtlpState;
use super::auth::{is_authorized, unauthorized};
use super::traces::normalize_span_with_privacy;

/// Maximum spans accepted from one OTLP trace request.
pub(super) const MAX_SPANS_PER_REQUEST: usize = 5_000;

#[derive(Clone)]
pub(super) struct TraceIngestState {
    pool: Arc<DbPool>,
    storage_state: Arc<Mutex<Option<StorageBudgetState>>>,
    privacy: AgentObservatoryPrivacyConfig,
}

impl TraceIngestState {
    pub(super) fn new(
        pool: Arc<DbPool>,
        storage_state: Arc<Mutex<Option<StorageBudgetState>>>,
        privacy: AgentObservatoryPrivacyConfig,
    ) -> Self {
        Self {
            pool,
            storage_state,
            privacy,
        }
    }
}

pub(super) async fn traces_handler(
    State(state): State<OtlpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    if !is_authorized(&state, &headers) {
        return unauthorized();
    }
    if !is_protobuf_content_type(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({"error": "unsupported_content_type"})),
        )
            .into_response();
    }

    let Some(trace_ingest) = state.trace_ingest.clone() else {
        tracing::error!("OTLP trace ingest state is unavailable");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "trace_ingest_unavailable"})),
        )
            .into_response();
    };

    let decoded =
        tokio::task::spawn_blocking(move || ExportTraceServiceRequest::decode(body)).await;
    let req = match decoded {
        Ok(Ok(req)) => req,
        Ok(Err(err)) => {
            state
                .counters
                .decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(error = %err, source_ip = %peer, "OTLP /v1/traces decode failed");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "decode_failed"})),
            )
                .into_response();
        }
        Err(err) => {
            state
                .counters
                .decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(error = %err, "OTLP trace decode task panicked");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal"})),
            )
                .into_response();
        }
    };

    let received_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut normalized = Vec::new();
    let mut rejected = 0usize;
    let mut seen = 0usize;
    let mut rejected_invalid = false;
    let mut rejected_over_cap = false;

    for resource_spans in &req.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                seen += 1;
                if seen > MAX_SPANS_PER_REQUEST {
                    rejected += 1;
                    rejected_over_cap = true;
                    continue;
                }
                match normalize_span_with_privacy(
                    resource_spans.resource.as_ref(),
                    &resource_spans.schema_url,
                    scope_spans.scope.as_ref(),
                    &scope_spans.schema_url,
                    span,
                    &trace_ingest.privacy,
                    &received_at,
                ) {
                    Ok(span) => normalized.push(span),
                    Err(error) => {
                        rejected += 1;
                        rejected_invalid = true;
                        tracing::debug!(error = %error, source_ip = %peer, "Rejected invalid OTLP span");
                    }
                }
            }
        }
    }

    let mut messages = Vec::new();
    if rejected_invalid {
        messages.push("invalid spans rejected");
    }
    if rejected_over_cap {
        messages.push("request exceeded 5000 span limit");
    }

    if trace_ingest
        .storage_state
        .lock()
        .as_ref()
        .is_some_and(|state| state.write_blocked)
    {
        rejected += normalized.len();
        if !normalized.is_empty() {
            messages.push("trace storage temporarily blocked by configured storage budget");
        }
        tracing::warn!(
            source_ip = %peer,
            rejected,
            "OTLP trace persistence blocked by storage budget"
        );
        return trace_success_response(rejected, &messages);
    }

    let pool = Arc::clone(&trace_ingest.pool);
    let persisted = tokio::task::spawn_blocking(move || {
        crate::db::otlp_traces::insert_otel_spans_batch(&pool, &normalized)
    })
    .await;
    let result = match persisted {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::error!(error = %error, source_ip = %peer, "OTLP trace persistence failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "trace_persistence_failed"})),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(error = %error, "OTLP trace persistence task panicked");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal"})),
            )
                .into_response();
        }
    };

    rejected += result.rejected;
    if result.rejected > 0 {
        messages.push("spans rejected by storage validation");
    }
    tracing::info!(
        source_ip = %peer,
        accepted = result.accepted,
        duplicates = result.duplicates,
        rejected,
        "OTLP traces ingested"
    );
    trace_success_response(rejected, &messages)
}

fn is_protobuf_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| {
            media_type
                .trim()
                .eq_ignore_ascii_case("application/x-protobuf")
        })
}

fn trace_success_response(rejected: usize, messages: &[&str]) -> axum::response::Response {
    let partial_success = (rejected > 0).then(|| ExportTracePartialSuccess {
        rejected_spans: i64::try_from(rejected).unwrap_or(i64::MAX),
        error_message: messages.join("; "),
    });
    let response = ExportTraceServiceResponse { partial_success };
    (
        StatusCode::OK,
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-protobuf"),
        )],
        Bytes::from(response.encode_to_vec()),
    )
        .into_response()
}
