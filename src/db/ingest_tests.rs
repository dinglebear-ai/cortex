use super::*;
use crate::config::StorageConfig;
use crate::db::{DbPool, LogBatchEntry, init_pool, list_hosts, tail_logs};

fn test_storage_config(db_path: std::path::PathBuf) -> StorageConfig {
    StorageConfig::for_test(db_path)
}

/// Create an isolated test pool using a temp file (not :memory: — FTS5 needs file)
fn test_pool() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).unwrap();
    (pool, dir) // keep dir alive for test duration
}

fn make_entry(ts: &str, host: &str, severity: &str, msg: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: severity.to_string(),
        app_name: None,
        process_id: None,
        message: msg.to_string(),
        raw: msg.to_string(),
        source_ip: "127.0.0.1:514".to_string(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn test_insert_and_tail() {
    let (pool, _dir) = test_pool();
    let entries = vec![
        make_entry("2026-01-01T00:00:01Z", "host-a", "err", "first error"),
        make_entry("2026-01-01T00:00:02Z", "host-a", "info", "second info"),
        make_entry("2026-01-01T00:00:03Z", "host-b", "warning", "third warning"),
    ];
    let n = insert_logs_batch(&pool, &entries).unwrap();
    assert_eq!(n, 3);

    let rows = tail_logs(&pool, None, None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn test_host_aggregation() {
    let (pool, _dir) = test_pool();
    let entries = vec![
        make_entry("2026-01-01T00:00:01Z", "host-a", "info", "msg1"),
        make_entry("2026-01-01T00:00:02Z", "host-a", "info", "msg2"),
        make_entry("2026-01-01T00:00:03Z", "host-b", "info", "msg3"),
    ];
    insert_logs_batch(&pool, &entries).unwrap();

    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 2);
    // host-a should have log_count = 2
    let ha = hosts.iter().find(|h| h.hostname == "host-a").unwrap();
    assert_eq!(ha.log_count, 2);
}

#[test]
fn test_batch_multiple_entries_same_host() {
    let (pool, _dir) = test_pool();
    let entries = vec![
        make_entry("2026-01-01T00:00:01Z", "host-x", "info", "msg1"),
        make_entry("2026-01-01T00:00:02Z", "host-x", "info", "msg2"),
        make_entry("2026-01-01T00:00:03Z", "host-x", "err", "msg3"),
    ];
    insert_logs_batch(&pool, &entries).unwrap();

    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].hostname, "host-x");
    assert_eq!(hosts[0].log_count, 3);
}

#[test]
fn test_batch_empty() {
    let (pool, _dir) = test_pool();
    let result = insert_logs_batch(&pool, &[]);
    assert!(result.is_ok(), "empty batch should not error");
    assert_eq!(result.unwrap(), 0);

    let rows = tail_logs(&pool, None, None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 0, "no rows should exist after empty batch");

    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 0, "no hosts should exist after empty batch");
}

#[test]
fn test_batch_mixed_hosts() {
    let (pool, _dir) = test_pool();
    let entries = vec![
        make_entry("2026-01-01T00:00:01Z", "host-a", "info", "a msg1"),
        make_entry("2026-01-01T00:00:02Z", "host-a", "info", "a msg2"),
        make_entry("2026-01-01T00:00:03Z", "host-b", "info", "b msg1"),
    ];
    insert_logs_batch(&pool, &entries).unwrap();

    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 2);

    let ha = hosts.iter().find(|h| h.hostname == "host-a").unwrap();
    assert_eq!(ha.log_count, 2);

    let hb = hosts.iter().find(|h| h.hostname == "host-b").unwrap();
    assert_eq!(hb.log_count, 1);
}

#[test]
#[allow(clippy::type_complexity)]
fn insert_logs_batch_persists_enrichment_fields() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    let pool = crate::db::pool::init_pool(&config).unwrap();

    let entry = crate::db::LogBatchEntry {
        timestamp: "2026-05-16T10:00:00Z".to_string(),
        hostname: "test-host".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("swag".to_string()),
        process_id: None,
        message: "GET / 200".to_string(),
        raw: "raw line".to_string(),
        source_ip: "docker://localhost/swag/stdout".to_string(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(r#"{"swag":{"method":"GET"}}"#.to_string()),
        http_status: Some(200),
        auth_outcome: None,
        dns_blocked: None,
        event_action: Some("http_request".to_string()),
        parse_error: None,
    };

    super::insert_logs_batch(&pool, &[entry]).expect("insert ok");

    let conn = pool.get().unwrap();
    let row: (Option<i32>, Option<String>, Option<i64>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT http_status, auth_outcome, dns_blocked, event_action, parse_error FROM logs LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row.0, Some(200));
    assert_eq!(row.1, None);
    assert_eq!(row.2, None);
    assert_eq!(row.3, Some("http_request".to_string()));
    assert_eq!(row.4, None);
}

#[test]
fn insert_logs_batch_in_tx_returns_ids_in_input_order() {
    let (pool, _dir) = test_pool();
    let mut conn = pool.get().unwrap();
    let tx = conn.transaction().unwrap();

    let entries = vec![
        make_entry("2026-06-01T00:00:00.000Z", "devhost", "info", "first"),
        make_entry("2026-06-01T00:00:01.000Z", "devhost", "info", "second"),
    ];

    let ids = insert_logs_batch_in_tx(&tx, &entries).unwrap();
    tx.commit().unwrap();
    drop(conn);

    assert_eq!(ids.len(), 2);
    assert!(
        ids[1] > ids[0],
        "second row's id must be greater than first"
    );

    let conn = pool.get().unwrap();
    let stored_message: String = conn
        .query_row("SELECT message FROM logs WHERE id = ?1", [ids[0]], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored_message, "first");
    let stored_message: String = conn
        .query_row("SELECT message FROM logs WHERE id = ?1", [ids[1]], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored_message, "second");
}
