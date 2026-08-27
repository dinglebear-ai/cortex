//! Client-visible error codes for the OTLP/HTTP surface.
//!
//! The strings in [`OtlpError::code`] are contract, not log text:
//! `docs/contracts/http-endpoints.md` §13 publishes them and exporters branch
//! on them. Collecting them in one enum is what keeps that table checkable and
//! stops a second, subtly different literal from being introduced at a new
//! return site.
//!
//! The enum also owns the `retryable` flag and the `Retry-After` header, which
//! is the point: those two used to be attached ad hoc at each `503` return
//! site, so `retryable` appeared on three of the six `503`s and clients could
//! not use it as a discriminator at all.

use axum::{
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Json, Response},
};
use serde_json::json;

/// Every error the OTLP handlers report as a JSON `{"error": ...}` body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OtlpError {
    /// Bearer gate rejected the request.
    Unauthorized,
    /// Protobuf body did not decode.
    DecodeFailed,
    /// `Content-Type` was not `application/x-protobuf`.
    UnsupportedContentType,
    /// A `spawn_blocking` task panicked, or another unclassified server fault.
    Internal,
    /// The `/v1/logs` write channel is closed — the batch writer task is dead.
    WriterUnavailable,
    /// The `/v1/logs` write channel cannot fit the batch.
    ChannelFull,
    /// Agent Observatory metric ingest is not configured on this process.
    MetricIngestUnavailable,
    /// Metric persistence is write-blocked by the configured storage budget.
    MetricStorageBlocked,
    /// The pool did not yield a connection for the metric write.
    MetricStorageUnavailable,
    /// Metric persistence failed for a reason that retrying will not fix.
    MetricPersistenceFailed,
    /// Agent Observatory trace ingest is not configured on this process.
    TraceIngestUnavailable,
    /// The pool did not yield a connection for the span write.
    TraceStorageUnavailable,
    /// Span persistence failed for a reason that retrying will not fix.
    TracePersistenceFailed,
}

/// `Retry-After` handed to exporters for the retryable cases.
///
/// One second: every retryable condition here is contention that clears on the
/// order of a single write, and OTLP exporters apply their own backoff on top.
const RETRY_AFTER_SECONDS: &str = "1";

impl OtlpError {
    /// The stable `error` string. Changing one of these is a wire-contract
    /// change; update `docs/contracts/http-endpoints.md` in the same patch.
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::DecodeFailed => "decode_failed",
            Self::UnsupportedContentType => "unsupported_content_type",
            Self::Internal => "internal",
            Self::WriterUnavailable => "writer_unavailable",
            Self::ChannelFull => "channel_full",
            Self::MetricIngestUnavailable => "metric_ingest_unavailable",
            Self::MetricStorageBlocked => "metric_storage_blocked",
            Self::MetricStorageUnavailable => "metric_storage_unavailable",
            Self::MetricPersistenceFailed => "metric_persistence_failed",
            Self::TraceIngestUnavailable => "trace_ingest_unavailable",
            Self::TraceStorageUnavailable => "trace_storage_unavailable",
            Self::TracePersistenceFailed => "trace_persistence_failed",
        }
    }

    pub(super) const fn status(self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::DecodeFailed => StatusCode::BAD_REQUEST,
            Self::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Internal
            | Self::WriterUnavailable
            | Self::MetricPersistenceFailed
            | Self::TracePersistenceFailed => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ChannelFull
            | Self::MetricIngestUnavailable
            | Self::MetricStorageBlocked
            | Self::MetricStorageUnavailable
            | Self::TraceIngestUnavailable
            | Self::TraceStorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Whether the exporter should send this batch again.
    ///
    /// This — not the status code — is the discriminator clients branch on.
    /// Note two `503`s are **not** retryable: `*_ingest_unavailable` means the
    /// signal is not configured on this process, which is fixed for the
    /// process lifetime. They are still `503` rather than `404` because the
    /// route is mounted and the same payload would be accepted by a process
    /// that has the signal enabled.
    pub(super) const fn retryable(self) -> bool {
        match self {
            Self::ChannelFull
            | Self::MetricStorageBlocked
            | Self::MetricStorageUnavailable
            | Self::TraceStorageUnavailable => true,
            Self::Unauthorized
            | Self::DecodeFailed
            | Self::UnsupportedContentType
            | Self::Internal
            | Self::WriterUnavailable
            | Self::MetricIngestUnavailable
            | Self::MetricPersistenceFailed
            | Self::TraceIngestUnavailable
            | Self::TracePersistenceFailed => false,
        }
    }

    /// `Retry-After` value, present exactly when [`Self::retryable`] is true.
    /// Derived from it rather than tabulated separately so the two cannot drift.
    pub(super) const fn retry_after(self) -> Option<&'static str> {
        if self.retryable() {
            Some(RETRY_AFTER_SECONDS)
        } else {
            None
        }
    }

    /// Render with an extra `message` detail alongside the stable `error` code.
    pub(super) fn into_response_with_message(self, message: String) -> Response {
        self.render(Some(message))
    }

    fn render(self, message: Option<String>) -> Response {
        let mut body = json!({"error": self.code(), "retryable": self.retryable()});
        if let Some(message) = message
            && let Some(object) = body.as_object_mut()
        {
            object.insert("message".to_string(), json!(message));
        }
        let mut response = (self.status(), Json(body)).into_response();
        if let Some(seconds) = self.retry_after() {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static(seconds));
        }
        response
    }
}

impl IntoResponse for OtlpError {
    fn into_response(self) -> Response {
        self.render(None)
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
