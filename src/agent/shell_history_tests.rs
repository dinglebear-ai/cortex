use super::*;
use std::io::Write;

fn write_file(path: &std::path::Path, content: &str) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

#[test]
fn read_new_zsh_lines_respects_limit_and_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".zsh_history");
    write_file(
        &path,
        ": 1716500000:1;echo one\n: 1716500001:2;echo two\n: 1716500002:3;echo three\n",
    );

    let (lines, checkpoint, byte_offset) = read_new_zsh_lines(&path, 0, 0, 2).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(checkpoint, 2);

    let (lines, checkpoint, _) = read_new_zsh_lines(&path, checkpoint, byte_offset, 2).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(checkpoint, 3);
}

#[test]
fn read_new_zsh_lines_tolerates_non_utf8_bytes_without_aborting_the_whole_read() {
    // Regression: a real `.zsh_history` file can contain stray non-UTF-8
    // bytes (pasted binary output, odd terminal escapes). `BufRead::lines()`
    // hard-errors the ENTIRE read on the first bad byte, silently blocking
    // every valid line after it from ever forwarding again. Must tolerate
    // this and keep reading.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".zsh_history");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b": 1716500000:1;echo one\n");
    bytes.extend_from_slice(b": 1716500001:2;echo \xff\xfebroken\n"); // invalid UTF-8
    bytes.extend_from_slice(b": 1716500002:3;echo three\n");
    std::fs::write(&path, &bytes).unwrap();

    let (lines, checkpoint, _) = read_new_zsh_lines(&path, 0, 0, 500).unwrap();
    assert_eq!(lines.len(), 3, "all three lines must be read: {lines:?}");
    assert_eq!(checkpoint, 3);
    assert!(parse_zsh_extended_history_line(&lines[0]).is_some());
    assert!(parse_zsh_extended_history_line(&lines[2]).is_some());
}

#[test]
fn scan_zsh_parses_extended_history_and_scrubs_command() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".zsh_history");
    write_file(
        &path,
        ": 1716500000:3;export OPENAI_API_KEY=sk-proj-super-secret-value-long-enough-to-match\n",
    );

    let (records, new_line, _) = scan_zsh(&path, "test-host", 0, 0, 500).unwrap();
    assert_eq!(new_line, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, "zsh");
    assert_eq!(records[0].hostname, "test-host");
    assert_eq!(records[0].duration_ms, Some(3000));
    assert!(
        records[0]
            .idempotency_key
            .as_deref()
            .is_some_and(|key| key.starts_with("zsh:"))
    );
    assert!(
        !records[0]
            .command
            .contains("sk-proj-super-secret-value-long-enough-to-match"),
        "command must be scrubbed: {}",
        records[0].command
    );
}

fn make_atuin_db(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE history (
            id TEXT PRIMARY KEY,
            timestamp INTEGER,
            duration INTEGER,
            exit INTEGER,
            command TEXT,
            cwd TEXT,
            session TEXT,
            hostname TEXT,
            deleted_at INTEGER
        );",
    )
    .unwrap();
}

fn insert_atuin_row(path: &std::path::Path, id: &str, timestamp_ns: i64, command: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO history (id, timestamp, duration, exit, command, cwd, session, hostname, deleted_at)
         VALUES (?1, ?2, 500000000, 0, ?3, '/home/test', 'sess-1', 'test-host', NULL)",
        rusqlite::params![id, timestamp_ns, command],
    )
    .unwrap();
}

#[test]
fn scan_atuin_returns_rows_past_cursor_and_reports_new_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    make_atuin_db(&path);
    insert_atuin_row(&path, "row-1", 1_716_500_000_000_000_000, "echo one");
    insert_atuin_row(&path, "row-2", 1_716_500_001_000_000_000, "echo two");

    let (records, last_ts, last_id) = scan_atuin(&path, "test-host", 0, "", 500).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].command, "echo one");
    assert_eq!(
        records[0].idempotency_key,
        Some(source_record_key("atuin", "row-1"))
    );
    assert_eq!(last_ts, 1_716_500_001_000_000_000);
    assert_eq!(last_id, "row-2");

    // Re-scanning from the new cursor should return nothing.
    let (records_again, _, _) = scan_atuin(&path, "test-host", last_ts, &last_id, 500).unwrap();
    assert!(records_again.is_empty());
}

