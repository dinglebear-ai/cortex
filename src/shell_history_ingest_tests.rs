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
    let state = ShellHistoryIngestState::new(
        pool,
        token.map(str::to_string),
        Default::default(),
        auth_policy,
        Arc::new(parking_lot::Mutex::new(None)),
    );
    let app = router(state).layer(MockConnectInfo(peer));
    (app, dir)
}

fn test_app_with_storage(
    token: Option<&str>,
    storage_state: crate::db::StorageBudgetState,
) -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("shell-history-ingest-test.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    let shared_storage = Arc::new(parking_lot::Mutex::new(Some(storage_state)));
    let state = ShellHistoryIngestState::new(
        pool,
        token.map(str::to_string),
        Default::default(),
        AuthPolicy::Mounted { auth_state: None },
        shared_storage,
    );
    let app = router(state).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 41000))));
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

fn sample_record_with_key(key: &str) -> serde_json::Value {
    let mut record = sample_record();
    record.as_object_mut().unwrap().insert(
        "idempotency_key".to_string(),
        serde_json::Value::String(key.to_string()),
    );
    record
}

async fn post_records(app: Router, records: Vec<serde_json::Value>) -> Response {
    post_records_with_token(app, records, "secret").await
}

async fn post_records_with_token(
    app: Router,
    records: Vec<serde_json::Value>,
    token: &str,
) -> Response {
    let body = serde_json::to_string(&serde_json::json!({"records": records})).unwrap();
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/v1/shell-history")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn named_forwarding_principals_are_accepted_and_isolate_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("named-shell-history.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    let state = ShellHistoryIngestState::new(
        Arc::clone(&pool),
        None,
        HashMap::from([
            ("agent-a-token".into(), "agent-a".into()),
            ("agent-b-token".into(), "agent-b".into()),
        ]),
        AuthPolicy::Mounted { auth_state: None },
        Arc::new(parking_lot::Mutex::new(None)),
    );
    let app = router(state).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 41000))));
    let record = sample_record_with_key("same-local-key");

    let first = post_records_with_token(app.clone(), vec![record.clone()], "agent-a-token").await;
    let second = post_records_with_token(app, vec![record], "agent-b-token").await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

async fn response_json(response: Response) -> serde_json::Value {
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body_bytes).unwrap()
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
async fn rejects_malformed_idempotency_keys_before_storage_work() {
    for key in [String::new(), "x".repeat(257)] {
        let dir = tempfile::tempdir().unwrap();
        let storage_config = StorageConfig::for_test(dir.path().join("metrics.db"));
        let metrics_pool = crate::db::init_pool(&storage_config).unwrap();
        let storage_state = crate::db::StorageBudgetState {
            metrics: crate::db::get_storage_metrics(&metrics_pool, &storage_config).unwrap(),
            write_blocked: true,
        };
        let (app, _app_dir) = test_app_with_storage(Some("secret"), storage_state);

        let response = post_records(app, vec![sample_record_with_key(&key)]).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value = response_json(response).await;
        assert_eq!(value["error"], "invalid_payload");
    }
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

#[tokio::test]
async fn storage_block_returns_retryable_service_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let storage_config = StorageConfig::for_test(dir.path().join("metrics.db"));
    let metrics_pool = crate::db::init_pool(&storage_config).unwrap();
    let storage_state = crate::db::StorageBudgetState {
        metrics: crate::db::get_storage_metrics(&metrics_pool, &storage_config).unwrap(),
        write_blocked: true,
    };
    let (app, _app_dir) = test_app_with_storage(Some("secret"), storage_state);
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

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "5");
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(value["error"], "storage_write_blocked");
}

#[tokio::test]
async fn mixed_duplicate_and_new_records_update_host_counts_once() {
    let (app, dir) = test_app(Some("secret"));
    let existing = sample_record_with_key("existing");
    let response = post_records(app.clone(), vec![existing.clone()]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["accepted"], 1);

    let mut new_idempotent = sample_record_with_key("new");
    new_idempotent["command"] = serde_json::json!("cargo clippy");
    let mut keyless = sample_record();
    keyless["hostname"] = serde_json::json!("edgehost");
    keyless["command"] = serde_json::json!("cargo fmt --check");

    let response = post_records(app, vec![existing, new_idempotent, keyless]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["accepted"], 2);

    let storage = StorageConfig::for_test(dir.path().join("shell-history-ingest-test.db"));
    let pool = crate::db::init_pool(&storage).unwrap();
    let hosts = crate::db::list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 2);
    assert_eq!(
        hosts
            .iter()
            .find(|host| host.hostname == "devhost")
            .unwrap()
            .log_count,
        2
    );
    assert_eq!(
        hosts
            .iter()
            .find(|host| host.hostname == "edgehost")
            .unwrap()
            .log_count,
        1
    );
}

#[tokio::test]
async fn duplicate_keys_with_different_payloads_in_one_batch_conflict_atomically() {
    let (app, dir) = test_app(Some("secret"));
    let first = sample_record_with_key("same-key");
    let mut changed = first.clone();
    changed["command"] = serde_json::json!("different command");

    let response = post_records(app, vec![first, changed]).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error"],
        "idempotency_conflict"
    );

    let storage = StorageConfig::for_test(dir.path().join("shell-history-ingest-test.db"));
    let pool = crate::db::init_pool(&storage).unwrap();
    assert!(
        crate::db::tail_logs(&pool, None, None, None, None, 10)
            .unwrap()
            .is_empty()
    );
}
