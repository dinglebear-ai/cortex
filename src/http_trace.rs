//! Minimal HTTP spans safe to export from the Cortex server.
//!
//! Route groups are static labels. Request paths, queries, headers, and bodies
//! are never captured in exported spans.

use std::time::Duration;

use axum::http::{Request, Response};
use tracing::{Span, field};

pub fn request_span<B>(request: &Request<B>) -> Span {
    if request.uri().path() == "/health" {
        return Span::none();
    }
    tracing::info_span!(
        target: "cortex_http",
        "http.request",
        method = method_group(request.method()),
        surface = route_group(request.uri().path()),
        status = field::Empty,
    )
}

pub fn response_span<B>(response: &Response<B>, _latency: Duration, span: &Span) {
    span.record("status", response.status().as_u16());
}

fn route_group(path: &str) -> &'static str {
    match path {
        "/health" => "health",
        "/mcp" => "mcp",
        "/api" => "api",
        path if path.starts_with("/api/") => "api",
        path if path.starts_with("/v1/") => "ingest",
        "/app" => "app",
        path if path.starts_with("/app/") => "app",
        _ => "other",
    }
}

fn method_group(method: &axum::http::Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    }
}

#[cfg(test)]
#[path = "http_trace_tests.rs"]
mod tests;