#[test]
fn checkpoint_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");
    let checkpoint = Checkpoint {
        zsh_line: 42,
        zsh_byte_offset: 4_096,
        zsh_cursor_fingerprint: "bounded-window".into(),
        zsh_file_len: 8_192,
        zsh_modified_ns: 123_456,
        zsh_file_dev: 7,
        zsh_file_ino: 9,
        atuin_timestamp_ns: 123,
        atuin_id: "row-9".to_string(),
        ..Checkpoint::default()
    };
    save_checkpoint(&checkpoint_path, &checkpoint).unwrap();

    let loaded = load_checkpoint(&checkpoint_path);
    assert_eq!(loaded.zsh_line, 42);
    assert_eq!(loaded.zsh_byte_offset, 4_096);
    assert_eq!(loaded.zsh_cursor_fingerprint, "bounded-window");
    assert_eq!(loaded.zsh_file_len, 8_192);
    assert_eq!(loaded.zsh_modified_ns, 123_456);
    assert_eq!(loaded.zsh_file_dev, 7);
    assert_eq!(loaded.zsh_file_ino, 9);
    assert_eq!(loaded.atuin_timestamp_ns, 123);
    assert_eq!(loaded.atuin_id, "row-9");
}

#[tokio::test]
async fn scan_and_forward_sends_zsh_and_atuin_records_together() {
    let dir = tempfile::tempdir().unwrap();
    let zsh_path = dir.path().join(".zsh_history");
    write_file(&zsh_path, ": 1716500000:1;echo from-zsh\n");
    let atuin_path = dir.path().join("history.db");
    make_atuin_db(&atuin_path);
    insert_atuin_row(
        &atuin_path,
        "row-1",
        1_716_500_000_000_000_000,
        "echo from-atuin",
    );

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/shell-history"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"accepted": 2})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(zsh_path),
        atuin_db_path: Some(atuin_path),
        target: server.uri(),
        token: Some("test-token".to_string()),
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(20),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    let sent = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent, 2);
    assert_eq!(checkpoint.zsh_line, 1);
    assert_eq!(checkpoint.atuin_id, "row-1");
    let requests = server.received_requests().await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let records = payload["records"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["source"], "zsh");
    assert_eq!(records[0]["command"], "echo from-zsh");
    assert_eq!(records[1]["source"], "atuin");
    assert_eq!(records[1]["command"], "echo from-atuin");
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer test-token"
    );

    let sent_again = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent_again, 0);
}

#[tokio::test]
async fn invalid_page_advances_disk_cursor_before_later_valid_command() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    write_file(
        &path,
        &("invalid\n".repeat(500) + ": 1716500000:1;echo reachable\n"),
    );
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(path),
        atuin_db_path: None,
        target: server.uri(),
        token: None,
        hostname: "host".into(),
        checkpoint_path: dir.path().join("checkpoint"),
        poll_interval: Duration::from_secs(1),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        0
    );
    assert_eq!(checkpoint.zsh_line, 500);
    let disk: Checkpoint =
        serde_json::from_slice(&std::fs::read(&config.checkpoint_path).unwrap()).unwrap();
    assert_eq!(disk.zsh_line, 500);
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(checkpoint.zsh_line, 501);
}

#[tokio::test]
async fn same_length_zsh_rewrite_replays_the_new_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    write_file(&path, ": 1716500000:1;echo old\n");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(path.clone()),
        atuin_db_path: None,
        target: server.uri(),
        token: None,
        hostname: "host".into(),
        checkpoint_path: dir.path().join("checkpoint"),
        poll_interval: Duration::from_secs(1),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    write_file(&path, ": 1716500001:1;echo new\n");
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        checkpoint.zsh_cursor_fingerprint,
        zsh_cursor_fingerprint(&path, checkpoint.zsh_byte_offset).unwrap()
    );
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        0,
        "the rewritten window must converge after one successful replay"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["records"][0]["command"], "echo old");
    assert_eq!(second["records"][0]["command"], "echo new");
    assert_ne!(
        first["records"][0]["idempotency_key"], second["records"][0]["idempotency_key"],
        "a changed source record at the same line must not reuse its receipt key"
    );
    let disk: Checkpoint =
        serde_json::from_slice(&std::fs::read(&config.checkpoint_path).unwrap()).unwrap();
    assert_eq!(disk.zsh_line, 1);
    assert_eq!(
        disk.zsh_cursor_fingerprint,
        checkpoint.zsh_cursor_fingerprint
    );
}

