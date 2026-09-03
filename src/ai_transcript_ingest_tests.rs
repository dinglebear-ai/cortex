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
    let storage = StorageConfig::for_test(dir.path().join("ai-transcript-ingest-test.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    let state = AiTranscriptIngestState::new(pool, token.map(str::to_string), auth_policy);
    let app = router(state).layer(MockConnectInfo(peer));
    (app, dir)
}

fn sample_record() -> serde_json::Value {
    serde_json::json!({
        "envelope": {
            "version": EVIDENCE_ENVELOPE_VERSION,
            "source_record_id": format!("sha256:{}", "a".repeat(64)),
            "source": {
                "provider": "claude",
                "adapter_version": "test-adapter-v1",
                "source_identity": format!("sha256:{}", "b".repeat(64)),
                "source_epoch": format!("sha256:{}", "c".repeat(64)),
                "source_revision": format!("sha256:{}", "d".repeat(64)),
                "locator": format!("sha256:{}", "e".repeat(64)),
                "native_session_id": "session:sha256:test",
                "title": "safe title",
            },
            "timestamp": "2026-07-09T00:00:00Z",
            "hostname": "devhost",
            "ai_project": "project:sha256:test",
            "ai_session_id": "session:sha256:test",
            "message": "test transcript line",
            "capabilities": {
                "transcript": "observed",
                "mcp_events": "partial",
                "skill_events": "partial",
                "hook_events": "not_observed",
            },
            "diagnostics": [],
        }
    })
}

#[tokio::test]
async fn rejects_missing_bearer_token() {
    let (app, _dir) = test_app(Some("secret"));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
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
                .uri("/v1/ai-transcripts")
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
                .uri("/v1/ai-transcripts")
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
    assert_eq!(value["receipts"][0]["disposition"], "accepted");
}

#[tokio::test]
async fn rejects_unsupported_envelope_version_before_any_receipt_or_log_write() {
    let (app, dir) = test_app(Some("secret"));
    let mut record = sample_record();
    record["envelope"]["version"] = serde_json::json!(EVIDENCE_ENVELOPE_VERSION + 1);
    let body = serde_json::to_string(&serde_json::json!({"records": [record]})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
        "unsupported_evidence_envelope_version"
    );
    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM ai_transcript_forward_receipts",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn mixed_valid_and_invalid_batch_is_rejected_atomically() {
    let (app, dir) = test_app(Some("secret"));
    let valid = sample_record();
    let mut invalid = sample_record();
    invalid["envelope"]["source_record_id"] =
        serde_json::json!(format!("sha256:{}", "f".repeat(64)));
    invalid["envelope"]["source"]["source_revision"] = serde_json::json!("not-a-digest");
    let body = serde_json::to_string(&serde_json::json!({
        "records": [valid, invalid]
    }))
    .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0,
        "a rejected sibling must not leave a canonical log behind"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM ai_transcript_forward_receipts",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "a rejected sibling must not leave a receipt behind"
    );
}

#[tokio::test]
async fn replay_returns_duplicate_receipt_without_duplicate_log_row() {
    let (app, dir) = test_app(Some("secret"));
    let body = serde_json::to_string(&serde_json::json!({"records": [sample_record()]})).unwrap();
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["accepted"], 1);
    assert_eq!(value["receipts"][0]["disposition"], "duplicate");

    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    let log_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    let receipt_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1);
    assert_eq!(receipt_count, 1);
}

#[tokio::test]
async fn receiver_scrubs_hostile_envelope_fields_before_persistence() {
    let (app, dir) = test_app(Some("secret"));
    let mut record = sample_record();
    let envelope = record["envelope"].as_object_mut().unwrap();
    envelope.insert(
        "message".into(),
        serde_json::json!("token=sk-canary-secret"),
    );
    envelope.insert(
        "hostname".into(),
        serde_json::json!("host TOKEN=host-canary"),
    );
    envelope["source"]["title"] = serde_json::json!("secret=title-canary");
    envelope["source"]["native_session_id"] = serde_json::json!("sk-session-canary");
    envelope["diagnostics"] = serde_json::json!([
        {"code":"TOKEN=diagnostic-canary", "detail":"Authorization: Bearer sk-diagnostic-canary"}
    ]);
    let body = serde_json::to_string(&serde_json::json!({"records": [record]})).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    let persisted: String = conn
        .query_row(
            "SELECT message || ' ' || hostname || ' ' || metadata_json FROM logs LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for canary in [
        "sk-canary-secret",
        "host-canary",
        "title-canary",
        "sk-session-canary",
        "diagnostic-canary",
        "sk-diagnostic-canary",
    ] {
        assert!(
            !persisted.contains(canary),
            "persisted canary leaked: {canary}"
        );
    }
    assert!(persisted.contains("REDACTED"));
    let fts_canary_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs_fts WHERE logs_fts MATCH 'canary'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        fts_canary_count, 0,
        "a redacted secret must not be discoverable through transcript FTS"
    );
}

#[test]
fn transcript_timestamp_is_scrubbed_bounded_and_canonicalized_before_persistence() {
    assert_eq!(
        safe_timestamp("2026-07-09T00:00:00Z"),
        Some("2026-07-09T00:00:00.000Z".into())
    );
    assert_eq!(safe_timestamp("token=timestamp-canary"), None);
    assert_eq!(safe_timestamp(&"x".repeat(MAX_TIMESTAMP_CHARS + 1)), None);
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
                .uri("/v1/ai-transcripts")
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
                .uri("/v1/ai-transcripts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
