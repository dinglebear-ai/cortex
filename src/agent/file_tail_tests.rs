use super::*;

#[test]
fn source_id_is_uri_safe_and_stable() {
    assert_eq!(source_id("plex media/server"), "plex-media-server");
    assert_eq!(source_id("---"), "file-tail");
}

#[test]
fn delivery_keys_distinguish_identical_records_at_the_same_timestamp() {
    let config = FileTailForwardConfig {
        source: FileTailSource {
            path: "/var/log/app.log".into(),
            tag: Some("app".into()),
        },
        target: "http://cortex.test".into(),
        token: None,
        hostname: "host-a".into(),
        checkpoint_path: "/tmp/file-tail-test.json".into(),
    };

    let first = delivery_key(
        &config,
        "app",
        "2026-09-13T12:00:00.000Z",
        "same",
        1_000,
        41,
    );
    let second = delivery_key(
        &config,
        "app",
        "2026-09-13T12:00:00.000Z",
        "same",
        1_000,
        42,
    );

    assert_ne!(first, second);
    assert_eq!(
        first,
        delivery_key(
            &config,
            "app",
            "2026-09-13T12:00:00.000Z",
            "same",
            1_000,
            41,
        )
    );
}

fn test_config(dir: &std::path::Path, target: String) -> FileTailForwardConfig {
    FileTailForwardConfig {
        source: FileTailSource {
            path: dir.join("app.log"),
            tag: Some("app".into()),
        },
        target,
        token: None,
        hostname: "host-a".into(),
        checkpoint_path: dir.join("state.json"),
    }
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn file_tail_outage_restart_replays_pending_and_offline_backlog() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let config = test_config(dir.path(), server.uri());
    std::fs::write(&config.source.path, "pre-install history\n").unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/file-tails"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let runner = tokio::spawn(run(config.clone()));
    wait_until(|| config.checkpoint_path.exists()).await;
    std::fs::write(&config.source.path, "pre-install history\noutage record\n").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let first = server.received_requests().await.unwrap()[0].body.clone();
    runner.abort();
    let _ = runner.await;
    std::fs::write(
        &config.source.path,
        "pre-install history\noutage record\nappended while stopped\n",
    )
    .unwrap();
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/file-tails"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"accepted": 1})))
        .mount(&server)
        .await;
    let restarted = tokio::spawn(run(config.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    restarted.abort();
    let _ = restarted.await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body, first,
        "pending timestamp/key/body survive restart unchanged"
    );
    let second: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(second["records"][0]["message"], "appended while stopped");
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("pre-install history"));
}

#[tokio::test]
async fn file_tail_requires_receipts_and_distinguishes_permanent_rejections() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let config = test_config(dir.path(), server.uri());
    let client = reqwest::Client::new();
    let record = build_record(&config, "evidence");
    for (status, body, permanent, success) in [
        (400, "{}", true, false),
        (401, "{}", true, false),
        (429, "{}", false, false),
        (503, "{}", false, false),
        (200, "{}", false, false),
        (200, "not-json", false, false),
        (200, "{\"accepted\":0}", false, true),
        (200, "{\"accepted\":1}", false, true),
        (200, "{\"accepted\":2}", false, false),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let result = send_record(&client, &config, &record).await;
        assert_eq!(result.is_ok(), success, "status={status}, body={body}");
        if let Err(error) = result {
            assert_eq!(
                error.downcast_ref::<PermanentRejection>().is_some(),
                permanent
            );
        }
    }
}

#[tokio::test]
async fn file_tail_invalid_config_fails_before_network_or_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path(), "http://127.0.0.1:1".into());
    config.source.tag = Some("x".repeat(256));
    assert!(
        run(config.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid file-tail tag")
    );
    assert!(!config.checkpoint_path.exists());
}

#[tokio::test]
async fn file_tail_state_replaces_existing_checkpoint_and_reopens_latest_state() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), "http://cortex.test".into());
    std::fs::write(&config.source.path, "history\n").unwrap();
    let mut reader = super::super::tail_reader::TailReader::open_durable(&config.source.path)
        .await
        .unwrap();
    let (store, initial) = Store::open(&config.checkpoint_path).await.unwrap();
    assert!(initial.is_none());
    store
        .save(&State {
            cursor: reader.cursor().await.unwrap(),
            pending: None,
        })
        .await
        .unwrap();
    let initial_bytes = std::fs::read(&config.checkpoint_path).unwrap();
    std::fs::write(&config.source.path, "history\nnew record\n").unwrap();
    let record = build_record(&config, &reader.next_line().await.unwrap().unwrap());
    let pending = State {
        cursor: reader.cursor().await.unwrap(),
        pending: Some(record),
    };
    store.save(&pending).await.unwrap();
    assert_ne!(
        std::fs::read(&config.checkpoint_path).unwrap(),
        initial_bytes
    );
    drop(store);
    let (store, restored) = Store::open(&config.checkpoint_path).await.unwrap();
    assert_eq!(
        serde_json::to_value(restored.unwrap()).unwrap(),
        serde_json::to_value(&pending).unwrap()
    );
    let acknowledged = State {
        cursor: pending.cursor,
        pending: None,
    };
    store.save(&acknowledged).await.unwrap();
    drop(store);
    let (_store, restored) = Store::open(&config.checkpoint_path).await.unwrap();
    assert_eq!(
        serde_json::to_value(restored.unwrap()).unwrap(),
        serde_json::to_value(&acknowledged).unwrap()
    );
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count(),
        0
    );
}

#[tokio::test]
async fn file_tail_state_is_bounded_locked_and_corruption_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let (owner, state) = Store::open(&path).await.unwrap();
    assert!(state.is_none());
    assert!(Store::open(&path).await.is_err());
    drop(owner);
    std::fs::write(&path, "broken checkpoint").unwrap();
    assert!(Store::open(&path).await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken checkpoint");
    std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(Store::open(&path).await.is_err());
}

#[tokio::test]
async fn file_tail_lost_receipt_retries_identical_outbox_even_after_source_disappears() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let config = test_config(dir.path(), server.uri());
    std::fs::write(&config.source.path, "history\n").unwrap();
    let mut reader = super::super::tail_reader::TailReader::open_durable(&config.source.path)
        .await
        .unwrap();
    std::fs::write(&config.source.path, "history\npending\n").unwrap();
    let record = build_record(&config, &reader.next_line().await.unwrap().unwrap());
    let expected_key = record.idempotency_key.clone();
    let (store, _) = Store::open(&config.checkpoint_path).await.unwrap();
    store
        .save(&State {
            cursor: reader.cursor().await.unwrap(),
            pending: Some(record),
        })
        .await
        .unwrap();
    drop(store);
    drop(reader);
    // A successful server insertion whose response lost its receipt must not
    // clear the local outbox, and its restart retry may return accepted=0.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let runner = tokio::spawn(run(config.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    runner.abort();
    let _ = runner.await;
    let first_body = server.received_requests().await.unwrap()[0].body.clone();
    std::fs::remove_file(&config.source.path).unwrap();
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"accepted":0})))
        .mount(&server)
        .await;
    // The saved record is acknowledged before the absent source causes a read error.
    assert!(
        run(config.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("source unavailable")
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, first_body);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body["records"][0]["idempotency_key"].as_str(),
        expected_key.as_deref()
    );
    let (_, state) = Store::open(&config.checkpoint_path).await.unwrap();
    assert!(state.unwrap().pending.is_none());
}
