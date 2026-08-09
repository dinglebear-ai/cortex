use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use super::*;
use crate::config::StorageConfig;
use crate::mcp::AuthPolicy;

fn test_app(token: Option<&str>) -> (Router, tempfile::TempDir) {
    test_app_with(
        token,
        AuthPolicy::Mounted { auth_state: None },
        SocketAddr::from(([10, 0, 0, 7], 41000)),
    )
}

fn test_app_with(
    token: Option<&str>,
    auth_policy: AuthPolicy,
    peer: SocketAddr,
) -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("shell-history-ingest-test.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    let state = ShellHistoryIngestState::new(pool, token.map(str::to_string), auth_policy);
    let app = router(state).layer(MockConnectInfo(peer));
    (app, dir)
}

fn sample_record() -> serde_json::Value {
    serde_json::json!({
        "source": "zsh",
        "hostname": "devhost",
        "timestamp": "2026-07-09T00:00:00Z",
        "duration_ms": 500,
        "command": "cargo test",
        "cwd": null,
        "exit_status": 0,
        "session_id": null,
    })
}

#[tokio::test]
async fn rejects_missing_bearer_token() {
    let (app, _dir) = test_app(Some("secret"));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/shell-history")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"records":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn loopback_dev_allows_unauthenticated_local_peer() {
    let (app, _dir) = test_app_with(
        None,
        AuthPolicy::LoopbackDev,
        SocketAddr::from(([127, 0, 0, 1], 41000)),
    );
    let body = serde_json::to_string(&serde_json::json!({"records": [sample_record()]})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/shell-history")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn accepts_batch_with_valid_bearer_token_and_inserts_rows() {
    let (app, _dir) = test_app(Some("secret"));
    let body = serde_json::to_string(&serde_json::json!({"records": [sample_record()]})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/shell-history")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(value["accepted"], 1);
}

#[tokio::test]
async fn rejects_batch_over_record_limit() {
    let (app, _dir) = test_app(Some("secret"));
    let records: Vec<_> = (0..MAX_RECORDS_PER_BATCH + 1)
        .map(|_| sample_record())
        .collect();
    let body = serde_json::to_string(&serde_json::json!({"records": records})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/shell-history")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn rejects_unknown_fields_in_record() {
    let (app, _dir) = test_app(Some("secret"));
    let mut record = sample_record();
    record
        .as_object_mut()
        .unwrap()
        .insert("bogus".to_string(), serde_json::json!(true));
    let body = serde_json::to_string(&serde_json::json!({"records": [record]})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/shell-history")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
