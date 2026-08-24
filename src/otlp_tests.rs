//! Handler-level tests for the OTLP HTTP receiver.

use super::*;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::body::{Body, to_bytes};
use axum::http::{
    Request,
    header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER},
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
    number_data_point::Value as NumberValue,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use parking_lot::Mutex;
use tower::util::ServiceExt;

use crate::config::StorageConfig;
use crate::db::{DbPool, StorageBudgetState, get_storage_metrics, init_pool};

struct TestOtlpState {
    _dir: tempfile::TempDir,
    state: OtlpState,
    pool: Arc<DbPool>,
    storage: StorageConfig,
    storage_state: Arc<Mutex<Option<StorageBudgetState>>>,
}

fn state_with_token(token: Option<&str>) -> TestOtlpState {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("otlp.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    let storage_state = Arc::new(Mutex::new(None));
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(10);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    let auth_policy = if token.is_some() {
        crate::mcp::AuthPolicy::Mounted { auth_state: None }
    } else {
        crate::mcp::AuthPolicy::LoopbackDev
    };
    let state = OtlpState::new(
        ingest,
        token.map(String::from),
        Arc::new(OtlpCounters::default()),
        auth_policy,
    )
    .with_trace_ingest(
        Arc::clone(&pool),
        Arc::clone(&storage_state),
        AgentObservatoryPrivacyConfig::default(),
    );
    TestOtlpState {
        _dir: dir,
        state,
        pool,
        storage,
        storage_state,
    }
}

fn peer() -> ConnectInfo<SocketAddr> {
    ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4318))
}

fn protobuf_headers(token: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-protobuf"),
    );
    if let Some(token) = token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    headers
}

fn span(id: u64) -> Span {
    Span {
        trace_id: vec![0x11; 16],
        span_id: id.to_be_bytes().to_vec(),
        name: format!("span-{id}"),
        start_time_unix_nano: 1_700_000_000_000_000_000,
        end_time_unix_nano: 1_700_000_000_000_001_000,
        ..Default::default()
    }
}

fn trace_request(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: None,
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn metric_request(points: Vec<NumberDataPoint>) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: None,
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: "agent.tokens".to_string(),
                    data: Some(
                        opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
                            data_points: points,
                        }),
                    ),
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn metric_point(time: u64) -> NumberDataPoint {
    NumberDataPoint {
        time_unix_nano: time,
        value: Some(NumberValue::AsInt(42)),
        ..Default::default()
    }
}

async fn call_metrics(
    state: &OtlpState,
    headers: HeaderMap,
    request: ExportMetricsServiceRequest,
) -> axum::response::Response {
    metrics_handler(
        State(state.clone()),
        peer(),
        headers,
        Bytes::from(request.encode_to_vec()),
    )
    .await
}

async fn decode_metric_response(
    response: axum::response::Response,
) -> ExportMetricsServiceResponse {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/x-protobuf"
    );
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    ExportMetricsServiceResponse::decode(body).unwrap()
}

fn metric_rows(pool: &DbPool) -> i64 {
    pool.get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM otel_metric_points", [], |row| {
            row.get(0)
        })
        .unwrap()
}

async fn call_traces(
    state: &OtlpState,
    headers: HeaderMap,
    request: ExportTraceServiceRequest,
) -> axum::response::Response {
    traces_handler(
        State(state.clone()),
        peer(),
        headers,
        Bytes::from(request.encode_to_vec()),
    )
    .await
}

async fn decode_trace_response(response: axum::response::Response) -> ExportTraceServiceResponse {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/x-protobuf"
    );
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    ExportTraceServiceResponse::decode(body).unwrap()
}

