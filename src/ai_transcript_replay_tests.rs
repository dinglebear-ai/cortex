use super::*;

fn receipt_v2_fingerprint(record: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let envelope: EvidenceEnvelope =
        serde_json::from_value(record["envelope"].clone()).unwrap();
    let mut evidence = scrub_envelope(envelope).unwrap();
    evidence.source.title = None;
    evidence.source.title_provenance = None;
    format!(
        "evidence-v2:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&evidence).unwrap())
    )
}

fn transient_receipt_v3_fingerprint(record: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let envelope: EvidenceEnvelope =
        serde_json::from_value(record["envelope"].clone()).unwrap();
    let mut evidence = scrub_envelope(envelope).unwrap();
    evidence.source.title = None;
    evidence.source.title_provenance = None;
    evidence.source.locator.clear();
    format!(
        "evidence-v3:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&evidence).unwrap())
    )
}

#[tokio::test]
async fn archived_transcript_replay_accepts_changed_locator_and_preserves_v2_receipt() {
    let (app, dir) = test_app(Some("secret"));
    let original = sample_record();
    let original_v2 = receipt_v2_fingerprint(&original);

    let first = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [original]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let conn = rusqlite::Connection::open(dir.path().join("ai-transcript-ingest-test.db")).unwrap();
    let fingerprint: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fingerprint, original_v2);

    for locator in ["f", "1"] {
        let mut archived = sample_record();
        archived["envelope"]["source"]["locator"] =
            json!(format!("sha256:{}", locator.repeat(64)));
        let replay = app
            .clone()
            .oneshot(transcript_request(
                json!({"records": [archived]}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let body = axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["receipts"][0]["disposition"], "duplicate");
    }

    let fingerprint: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fingerprint, original_v2);

    let mut changed_evidence = sample_record();
    changed_evidence["envelope"]["source"]["locator"] =
        json!(format!("sha256:{}", "1".repeat(64)));
    changed_evidence["envelope"]["message"] = json!("different evidence");
    let conflict = app
        .oneshot(transcript_request(
            json!({"records": [changed_evidence]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let (log_count, fingerprint): (i64, String) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM logs), request_fingerprint
             FROM ai_transcript_forward_receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(log_count, 1);
    assert_eq!(fingerprint, original_v2);
}

#[tokio::test]
async fn transient_v3_receipt_rebinds_to_rollback_safe_v2() {
    let (app, dir) = test_app(Some("secret"));
    let original = sample_record();
    let original_v2 = receipt_v2_fingerprint(&original);
    let transient_v3 = transient_receipt_v3_fingerprint(&original);

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
        [&transient_v3],
    )
    .unwrap();

    let mut archived = sample_record();
    archived["envelope"]["source"]["locator"] =
        json!(format!("sha256:{}", "f".repeat(64)));
    let replay = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [archived]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);

    let fingerprint: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fingerprint, original_v2);

    let mut moved_again = sample_record();
    moved_again["envelope"]["source"]["locator"] =
        json!(format!("sha256:{}", "1".repeat(64)));
    let replay = app
        .oneshot(transcript_request(
            json!({"records": [moved_again]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
}

#[tokio::test]
async fn legacy_null_fingerprint_receipt_accepts_locator_move_and_rebinds_v2() {
    let (app, dir) = test_app(Some("secret"));
    let original = sample_record();
    let original_v2 = receipt_v2_fingerprint(&original);

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
        "UPDATE ai_transcript_forward_receipts SET request_fingerprint = NULL",
        [],
    )
    .unwrap();

    let mut archived = sample_record();
    archived["envelope"]["source"]["locator"] =
        json!(format!("sha256:{}", "f".repeat(64)));
    let replay = app
        .clone()
        .oneshot(transcript_request(
            json!({"records": [archived]}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);

    let fingerprint: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fingerprint, original_v2);
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
    let expected_v2 = receipt_v2_fingerprint(&original);
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
    let rebound: String = conn
        .query_row(
            "SELECT request_fingerprint FROM ai_transcript_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rebound, expected_v2);
    replay["envelope"]["message"] = json!("different evidence");
    let changed = app
        .oneshot(transcript_request(json!({"records": [replay]}).to_string()))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
}
