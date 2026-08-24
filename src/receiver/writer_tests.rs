use super::*;
use crate::config::StorageConfig;
use crate::db::{self, DbPool};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

fn test_storage_config(db_path: std::path::PathBuf) -> StorageConfig {
    StorageConfig::for_test(db_path)
}

fn test_pool() -> (Arc<DbPool>, StorageConfig, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("syslog-test.db"));
    let pool = Arc::new(db::init_pool(&config).unwrap());
    (pool, config, dir)
}

fn make_entry(message: &str) -> db::LogBatchEntry {
    db::LogBatchEntry {
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hostname: "mymachine".to_string(),
        facility: Some("auth".to_string()),
        severity: "crit".to_string(),
        app_name: Some("su".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: message.to_string(),
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

fn make_entry_for_sender(hostname: &str, source_ip: &str, message: &str) -> db::LogBatchEntry {
    db::LogBatchEntry {
        hostname: hostname.to_string(),
        source_ip: source_ip.to_string(),
        ..make_entry(message)
    }
}

fn envelope_batch(entries: Vec<db::LogBatchEntry>) -> Vec<crate::ingest::IngestEnvelope> {
    entries
        .into_iter()
        .map(crate::ingest::IngestEnvelope::best_effort)
        .collect()
}

#[tokio::test]
async fn flush_batch_retains_entries_while_storage_is_write_blocked() {
    let (pool, mut storage, _dir) = test_pool();
    let storage_state: Arc<Mutex<Option<db::StorageBudgetState>>> = Arc::new(Mutex::new(None));
    let free_disk_mb = db::get_storage_metrics(&pool, &storage)
        .unwrap()
        .free_disk_bytes
        .unwrap()
        / 1_048_576;
    storage.min_free_disk_mb = free_disk_mb + 1024;
    storage.recovery_free_disk_mb = free_disk_mb + 2048;
    *storage_state.lock() = Some(db::StorageBudgetState {
        metrics: db::get_storage_metrics(&pool, &storage).unwrap(),
        write_blocked: true,
    });
    let mut batch = envelope_batch(vec![make_entry("blocked write")]);
    let mut storage_blocked = false;
    let mut summary = IngestSummary::default();
    let observability = Arc::new(crate::observability::RuntimeObservability::default());
    let context = WriterContext::new(
        Arc::clone(&pool),
        storage.clone(),
        Arc::clone(&storage_state),
        crate::receiver::enrichment::EnrichmentConfig::default(),
        Arc::new(crate::enrich::EnrichmentPipeline::new()),
        Arc::clone(&observability),
    );

    flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;

    assert_eq!(batch.len(), 1);
    assert!(storage_blocked);
    assert_eq!(observability.snapshot().writer_logs_retained, 1);
}

#[tokio::test]
async fn flush_batch_resumes_after_storage_recovers() {
    let (pool, storage, _dir) = test_pool();
    let storage_state = Arc::new(Mutex::new(Some(db::StorageBudgetState {
        metrics: db::get_storage_metrics(&pool, &storage).unwrap(),
        write_blocked: false,
    })));
    let mut batch = envelope_batch(vec![make_entry("resumed write")]);
    let mut storage_blocked = true;
    let mut summary = IngestSummary::default();
    let observability = Arc::new(crate::observability::RuntimeObservability::default());
    let context = WriterContext::new(
        Arc::clone(&pool),
        storage.clone(),
        Arc::clone(&storage_state),
        crate::receiver::enrichment::EnrichmentConfig::default(),
        Arc::new(crate::enrich::EnrichmentPipeline::new()),
        Arc::clone(&observability),
    );

    flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;

    assert!(batch.is_empty());
    assert!(!storage_blocked);
    assert_eq!(observability.snapshot().writer_logs_written, 1);
    let rows = db::tail_logs(&pool, None, None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn flush_batch_isolates_bad_rows_and_writes_remaining_entries() {
    let (pool, storage, _dir) = test_pool();
    pool.get()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_bad_message BEFORE INSERT ON logs
             WHEN new.message = 'bad row'
             BEGIN
                 SELECT RAISE(FAIL, 'bad row rejected');
             END;",
        )
        .unwrap();
    let storage_state = Arc::new(Mutex::new(Some(db::StorageBudgetState {
        metrics: db::get_storage_metrics(&pool, &storage).unwrap(),
        write_blocked: false,
    })));
    let mut batch = envelope_batch(vec![
        make_entry("good row one"),
        make_entry("bad row"),
        make_entry("good row two"),
    ]);
    let mut storage_blocked = false;
    let mut summary = IngestSummary::default();
    let observability = Arc::new(crate::observability::RuntimeObservability::default());
    let context = WriterContext::new(
        Arc::clone(&pool),
        storage.clone(),
        Arc::clone(&storage_state),
        crate::receiver::enrichment::EnrichmentConfig::default(),
        Arc::new(crate::enrich::EnrichmentPipeline::new()),
        Arc::clone(&observability),
    );

    flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;

    assert!(batch.is_empty());
    assert_eq!(observability.snapshot().writer_logs_written, 2);
    assert_eq!(observability.snapshot().writer_logs_discarded, 1);
    let rows = db::tail_logs(&pool, None, None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn flush_batch_retains_bounded_entries_for_large_retryable_failures() {
    let (pool, storage, _dir) = test_pool();
    let metrics = db::get_storage_metrics(&pool, &storage).unwrap();
    let lock_conn = rusqlite::Connection::open(&storage.db_path).unwrap();
    lock_conn.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let storage_state = Arc::new(Mutex::new(Some(db::StorageBudgetState {
        metrics,
        write_blocked: false,
    })));
    let mut batch = envelope_batch(
        (0..(FAILED_BATCH_RETAIN_LIMIT + 5))
            .map(|i| make_entry(&format!("locked row {i}")))
            .collect::<Vec<_>>(),
    );
    let mut storage_blocked = false;
    let mut summary = IngestSummary::default();
    let observability = Arc::new(crate::observability::RuntimeObservability::default());
    let context = WriterContext::new(
        Arc::clone(&pool),
        storage.clone(),
        Arc::clone(&storage_state),
        crate::receiver::enrichment::EnrichmentConfig::default(),
        Arc::new(crate::enrich::EnrichmentPipeline::new()),
        Arc::clone(&observability),
    );

    flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;

    assert_eq!(batch.len(), FAILED_BATCH_RETAIN_LIMIT);
    assert_eq!(observability.snapshot().writer_logs_retained, 1000);
    assert_eq!(observability.snapshot().writer_logs_discarded, 5);
    lock_conn.execute_batch("ROLLBACK;").unwrap();
}

#[test]
fn failed_batch_retains_disk_full_errors_instead_of_discarding_rows() {
    let (pool, _storage, _dir) = test_pool();
    let batch = envelope_batch(vec![make_entry("disk full retained")]);
    let error = anyhow::anyhow!(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
        Some("database or disk is full".to_string()),
    ));

    let outcome = handle_failed_batch(&pool, batch, &error);

    assert_eq!(outcome.retained_entries.len(), 1);
    assert_eq!(outcome.retained_chunks, 1);
    assert_eq!(outcome.discarded_count, 0);
}

#[test]
fn failed_batch_retains_pool_timeout_errors_instead_of_discarding_rows() {
    let (pool, _storage, _dir) = test_pool();
    // `StorageConfig::for_test` builds a single-connection pool, so holding
    // that one connection makes every further acquisition fail with a real
    // `r2d2::Error`. That error never carries a `rusqlite::Error` in its
    // chain — the batch never reached SQLite at all.
    let _held = pool.get().unwrap();
    let error = anyhow::Error::new(
        pool.get_timeout(std::time::Duration::from_millis(50))
            .expect_err("exhausted pool must time out"),
    );
    let batch = envelope_batch(vec![make_entry("pool timeout retained")]);

    let outcome = handle_failed_batch(&pool, batch, &error);

    assert_eq!(outcome.retained_entries.len(), 1);
    assert_eq!(outcome.retained_chunks, 1);
    assert_eq!(outcome.discarded_count, 0);
}

#[test]
fn ingest_summary_aggregates_cardinality_overflow() {
    let mut summary = IngestSummary::default();
    let entries = (0..(INGEST_SUMMARY_CARDINALITY_LIMIT + 5))
        .map(|i| {
            make_entry_for_sender(
                &format!("host-{i}"),
                &format!("192.0.2.{}:514", i % 255),
                &format!("message {i}"),
            )
        })
        .collect::<Vec<_>>();

    summary.record_batch(&entries);

    assert_eq!(summary.total_logs, INGEST_SUMMARY_CARDINALITY_LIMIT + 5);
    assert_eq!(summary.host_counts.len(), INGEST_SUMMARY_CARDINALITY_LIMIT);
    assert_eq!(summary.host_overflow_count, 5);
    assert!(!summary.host_counts.contains_key(OTHER_SUMMARY_LABEL));
    assert_eq!(summary.sender_overflow_count, 5);
    assert!(!summary.sender_counts.contains_key(&(
        OTHER_SUMMARY_LABEL.to_string(),
        OTHER_SUMMARY_LABEL.to_string()
    )));
}
#[test]
fn source_addr_ip_strips_socket_ports() {
    assert_eq!(source_addr_ip("198.51.100.3:49238"), "198.51.100.3");
    assert_eq!(
        source_addr_ip("[fd7a:115c:a1e0::4f32:104f]:1514"),
        "fd7a:115c:a1e0::4f32:104f"
    );
    assert_eq!(source_addr_ip("unknown-source"), "unknown-source");
}

#[test]
fn summarize_top_senders_pairs_hostnames_with_source_ips() {
    let counts = HashMap::from([
        (("devhost".to_string(), "172.19.0.1".to_string()), 29),
        (("edgehost".to_string(), "198.51.100.3".to_string()), 15),
        (("laptophost".to_string(), "198.51.100.5".to_string()), 28),
    ]);

    assert_eq!(
        summarize_top_senders(&counts, 0, 2),
        "devhost@172.19.0.1=29, laptophost@198.51.100.5=28"
    );
}

#[test]
fn summarize_top_senders_includes_other_bucket_deterministically() {
    let counts = HashMap::from([
        (("devhost".to_string(), "172.19.0.1".to_string()), 29),
        (("laptophost".to_string(), "198.51.100.5".to_string()), 28),
    ]);

    assert_eq!(
        summarize_top_senders(&counts, 31, 2),
        "__other__@__other__=31, devhost@172.19.0.1=29"
    );
}
