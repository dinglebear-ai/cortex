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

    let (lines, checkpoint) = read_new_zsh_lines(&path, 0, 2).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(checkpoint, 2);

    let (lines, checkpoint) = read_new_zsh_lines(&path, checkpoint, 2).unwrap();
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

    let (lines, checkpoint) = read_new_zsh_lines(&path, 0, 500).unwrap();
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

    let (records, new_line) = scan_zsh(&path, "test-host", 0, 500).unwrap();
    assert_eq!(new_line, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, "zsh");
    assert_eq!(records[0].hostname, "test-host");
    assert_eq!(records[0].duration_ms, Some(3000));
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
        atuin_timestamp_ns: 123,
        atuin_id: "row-9".to_string(),
    };
    save_checkpoint(&checkpoint_path, &checkpoint).unwrap();

    let loaded = load_checkpoint(&checkpoint_path);
    assert_eq!(loaded.zsh_line, 42);
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