fn span_rows(pool: &DbPool) -> i64 {
    pool.get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn metrics_handler_persists_valid_protobuf() {
    let test = state_with_token(None);
    let response = call_metrics(
        &test.state,
        protobuf_headers(None),
        metric_request(vec![metric_point(1_700_000_000_000_000_000)]),
    )
    .await;
    assert!(
        decode_metric_response(response)
            .await
            .partial_success
            .is_none()
    );
    assert_eq!(metric_rows(&test.pool), 1);
}

#[tokio::test]
async fn metrics_handler_requires_configured_bearer() {
    let test = state_with_token(Some("secret"));
    let response = call_metrics(
        &test.state,
        protobuf_headers(None),
        metric_request(vec![metric_point(1_700_000_000_000_000_000)]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(metric_rows(&test.pool), 0);
    assert_eq!(
        test.state
            .counters
            .metrics_auth_failures
            .load(Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn metrics_handler_rejects_unsupported_content_type() {
    let test = state_with_token(None);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let response = call_metrics(&test.state, headers, metric_request(vec![metric_point(1)])).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(metric_rows(&test.pool), 0);
}

#[tokio::test]
async fn metrics_handler_rejects_malformed_protobuf_and_counts_decode_error() {
    let test = state_with_token(None);
    let response = metrics_handler(
        State(test.state.clone()),
        peer(),
        protobuf_headers(None),
        Bytes::from_static(&[0xff]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(metric_rows(&test.pool), 0);
    assert_eq!(
        test.state
            .counters
            .metrics_decode_errors
            .load(Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn metrics_handler_reports_unavailable_ingest_state() {
    let test = state_with_token(None);
    let mut state = test.state.clone();
    state.metric_ingest = None;
    let response = call_metrics(
        &state,
        protobuf_headers(None),
        metric_request(vec![metric_point(1)]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(metric_rows(&test.pool), 0);
}

#[tokio::test]
async fn duplicate_metric_export_is_successful_and_idempotent() {
    let test = state_with_token(None);
    let request = metric_request(vec![metric_point(1_700_000_000_000_000_000)]);
    let first = call_metrics(&test.state, protobuf_headers(None), request.clone()).await;
    assert!(
        decode_metric_response(first)
            .await
            .partial_success
            .is_none()
    );
    let second = call_metrics(&test.state, protobuf_headers(None), request).await;
    assert!(
        decode_metric_response(second)
            .await
            .partial_success
            .is_none()
    );
    assert_eq!(metric_rows(&test.pool), 1);
}

#[tokio::test]
async fn metrics_handler_reports_invalid_point_as_partial_success() {
    let test = state_with_token(None);
    let response = call_metrics(
        &test.state,
        protobuf_headers(None),
        metric_request(vec![metric_point(0)]),
    )
    .await;
    let partial = decode_metric_response(response)
        .await
        .partial_success
        .unwrap();
    assert_eq!(partial.rejected_data_points, 1);
    assert!(
        partial
            .error_message
            .contains("invalid metric points rejected")
    );
    assert_eq!(metric_rows(&test.pool), 0);
}

#[tokio::test]
async fn metric_request_over_cap_reports_excess_as_partial_success() {
    let test = state_with_token(None);
    let points = (1..=(MAX_METRIC_POINTS_PER_REQUEST + 1))
        .map(|offset| metric_point(1_700_000_000_000_000_000 + offset as u64))
        .collect();
    let response = call_metrics(&test.state, protobuf_headers(None), metric_request(points)).await;
    let partial = decode_metric_response(response)
        .await
        .partial_success
        .unwrap();
    assert_eq!(partial.rejected_data_points, 1);
    assert!(partial.error_message.contains("5000 metric point limit"));
    assert_eq!(
        metric_rows(&test.pool),
        MAX_METRIC_POINTS_PER_REQUEST as i64
    );
}

#[tokio::test]
async fn metric_storage_budget_block_is_retryable() {
    let test = state_with_token(None);
    let metrics = get_storage_metrics(&test.pool, &test.storage).unwrap();
    *test.storage_state.lock() = Some(StorageBudgetState {
        metrics,
        write_blocked: true,
    });
    let response = call_metrics(
        &test.state,
        protobuf_headers(None),
        metric_request(vec![metric_point(1_700_000_000_000_000_000)]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "1");
    assert_eq!(
        test.state
            .counters
            .metrics_backpressure
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(metric_rows(&test.pool), 0);
}

#[tokio::test]
async fn traces_handler_requires_bearer_when_token_configured() {
    let test = state_with_token(Some("secret"));
    let response = call_traces(&test.state, HeaderMap::new(), trace_request(vec![span(1)])).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(span_rows(&test.pool), 0);
}

#[tokio::test]
async fn traces_handler_rejects_invalid_bearer() {
    let test = state_with_token(Some("secret"));
    let response = call_traces(
        &test.state,
        protobuf_headers(Some("wrong")),
        trace_request(vec![span(1)]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(span_rows(&test.pool), 0);
}

#[tokio::test]
async fn traces_handler_rejects_unsupported_content_type() {
    let test = state_with_token(None);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let response = call_traces(&test.state, headers, trace_request(vec![span(1)])).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(span_rows(&test.pool), 0);
}

#[tokio::test]
async fn traces_handler_rejects_malformed_protobuf() {
    let test = state_with_token(None);
    let response = traces_handler(
        State(test.state.clone()),
        peer(),
        protobuf_headers(None),
        Bytes::from_static(&[0xff]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(span_rows(&test.pool), 0);
    assert_eq!(test.state.counters.decode_errors.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn traces_handler_persists_valid_protobuf_and_returns_otlp_response() {
    let test = state_with_token(Some("secret"));
    let response = call_traces(
        &test.state,
        protobuf_headers(Some("secret")),
        trace_request(vec![span(1)]),
    )
    .await;
    let decoded = decode_trace_response(response).await;
    assert!(decoded.partial_success.is_none());
    assert_eq!(span_rows(&test.pool), 1);
}

#[tokio::test]
async fn traces_handler_reports_invalid_span_as_partial_success() {
    let test = state_with_token(None);
    let mut invalid = span(2);
    invalid.trace_id = vec![0; 16];
    let response = call_traces(
        &test.state,
        protobuf_headers(None),
        trace_request(vec![span(1), invalid]),
    )
    .await;
    let decoded = decode_trace_response(response).await;
    let partial = decoded.partial_success.unwrap();
    assert_eq!(partial.rejected_spans, 1);
    assert!(partial.error_message.contains("invalid spans rejected"));
    assert_eq!(span_rows(&test.pool), 1);
}

#[tokio::test]
async fn duplicate_trace_export_is_successful_and_idempotent() {
    let test = state_with_token(None);
    let request = trace_request(vec![span(1)]);
    let first = call_traces(&test.state, protobuf_headers(None), request.clone()).await;
    assert!(decode_trace_response(first).await.partial_success.is_none());
    let second = call_traces(&test.state, protobuf_headers(None), request).await;
    assert!(
        decode_trace_response(second)
            .await
            .partial_success
            .is_none()
    );
    assert_eq!(span_rows(&test.pool), 1);
}

#[tokio::test]
async fn trace_span_cap_rejects_only_excess_spans() {
    let test = state_with_token(None);
    let spans = (1..=(MAX_SPANS_PER_REQUEST as u64 + 1)).map(span).collect();
    let response = call_traces(&test.state, protobuf_headers(None), trace_request(spans)).await;
    let decoded = decode_trace_response(response).await;
    let partial = decoded.partial_success.unwrap();
    assert_eq!(partial.rejected_spans, 1);
    assert!(partial.error_message.contains("5000 span limit"));
    assert_eq!(span_rows(&test.pool), MAX_SPANS_PER_REQUEST as i64);
}

#[tokio::test]
async fn storage_budget_block_is_otlp_partial_success_not_server_failure() {
    let test = state_with_token(None);
    let metrics = get_storage_metrics(&test.pool, &test.storage).unwrap();
    *test.storage_state.lock() = Some(StorageBudgetState {
        metrics,
        write_blocked: true,
    });
    let response = call_traces(
        &test.state,
        protobuf_headers(None),
        trace_request(vec![span(1)]),
    )
    .await;
    let decoded = decode_trace_response(response).await;
    let partial = decoded.partial_success.unwrap();
    assert_eq!(partial.rejected_spans, 1);
    assert!(partial.error_message.contains("storage budget"));
    assert_eq!(span_rows(&test.pool), 0);
}

#[tokio::test]
async fn traces_router_enforces_eight_mib_body_limit_with_retry_after() {
    let test = state_with_token(None);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/traces")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .extension(peer())
        .body(Body::from(vec![0_u8; OTLP_SIGNAL_BODY_LIMIT_BYTES + 1]))
        .unwrap();
    let response = router(test.state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "86400");
}

#[tokio::test]
async fn metrics_router_enforces_eight_mib_body_limit_with_retry_after() {
    let test = state_with_token(None);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/metrics")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .extension(peer())
        .body(Body::from(vec![0_u8; OTLP_SIGNAL_BODY_LIMIT_BYTES + 1]))
        .unwrap();
    let response = router(test.state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "86400");
}

#[tokio::test]
async fn logs_router_preserves_four_mib_body_limit_with_retry_after() {
    let test = state_with_token(None);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/logs")
        .extension(peer())
        .body(Body::from(vec![0_u8; OTLP_BODY_LIMIT_BYTES + 1]))
        .unwrap();
    let response = router(test.state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "86400");
}

#[test]
fn counters_default_to_zero() {
    let counters = OtlpCounters::default();
    assert_eq!(counters.logs_received.load(Ordering::Relaxed), 0);
    assert_eq!(counters.decode_errors.load(Ordering::Relaxed), 0);
    assert_eq!(counters.metrics_accepted.load(Ordering::Relaxed), 0);
    assert_eq!(counters.metrics_duplicates.load(Ordering::Relaxed), 0);
    assert_eq!(counters.metrics_rejected.load(Ordering::Relaxed), 0);
    assert_eq!(counters.metrics_backpressure.load(Ordering::Relaxed), 0);
}

/// A pool-acquisition timeout must read as retryable backpressure, not a server
/// fault. OTLP/HTTP only retries 429/502/503/504, so answering 500 makes a
/// conforming exporter drop the batch permanently.
#[tokio::test]
async fn metric_pool_exhaustion_returns_retryable_503_not_500() {
    let test = state_with_token(None);
    // `StorageConfig::for_test` builds a single-connection pool, so holding
    // that connection reproduces production pool exhaustion exactly.
    let hog = test.pool.get().unwrap();

    let response = call_metrics(
        &test.state,
        protobuf_headers(None),
        metric_request(vec![metric_point(1_700_000_000_000_000_000)]),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "pool exhaustion must be retryable, not a 500"
    );
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "1");
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "metric_storage_unavailable");
    assert_eq!(json["retryable"], true);

    drop(hog);
}

/// Same contract for the trace endpoint.
#[tokio::test]
async fn trace_pool_exhaustion_returns_retryable_503_not_500() {
    let test = state_with_token(None);
    let hog = test.pool.get().unwrap();

    let response = call_traces(
        &test.state,
        protobuf_headers(None),
        trace_request(vec![span(1_700_000_000_000_000_000)]),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "pool exhaustion must be retryable, not a 500"
    );
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "1");
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "trace_storage_unavailable");
    assert_eq!(json["retryable"], true);

    drop(hog);
}
