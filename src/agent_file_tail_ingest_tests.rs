use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use super::*;
use crate::config::StorageConfig;

fn app() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap(),
    );
    let state = AgentFileTailIngestState::new(
        pool,
        Some("secret".into()),
        Default::default(),
        AuthPolicy::Mounted { auth_state: None },
        Arc::new(parking_lot::Mutex::new(None)),
        Default::default(),
    );
    (
        router(state).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 41000)))),
        dir,
    )
}

fn record() -> serde_json::Value {
    serde_json::json!({
        "idempotency_key": "test-record-1",
        "hostname": "devhost",
        "source_id": "app-log",
        "tag": "my-app",
        "path_basename": "app.log",
        "timestamp": "2026-09-02T12:00:00Z",
        "message": "request complete"
    })
}

async fn post(app: Router, value: serde_json::Value, authorized: bool) -> axum::response::Response {
    post_with_token(app, value, authorized.then_some("secret")).await
}

async fn post_with_token(
    app: Router,
    value: serde_json::Value,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/file-tails")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap()
}

async fn accepted(response: axum::response::Response) -> usize {
    let body = to_bytes(response.into_body(), BODY_LIMIT_BYTES)
        .await
        .unwrap();
    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["accepted"]
        .as_u64()
        .unwrap() as usize
}

