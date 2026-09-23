use super::*;

#[tokio::test]
async fn archived_transcript_replay_accepts_a_changed_locator() {
    use sha2::{Digest, Sha256};

    let (app, dir) = test_app(Some("secret"));
    let original = sample_record();
    let envelope: EvidenceEnvelope = serde_json::from_value(original["envelope"].clone()).unwrap();
    let mut v2_evidence = scrub_envelope(envelope).unwrap();
    v2_evidence.source.title = None;
    v2_evidence.source.title_provenance = None;
    let v2_fingerprint = format!(
        "evidence-v2:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&v2_evidence).unwrap())
    );

    let first = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [original]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    conn.execute(
        "UPDATE ai_transcript_forward_receipts SET request_fingerprint = ?1",
        [&v2_fingerprint],
    )
    .unwrap();

    let mut archived = sample_record();
    archived["envelope"]["source"]["locator"] = json!(format!("sha256:{}", "f".repeat(64)));
    let replay = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [archived.clone()]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["receipts"][0]["disposition"], "duplicate");

    archived["envelope"]["source"]["locator"] = json!(format!("sha256:{}", "1".repeat(64)));
    let second_replay = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [archived]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(second_replay.status(), StatusCode::OK);
    let fingerprint: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(fingerprint.starts_with("evidence-v3:sha256:"));

    let mut changed_evidence = sample_record();
    changed_evidence["envelope"]["source"]["locator"] = json!(format!("sha256:{}", "1".repeat(64)));
    changed_evidence["envelope"]["message"] = json!("different evidence");
    let conflict = app
        .oneshot(transcript_request(
            json!({"records": [changed_evidence]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let log_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(log_count, 1);
}

#[tokio::test]
async fn title_changes_and_missing_metadata_are_duplicate_replays() {
    let (app, dir) = test_app(Some("secret"));
    for title in [Some("Before"), Some("After"), None] {
        let mut record = sample_record();
        record["envelope"]["source"]["title"] = json!(title);
        record["envelope"]["source"]["title_provenance"] = json!(title.map(|_| "codex_state"));
        let response = app
            .clone()
            .oneshot(transcript_request(json!({"records": [record]}).to_string()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if title != Some("Before") {
            assert_eq!(body["receipts"][0]["disposition"], "duplicate");
        }
    }
    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn old_full_envelope_receipt_accepts_title_and_locator_changes_but_not_evidence_changes() {
    check_old_receipt_replay(false).await;
    check_old_receipt_replay(true).await;
}

async fn check_old_receipt_replay(timestamp_absent: bool) {
    use sha2::{Digest, Sha256};
    let (app, dir) = test_app(Some("secret"));
    let mut original = sample_record();
    if timestamp_absent {
        original["envelope"]["timestamp"] = serde_json::Value::Null;
    }
    let envelope: EvidenceEnvelope = serde_json::from_value(original["envelope"].clone()).unwrap();
    let scrubbed = scrub_envelope(envelope).unwrap();
    let old_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&scrubbed).unwrap())
    );
    let first = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [original]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    conn.execute(
        "UPDATE ai_transcript_forward_receipts SET request_fingerprint = ?1",
        [&old_hash],
    )
    .unwrap();
    // Bounded canonical metadata may be unavailable; an exact old request
    // must still be a duplicate, using its stored fingerprint as proof.
    let metadata: String = conn
        .query_row("SELECT metadata_json FROM logs", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "UPDATE logs SET metadata_json = '{\"metadata_truncated\":true}'",
        [],
    )
    .unwrap();
    let exact = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [original.clone()]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(exact.status(), StatusCode::OK);
    conn.execute("UPDATE logs SET metadata_json = ?1", [metadata])
        .unwrap();
    // Restore the old receipt to independently qualify title-change upgrade.
    conn.execute(
        "UPDATE ai_transcript_forward_receipts SET request_fingerprint = ?1",
        [&old_hash],
    )
    .unwrap();
    let mut replay = sample_record();
    if timestamp_absent {
        replay["envelope"]["timestamp"] = serde_json::Value::Null;
    }
    replay["envelope"]["source"]["title"] = json!("Renamed");
    replay["envelope"]["source"]["locator"] = json!(format!("sha256:{}", "f".repeat(64)));
    let renamed = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [replay.clone()]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    replay["envelope"]["message"] = json!("different evidence");
    let changed = app
        .oneshot(transcript_request(json!({"records": [replay]}).to_string()))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
}
