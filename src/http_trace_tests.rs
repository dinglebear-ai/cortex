use super::*;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    routing::{get, post},
};
use tower::ServiceExt;
use tracing::{
    Subscriber,
    span::{Attributes, Record},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Default)]
struct SpanFields(Vec<(String, String)>);

impl tracing::field::Visit for SpanFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }
}

struct Capture(Arc<Mutex<Vec<(String, String)>>>);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &tracing::span::Id, _ctx: Context<'_, S>) {
        if attrs.metadata().target() != "cortex_http" {
            return;
        }
        let mut fields = SpanFields::default();
        attrs.record(&mut fields);
        self.0.lock().unwrap().extend(fields.0);
    }

    fn on_record(&self, _id: &tracing::span::Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut fields = SpanFields::default();
        values.record(&mut fields);
        self.0.lock().unwrap().extend(fields.0);
    }
}

#[test]
fn route_group_never_exposes_dynamic_path_segments() {
    assert_eq!(route_group("/api/sessions/private-id"), "api");
    assert_eq!(route_group("/v1/heartbeats"), "ingest");
    assert_eq!(route_group("/app/sessions/private-id"), "app");
    assert_eq!(route_group("/health"), "health");
}

#[test]
fn custom_http_methods_have_bounded_labels() {
    let method = axum::http::Method::from_bytes(b"SECRET-METHOD-VALUE").unwrap();
    assert_eq!(method_group(&method), "OTHER");
}

#[tokio::test(flavor = "current_thread")]
async fn request_trace_has_only_bounded_fields_and_excludes_otlp_receiver() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Capture(Arc::clone(&captured)));
    let _guard = tracing::subscriber::set_default(subscriber);
    let traced = Router::new()
        .route("/api/private-id", get(|| async { "ok" }))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_response(response_span),
        );
    let app = traced.merge(Router::new().route("/v1/traces", post(|| async { "ok" })));
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/api/private-id?secret=hidden")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let first = captured.lock().unwrap().clone();
    assert!(
        first
            .iter()
            .any(|(key, value)| key == "surface" && value.contains("api"))
    );
    assert!(
        first
            .iter()
            .any(|(key, value)| key == "status" && value == "200")
    );
    assert!(first.iter().all(|(key, value)| {
        matches!(key.as_str(), "method" | "surface" | "status")
            && !value.contains("private-id")
            && !value.contains("secret")
    }));
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/v1/traces")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(*captured.lock().unwrap(), first);
}
