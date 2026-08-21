//! OTLP/HTTP metric request handling and Agent Observatory persistence.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Json},
};
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use parking_lot::Mutex;
use prost::Message;
use serde_json::json;

use crate::{
    config::AgentObservatoryPrivacyConfig,
    db::{DbPool, StorageBudgetState},
};

use super::{
    OtlpState,
    auth::{is_authorized, unauthorized},
    metrics::normalize_metric_with_privacy,
};

impl From<super::metrics::MetricPointInput> for crate::db::otlp_metrics::OtelMetricPointInput {
    fn from(point: super::metrics::MetricPointInput) -> Self {
        Self {
            point_key: point.point_key,
            metric_name: point.metric_name,
            description: point.description,
            unit: point.unit,
            instrument_kind: point.instrument_kind,
            aggregation_temporality: point.aggregation_temporality,
            monotonic: point.monotonic,
            start_time_unix_nano: point.start_time_unix_nano,
            time_unix_nano: point.time_unix_nano,
            hostname: point.hostname,
            service_name: point.service_name,
            service_version: point.service_version,
            scope_name: point.scope_name,
            scope_version: point.scope_version,
            ai_tool: point.ai_tool,
            ai_project: point.ai_project,
            ai_session_id: point.ai_session_id,
            run_id: point.run_id,
            resource_json: point.resource_json,
            attributes_json: point.attributes_json,
            value_json: point.value_json,
            exemplars_json: point.exemplars_json,
            received_at: point.received_at,
            content_scrubbed: point.content_scrubbed,
        }
    }
}

pub(super) const MAX_METRIC_POINTS_PER_REQUEST: usize = 5_000;

#[derive(Clone)]
pub(super) struct MetricIngestState {
    pool: Arc<DbPool>,
    storage_state: Arc<Mutex<Option<StorageBudgetState>>>,
    privacy: AgentObservatoryPrivacyConfig,
}

impl MetricIngestState {
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

pub(super) async fn metrics_handler(
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
    let Some(ingest) = state.metric_ingest.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "metric_ingest_unavailable"})),
        )
            .into_response();
    };
    let decoded =
        tokio::task::spawn_blocking(move || ExportMetricsServiceRequest::decode(body)).await;
    let req = match decoded {
        Ok(Ok(req)) => req,
        Ok(Err(error)) => {
            state
                .counters
                .decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(error = %error, source_ip = %peer, "OTLP /v1/metrics decode failed");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "decode_failed"})),
            )
                .into_response();
        }
        Err(error) => {
            state
                .counters
                .decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(error = %error, "OTLP metric decode task panicked");
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
    let mut invalid = false;
    let mut over_cap = false;
    for resource in &req.resource_metrics {
        for scope in &resource.scope_metrics {
            for metric in &scope.metrics {
                match normalize_metric_with_privacy(
                    resource.resource.as_ref(),
                    &resource.schema_url,
                    scope.scope.as_ref(),
                    &scope.schema_url,
                    metric,
                    &ingest.privacy,
                    &received_at,
                ) {
                    Ok(points) => {
                        for point in points {
                            seen += 1;
                            if seen > MAX_METRIC_POINTS_PER_REQUEST {
                                rejected += 1;
                                over_cap = true;
                            } else {
                                normalized.push(point.into());
                            }
                        }
                    }
                    Err(error) => {
                        rejected += metric_point_count(metric).max(1);
                        invalid = true;
                        tracing::debug!(error = %error, source_ip = %peer, "Rejected invalid OTLP metric");
                    }
                }
            }
        }
    }
    let mut messages = Vec::new();
    if invalid {
        messages.push("invalid metric points rejected");
    }
    if over_cap {
        messages.push("request exceeded 5000 metric point limit");
    }
    if ingest
        .storage_state
        .lock()
        .as_ref()
        .is_some_and(|state| state.write_blocked)
    {
        rejected += normalized.len();
        if !normalized.is_empty() {
            messages.push("metric storage temporarily blocked by configured storage budget");
        }
        return metric_success_response(rejected, &messages);
    }
    let pool = Arc::clone(&ingest.pool);
    let persisted = tokio::task::spawn_blocking(move || {
        crate::db::otlp_metrics::insert_otel_metric_points_batch(&pool, &normalized)
    })
    .await;
    let result = match persisted {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::error!(error = %error, source_ip = %peer, "OTLP metric persistence failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "metric_persistence_failed"})),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(error = %error, "OTLP metric persistence task panicked");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal"})),
            )
                .into_response();
        }
    };
    rejected += result.rejected;
    if result.rejected > 0 {
        messages.push("metric points rejected by storage validation");
    }
    tracing::info!(source_ip = %peer, accepted = result.accepted, duplicates = result.duplicates, rejected, "OTLP metrics ingested");
    metric_success_response(rejected, &messages)
}

fn metric_point_count(metric: &opentelemetry_proto::tonic::metrics::v1::Metric) -> usize {
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    match metric.data.as_ref() {
        Some(Data::Gauge(value)) => value.data_points.len(),
        Some(Data::Sum(value)) => value.data_points.len(),
        Some(Data::Histogram(value)) => value.data_points.len(),
        Some(Data::ExponentialHistogram(value)) => value.data_points.len(),
        Some(Data::Summary(value)) => value.data_points.len(),
        None => 0,
    }
}

fn is_protobuf_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/x-protobuf"))
}

fn metric_success_response(rejected: usize, messages: &[&str]) -> axum::response::Response {
    let partial_success = (rejected > 0).then(|| ExportMetricsPartialSuccess {
        rejected_data_points: i64::try_from(rejected).unwrap_or(i64::MAX),
        error_message: messages.join("; "),
    });
    let response = ExportMetricsServiceResponse { partial_success };
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
