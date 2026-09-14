//! OTLP/HTTP metric request handling and Agent Observatory persistence.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::IntoResponse,
};
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use parking_lot::Mutex;
use prost::Message;

use crate::{
    config::AgentObservatoryPrivacyConfig,
    db::{DbPool, StorageBudgetState},
};

use super::{
    OtlpState,
    auth::{is_authorized, unauthorized},
    error::OtlpError,
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
        state
            .counters
            .metrics_auth_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return unauthorized();
    }
    if !is_protobuf_content_type(&headers) {
        return OtlpError::UnsupportedContentType.into_response();
    }
    let Some(ingest) = state.metric_ingest.clone() else {
        return OtlpError::MetricIngestUnavailable.into_response();
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
            state
                .counters
                .metrics_decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(error = %error, source_ip = %peer, "OTLP /v1/metrics decode failed");
            return OtlpError::DecodeFailed.into_response();
        }
        Err(error) => {
            state
                .counters
                .decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            state
                .counters
                .metrics_decode_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(error = %error, "OTLP metric decode task panicked");
            return OtlpError::Internal.into_response();
        }
    };

    let received_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut normalized = Vec::new();
    let mut rejected = 0usize;
    let mut seen = 0usize;
    let mut normalized_bytes = 0usize;
    let mut invalid = false;
    let mut over_count_cap = false;
    let mut over_byte_cap = false;
    for resource in &req.resource_metrics {
        for scope in &resource.scope_metrics {
            for metric in &scope.metrics {
                let point_count = metric_point_count(metric);
                if metric.data.is_none() {
                    rejected = rejected.saturating_add(1);
                    invalid = true;
                    continue;
                }
                let remaining = MAX_METRIC_POINTS_PER_REQUEST.saturating_sub(seen);
                seen = seen.saturating_add(point_count);
                let count_admitted = point_count.min(remaining);
                if point_count > count_admitted {
                    rejected = rejected.saturating_add(point_count - count_admitted);
                    over_count_cap = true;
                }
                let admitted = metric_normalization_budget(
                    resource.resource.as_ref(),
                    &resource.schema_url,
                    scope.scope.as_ref(),
                    &scope.schema_url,
                    metric,
                    normalized_bytes,
                    count_admitted,
                );
                if count_admitted > admitted {
                    rejected = rejected.saturating_add(count_admitted - admitted);
                    over_byte_cap = true;
                }
                if admitted == 0 {
                    continue;
                }
                let bounded_metric = bounded_metric(metric, admitted);
                match normalize_metric_with_privacy(
                    resource.resource.as_ref(),
                    &resource.schema_url,
                    scope.scope.as_ref(),
                    &scope.schema_url,
                    &bounded_metric,
                    &ingest.privacy,
                    &received_at,
                ) {
                    Ok(points) => {
                        for point in points {
                            let point_bytes = metric_point_bytes(&point);
                            if normalized_bytes.saturating_add(point_bytes)
                                > MAX_NORMALIZED_METRIC_BYTES
                            {
                                rejected += 1;
                                over_byte_cap = true;
                            } else {
                                normalized_bytes += point_bytes;
                                normalized.push(point.into());
                            }
                        }
                    }
                    Err(error) => {
                        rejected += admitted.max(1);
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
    if over_count_cap {
        messages.push("request exceeded 5000 metric point limit");
    }
    if over_byte_cap {
        messages.push("request exceeded normalized metric byte limit");
    }
    if ingest
        .storage_state
        .lock()
        .as_ref()
        .is_some_and(|state| state.write_blocked)
    {
        let blocked = normalized.len();
        state.counters.metrics_backpressure.fetch_add(
            u64::try_from(blocked).unwrap_or(u64::MAX),
            std::sync::atomic::Ordering::Relaxed,
        );
        tracing::warn!(source_ip = %peer, blocked, "OTLP metric persistence blocked by storage budget");
        return OtlpError::MetricStorageBlocked.into_response();
    }
    let pool = Arc::clone(&ingest.pool);
    let persisted = tokio::task::spawn_blocking(move || {
        crate::db::otlp_metrics::insert_otel_metric_points_batch(&pool, &normalized)
    })
    .await;
    let result = match persisted {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            state
                .counters
                .metrics_persistence_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // A pool-acquisition timeout means the points never reached SQLite,
            // so this is backpressure rather than a server fault. OTLP/HTTP
            // only retries 429/502/503/504 — returning 500 makes a conforming
            // exporter drop the batch permanently. Same 503 + Retry-After shape
            // as the storage-budget branch above, so the exporter retries.
            //
            // Note the trace endpoint reports its storage-budget case
            // differently (200 with `rejected_spans`), so only this file has
            // that symmetry.
            if crate::db::is_pool_acquire_failure(&error) {
                tracing::warn!(
                    error = %error,
                    source_ip = %peer,
                    "OTLP metric persistence unavailable; asking exporter to retry"
                );
                return OtlpError::MetricStorageUnavailable.into_response();
            }
            tracing::error!(error = %error, source_ip = %peer, "OTLP metric persistence failed");
            return OtlpError::MetricPersistenceFailed.into_response();
        }
        Err(error) => {
            state
                .counters
                .metrics_persistence_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(error = %error, "OTLP metric persistence task panicked");
            return OtlpError::Internal.into_response();
        }
    };
    rejected += result.rejected;
    if result.rejected > 0 {
        messages.push("metric points rejected by storage validation");
    }
    state.counters.metrics_accepted.fetch_add(
        u64::try_from(result.accepted).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    state.counters.metrics_duplicates.fetch_add(
        u64::try_from(result.duplicates).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    state.counters.metrics_rejected.fetch_add(
        u64::try_from(rejected).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    tracing::info!(source_ip = %peer, accepted = result.accepted, duplicates = result.duplicates, rejected, "OTLP metrics ingested");
    metric_success_response(rejected, &messages)
}

const MAX_NORMALIZED_METRIC_BYTES: usize = 16 * 1024 * 1024;

fn metric_normalization_budget(
    resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>,
    resource_schema_url: &str,
    scope: Option<&opentelemetry_proto::tonic::common::v1::InstrumentationScope>,
    scope_schema_url: &str,
    metric: &opentelemetry_proto::tonic::metrics::v1::Metric,
    normalized_bytes: usize,
    count_budget: usize,
) -> usize {
    let remaining_bytes = MAX_NORMALIZED_METRIC_BYTES.saturating_sub(normalized_bytes);
    // Resource, scope, schema, and metric envelope values are copied into each
    // normalized point. Account for those shared values on every point, then
    // add the encoded size of that specific point. JSON escaping can expand an
    // arbitrary input byte to six bytes (for example, `\u0001`), so use that
    // worst-case multiplier plus fixed storage fields before normalization.
    let shared_wire_bytes = resource
        .map_or(0, Message::encoded_len)
        .saturating_add(resource_schema_url.len())
        .saturating_add(scope.map_or(0, Message::encoded_len))
        .saturating_add(scope_schema_url.len())
        .saturating_add(metric_envelope_encoded_len(metric));

    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    match metric.data.as_ref() {
        Some(Data::Gauge(value)) => admitted_points(
            &value.data_points,
            shared_wire_bytes,
            remaining_bytes,
            count_budget,
        ),
        Some(Data::Sum(value)) => admitted_points(
            &value.data_points,
            shared_wire_bytes,
            remaining_bytes,
            count_budget,
        ),
        Some(Data::Histogram(value)) => admitted_points(
            &value.data_points,
            shared_wire_bytes,
            remaining_bytes,
            count_budget,
        ),
        Some(Data::ExponentialHistogram(value)) => admitted_points(
            &value.data_points,
            shared_wire_bytes,
            remaining_bytes,
            count_budget,
        ),
        Some(Data::Summary(value)) => admitted_points(
            &value.data_points,
            shared_wire_bytes,
            remaining_bytes,
            count_budget,
        ),
        None => 0,
    }
}

fn metric_envelope_encoded_len(metric: &opentelemetry_proto::tonic::metrics::v1::Metric) -> usize {
    metric.metadata.iter().fold(
        bounded_metric(metric, 0).encoded_len(),
        |encoded_len, metadata| {
            encoded_len.saturating_add(length_delimited_field_len(metadata.encoded_len()))
        },
    )
}

fn length_delimited_field_len(payload_len: usize) -> usize {
    // Metric metadata uses protobuf field 12, whose encoded key is one byte.
    1usize
        .saturating_add(encoded_varint_len(payload_len))
        .saturating_add(payload_len)
}

fn encoded_varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn admitted_points<T: Message>(
    points: &[T],
    shared_wire_bytes: usize,
    byte_budget: usize,
    count_budget: usize,
) -> usize {
    let mut admitted = 0usize;
    let mut estimated_bytes = 0usize;
    for point in points.iter().take(count_budget) {
        let point_bytes = shared_wire_bytes
            .saturating_add(point.encoded_len())
            .saturating_mul(6)
            .saturating_add(1024);
        if estimated_bytes.saturating_add(point_bytes) > byte_budget {
            break;
        }
        estimated_bytes = estimated_bytes.saturating_add(point_bytes);
        admitted += 1;
    }
    admitted
}

fn metric_point_bytes(point: &crate::otlp::metrics::MetricPointInput) -> usize {
    point.point_key.len()
        + point.metric_name.len()
        + point.description.len()
        + point.unit.len()
        + point.instrument_kind.len()
        + point.hostname.len()
        + point.resource_json.len()
        + point.attributes_json.len()
        + point.value_json.len()
        + point.exemplars_json.len()
        + point.received_at.len()
        + point.service_name.as_ref().map_or(0, String::len)
        + point.service_version.as_ref().map_or(0, String::len)
        + point.scope_name.as_ref().map_or(0, String::len)
        + point.scope_version.as_ref().map_or(0, String::len)
        + point.ai_tool.as_ref().map_or(0, String::len)
        + point.ai_project.as_ref().map_or(0, String::len)
        + point.ai_session_id.as_ref().map_or(0, String::len)
}

fn bounded_metric(
    metric: &opentelemetry_proto::tonic::metrics::v1::Metric,
    limit: usize,
) -> opentelemetry_proto::tonic::metrics::v1::Metric {
    use opentelemetry_proto::tonic::metrics::v1::{
        ExponentialHistogram, Gauge, Histogram, Metric, Sum, Summary, metric::Data,
    };

    let data = match metric.data.as_ref() {
        Some(Data::Gauge(value)) => Some(Data::Gauge(Gauge {
            data_points: value.data_points.iter().take(limit).cloned().collect(),
        })),
        Some(Data::Sum(value)) => Some(Data::Sum(Sum {
            data_points: value.data_points.iter().take(limit).cloned().collect(),
            aggregation_temporality: value.aggregation_temporality,
            is_monotonic: value.is_monotonic,
        })),
        Some(Data::Histogram(value)) => Some(Data::Histogram(Histogram {
            data_points: value.data_points.iter().take(limit).cloned().collect(),
            aggregation_temporality: value.aggregation_temporality,
        })),
        Some(Data::ExponentialHistogram(value)) => {
            Some(Data::ExponentialHistogram(ExponentialHistogram {
                data_points: value.data_points.iter().take(limit).cloned().collect(),
                aggregation_temporality: value.aggregation_temporality,
            }))
        }
        Some(Data::Summary(value)) => Some(Data::Summary(Summary {
            data_points: value.data_points.iter().take(limit).cloned().collect(),
        })),
        None => None,
    };
    Metric {
        name: metric.name.clone(),
        description: metric.description.clone(),
        unit: metric.unit.clone(),
        // Metric metadata is not persisted by the normalizer. It is included
        // in the admission estimate above, but need not be cloned into the
        // bounded normalization input.
        metadata: Vec::new(),
        data,
    }
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