#[tokio::test]
async fn requires_bearer_authentication() {
    let (app, _dir) = app();
    assert_eq!(
        post(app, serde_json::json!({"records": []}), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn accepts_a_bounded_device_tail_record() {
    let (app, _dir) = app();
    let response = post(app, serde_json::json!({"records": [record()]}), true).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn accepts_a_named_forwarding_agent_token() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap(),
    );
    let state = AgentFileTailIngestState::new(
        pool,
        Some("shared-secret".into()),
        HashMap::from([("named-secret".into(), "devhost-agent".into())]),
        AuthPolicy::Mounted { auth_state: None },
        Arc::new(parking_lot::Mutex::new(None)),
        Default::default(),
    );
    let app = router(state).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 41000))));

    let response = post_with_token(
        app,
        serde_json::json!({"records": [record()]}),
        Some("named-secret"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn shared_token_rotation_preserves_replay_identity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap(),
    );
    let make_app = |token: &str| {
        let state = AgentFileTailIngestState::new(
            Arc::clone(&pool),
            Some(token.into()),
            Default::default(),
            AuthPolicy::Mounted { auth_state: None },
            Arc::new(parking_lot::Mutex::new(None)),
            Default::default(),
        );
        router(state).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 41000))))
    };

    let first = post_with_token(
        make_app("old-secret"),
        serde_json::json!({"records": [record()]}),
        Some("old-secret"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(accepted(first).await, 1);

    let replay = post_with_token(
        make_app("new-secret"),
        serde_json::json!({"records": [record()]}),
        Some("new-secret"),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(accepted(replay).await, 0);
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn accepts_legacy_records_without_idempotency_keys() {
    let (app, _dir) = app();
    let mut legacy = record();
    legacy.as_object_mut().unwrap().remove("idempotency_key");
    let response = post(app, serde_json::json!({"records": [legacy]}), true).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejects_a_changed_payload_reusing_the_same_key() {
    let (app, _dir) = app();
    assert_eq!(
        post(
            app.clone(),
            serde_json::json!({"records": [record()]}),
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut changed = record();
    changed["message"] = serde_json::json!("different payload");
    assert_eq!(
        post(app, serde_json::json!({"records": [changed]}), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn rejects_forgeable_or_ambiguous_identity() {
    let (app, _dir) = app();
    let mut bad = record();
    bad["hostname"] = serde_json::json!("devhost/other");
    assert_eq!(
        post(app, serde_json::json!({"records": [bad]}), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn rejects_unknown_fields() {
    let (app, _dir) = app();
    let mut bad = record();
    bad["credential"] = serde_json::json!("must-not-be-accepted");
    assert_eq!(
        post(app, serde_json::json!({"records": [bad]}), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn rejects_batches_over_the_record_limit() {
    let (app, _dir) = app();
    let records = std::iter::repeat_with(record)
        .take(MAX_RECORDS_PER_BATCH + 1)
        .collect::<Vec<_>>();
    assert_eq!(
        post(app, serde_json::json!({"records": records}), true)
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[test]
fn maps_to_distinct_agent_file_tail_envelope() {
    let parsed: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let entry = to_log_batch_entry(parsed).unwrap();
    assert_eq!(entry.source_ip, "agent-file-tail://devhost/app-log");
    assert_eq!(entry.hostname, "devhost");
    assert_eq!(entry.app_name.as_deref(), Some("my-app"));
    let metadata: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(metadata["source_kind"], "agent-file-tail");
    assert_eq!(metadata["path_basename"], "app.log");
}

#[test]
fn persistence_is_idempotent_and_obeys_storage_admission() {
    let dir = tempfile::tempdir().unwrap();
    let storage_config = StorageConfig::for_test(dir.path().join("tails.db"));
    let pool = crate::db::init_pool(&storage_config).unwrap();
    let parsed: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let entry = to_log_batch_entry(parsed.clone()).unwrap();
    let storage = parking_lot::Mutex::new(Some(crate::db::StorageBudgetState {
        metrics: crate::db::get_storage_metrics(&pool, &storage_config).unwrap(),
        write_blocked: true,
    }));
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            std::slice::from_ref(&entry),
            std::slice::from_ref(&parsed),
            "bearer:test"
        )
        .unwrap_err()
        .to_string(),
        "storage_write_blocked"
    );
    storage.lock().as_mut().unwrap().write_blocked = false;
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            std::slice::from_ref(&entry),
            std::slice::from_ref(&parsed),
            "bearer:test"
        )
        .unwrap(),
        1
    );
    assert_eq!(
        persist_idempotent(&pool, &storage, &[entry], &[parsed], "bearer:test").unwrap(),
        0
    );
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn mismatched_replay_is_an_idempotency_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap();
    let storage = parking_lot::Mutex::new(None);
    let first: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let first_entry = to_log_batch_entry(first.clone()).unwrap();
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[first_entry],
            std::slice::from_ref(&first),
            "bearer:test",
        )
        .unwrap(),
        1
    );

    let mut changed = first;
    changed.message = "different payload".into();
    let changed_entry = to_log_batch_entry(changed.clone()).unwrap();
    assert!(
        persist_idempotent(&pool, &storage, &[changed_entry], &[changed], "bearer:test",)
            .unwrap_err()
            .downcast_ref::<IdempotencyConflict>()
            .is_some()
    );
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn same_caller_key_is_scoped_by_authenticated_source() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap();
    let storage = parking_lot::Mutex::new(None);
    let first: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let mut second = first.clone();
    second.hostname = "host-b".into();
    let first_entry = to_log_batch_entry(first.clone()).unwrap();
    let second_entry = to_log_batch_entry(second.clone()).unwrap();

    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[first_entry],
            &[first],
            "bearer:principal-a",
        )
        .unwrap(),
        1
    );
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[second_entry],
            &[second],
            "bearer:principal-a",
        )
        .unwrap(),
        1
    );
    let third: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let third_entry = to_log_batch_entry(third.clone()).unwrap();
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[third_entry],
            &[third],
            "bearer:principal-b",
        )
        .unwrap(),
        1
    );
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn records_without_idempotency_keys_remain_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap();
    let storage = parking_lot::Mutex::new(None);
    let mut legacy: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    legacy.idempotency_key = None;
    let entry = to_log_batch_entry(legacy.clone()).unwrap();

    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[entry.clone(), entry],
            &[legacy.clone(), legacy],
            "bearer:test",
        )
        .unwrap(),
        2
    );
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM syslog_forward_receipts", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn mixed_duplicate_and_new_records_use_one_batched_host_update() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&StorageConfig::for_test(dir.path().join("tails.db"))).unwrap();
    let storage = parking_lot::Mutex::new(None);
    let first: AgentFileTailRecord = serde_json::from_value(record()).unwrap();
    let first_entry = to_log_batch_entry(first.clone()).unwrap();
    assert_eq!(
        persist_idempotent(
            &pool,
            &storage,
            &[first_entry],
            std::slice::from_ref(&first),
            "shared_bearer",
        )
        .unwrap(),
        1
    );

    {
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE host_update_audit (hostname TEXT NOT NULL);
             CREATE TRIGGER audit_host_update AFTER UPDATE ON hosts
             BEGIN
               INSERT INTO host_update_audit(hostname) VALUES (NEW.hostname);
             END;",
        )
        .unwrap();
    }

    let duplicate = first.clone();
    let mut keyed_new = first.clone();
    keyed_new.idempotency_key = Some("test-record-2".into());
    keyed_new.message = "second record".into();
    let mut keyless_new = first.clone();
    keyless_new.idempotency_key = None;
    keyless_new.message = "legacy record".into();
    let records = vec![duplicate, keyed_new, keyless_new];
    let entries = records
        .iter()
        .cloned()
        .map(to_log_batch_entry)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        persist_idempotent(&pool, &storage, &entries, &records, "shared_bearer").unwrap(),
        2
    );
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT log_count FROM hosts WHERE hostname = 'devhost'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM host_update_audit", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}
