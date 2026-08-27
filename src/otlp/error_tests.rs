use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::IntoResponse;

use super::OtlpError;

/// Every variant, so the invariants below are exhaustive rather than
/// spot-checks. A new variant fails to compile until it is listed here.
const ALL: &[OtlpError] = &[
    OtlpError::Unauthorized,
    OtlpError::DecodeFailed,
    OtlpError::UnsupportedContentType,
    OtlpError::Internal,
    OtlpError::WriterUnavailable,
    OtlpError::ChannelFull,
    OtlpError::MetricIngestUnavailable,
    OtlpError::MetricStorageBlocked,
    OtlpError::MetricStorageUnavailable,
    OtlpError::MetricPersistenceFailed,
    OtlpError::TraceIngestUnavailable,
    OtlpError::TraceStorageUnavailable,
    OtlpError::TracePersistenceFailed,
];

#[test]
fn all_lists_every_variant() {
    // Destructuring in an exhaustive match makes the compiler reject a new
    // variant that was added without extending ALL.
    for error in ALL {
        match error {
            OtlpError::Unauthorized
            | OtlpError::DecodeFailed
            | OtlpError::UnsupportedContentType
            | OtlpError::Internal
            | OtlpError::WriterUnavailable
            | OtlpError::ChannelFull
            | OtlpError::MetricIngestUnavailable
            | OtlpError::MetricStorageBlocked
            | OtlpError::MetricStorageUnavailable
            | OtlpError::MetricPersistenceFailed
            | OtlpError::TraceIngestUnavailable
            | OtlpError::TraceStorageUnavailable
            | OtlpError::TracePersistenceFailed => {}
        }
    }
    let mut codes: Vec<&str> = ALL.iter().map(|error| error.code()).collect();
    codes.sort_unstable();
    let unique = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), unique, "error codes must be distinct");
}

/// The finding this enum exists to fix: `retryable` used to appear on three of
/// the six 503s, so clients could not branch on it. It must be on all of them.
#[tokio::test]
async fn every_response_carries_retryable() {
    for error in ALL {
        let response = error.into_response();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], error.code(), "{:?}", error);
        assert_eq!(
            json["retryable"],
            serde_json::Value::Bool(error.retryable()),
            "{error:?} must report `retryable`"
        );
    }
}

/// `Retry-After` and `retryable` are one decision, not two that can drift.
#[test]
fn retry_after_is_present_exactly_when_retryable() {
    for error in ALL {
        assert_eq!(
            error.retry_after().is_some(),
            error.retryable(),
            "{error:?} disagrees between retry_after and retryable"
        );
    }
}

#[tokio::test]
async fn retryable_errors_send_the_retry_after_header() {
    for error in ALL {
        let response = error.into_response();
        let header = response.headers().get(RETRY_AFTER);
        assert_eq!(
            header.is_some(),
            error.retryable(),
            "{error:?} header presence must match retryable"
        );
        if let Some(value) = header {
            assert_eq!(value, error.retry_after().unwrap(), "{error:?}");
        }
    }
}

/// Only the two "signal not configured on this process" cases are permanent.
/// Everything else that answers 503 is transient contention.
#[test]
fn only_unconfigured_signals_are_non_retryable_503s() {
    let non_retryable_503: Vec<&str> = ALL
        .iter()
        .filter(|error| error.status() == StatusCode::SERVICE_UNAVAILABLE && !error.retryable())
        .map(|error| error.code())
        .collect();
    assert_eq!(
        non_retryable_503,
        vec!["metric_ingest_unavailable", "trace_ingest_unavailable"]
    );
}

/// Six 503s is the number `docs/contracts/http-endpoints.md` section 13
/// tabulates; a seventh must land in that table too.
#[test]
fn the_surface_has_exactly_six_service_unavailable_codes() {
    let codes: Vec<&str> = ALL
        .iter()
        .filter(|error| error.status() == StatusCode::SERVICE_UNAVAILABLE)
        .map(|error| error.code())
        .collect();
    assert_eq!(
        codes,
        vec![
            "channel_full",
            "metric_ingest_unavailable",
            "metric_storage_blocked",
            "metric_storage_unavailable",
            "trace_ingest_unavailable",
            "trace_storage_unavailable",
        ]
    );
}

#[tokio::test]
async fn message_detail_rides_alongside_the_stable_code() {
    let response = OtlpError::DecodeFailed.into_response_with_message("bad wire type".to_string());
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "decode_failed");
    assert_eq!(json["retryable"], false);
    assert_eq!(json["message"], "bad wire type");
}