#[tokio::test]
async fn legacy_checkpoint_migrates_to_bounded_cursor_while_idle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    write_file(&path, ": 1716500000:1;echo already-forwarded\n");
    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(path.clone()),
        atuin_db_path: None,
        target: "http://127.0.0.1:1".to_string(),
        token: None,
        hostname: "host".into(),
        checkpoint_path: dir.path().join("checkpoint"),
        poll_interval: Duration::from_secs(1),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint {
        zsh_line: 1,
        zsh_prefix_hash: String::new(),
        ..Checkpoint::default()
    };

    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        0
    );
    let expected_fingerprint =
        zsh_cursor_fingerprint(&path, std::fs::metadata(&path).unwrap().len()).unwrap();
    assert_eq!(checkpoint.zsh_cursor_fingerprint, expected_fingerprint);
    assert!(checkpoint.zsh_prefix_hash.is_empty());
    let disk: Checkpoint =
        serde_json::from_slice(&std::fs::read(&config.checkpoint_path).unwrap()).unwrap();
    assert_eq!(disk.zsh_line, 1);
    assert_eq!(disk.zsh_cursor_fingerprint, expected_fingerprint);
}

#[tokio::test]
async fn large_idle_zsh_file_does_not_read_history_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(8 * 1024 * 1024).unwrap();
    drop(file);

    let state = ZshFileState::read(&path).unwrap();
    let cursor_fingerprint = zsh_cursor_fingerprint(&path, state.len).unwrap();
    let mut checkpoint = Checkpoint {
        zsh_line: 250_000,
        zsh_byte_offset: state.len,
        zsh_cursor_fingerprint: cursor_fingerprint,
        zsh_file_len: state.len,
        zsh_modified_ns: state.modified_ns,
        zsh_file_dev: state.dev,
        zsh_file_ino: state.ino,
        ..Checkpoint::default()
    };
    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(path),
        atuin_db_path: None,
        target: "http://127.0.0.1:1".into(),
        token: None,
        hostname: "host".into(),
        checkpoint_path: dir.path().join("checkpoint"),
        poll_interval: Duration::from_secs(1),
    };

    ZSH_BYTES_READ.set(0);
    assert_eq!(
        scan_and_forward(&config, &reqwest::Client::new(), &mut checkpoint)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        ZSH_BYTES_READ.get(),
        0,
        "an unchanged idle file must be handled from metadata alone"
    );
}

#[test]
fn preserved_zsh_prefix_reuses_only_unchanged_record_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    write_file(
        &path,
        ": 1716500000:1;echo preserved\n: 1716500001:1;echo old suffix\n",
    );
    let (before, _, _) = scan_zsh(&path, "host", 0, 0, 500).unwrap();

    write_file(
        &path,
        ": 1716500000:1;echo preserved\n: 1716500002:1;echo new suffix\n",
    );
    let (after, _, _) = scan_zsh(&path, "host", 0, 0, 500).unwrap();

    assert_eq!(before[0].idempotency_key, after[0].idempotency_key);
    assert_ne!(before[1].idempotency_key, after[1].idempotency_key);
}

#[tokio::test]
async fn prefix_hash_failure_preserves_in_memory_and_disk_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");
    let original = Checkpoint {
        zsh_line: 7,
        zsh_prefix_hash: "known-prefix".into(),
        atuin_timestamp_ns: 42,
        atuin_id: "known-row".into(),
        ..Checkpoint::default()
    };
    save_checkpoint(&checkpoint_path, &original).unwrap();
    let config = ShellHistoryForwardConfig {
        zsh_history_path: Some(dir.path().join("missing-history")),
        atuin_db_path: None,
        target: "http://127.0.0.1:1".into(),
        token: None,
        hostname: "host".into(),
        checkpoint_path: checkpoint_path.clone(),
        poll_interval: Duration::from_secs(1),
    };
    let mut checkpoint = load_checkpoint(&checkpoint_path);

    let error = scan_and_forward(&config, &reqwest::Client::new(), &mut checkpoint)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("missing-history"));
    assert_eq!(checkpoint.zsh_line, original.zsh_line);
    assert_eq!(checkpoint.zsh_prefix_hash, original.zsh_prefix_hash);
    assert_eq!(checkpoint.atuin_timestamp_ns, original.atuin_timestamp_ns);
    assert_eq!(checkpoint.atuin_id, original.atuin_id);
    let disk = load_checkpoint(&checkpoint_path);
    assert_eq!(disk.zsh_line, original.zsh_line);
    assert_eq!(disk.zsh_prefix_hash, original.zsh_prefix_hash);
    assert_eq!(disk.atuin_timestamp_ns, original.atuin_timestamp_ns);
    assert_eq!(disk.atuin_id, original.atuin_id);
}
