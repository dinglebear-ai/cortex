use super::*;
use std::sync::Arc;

#[test]
fn records_reject_unsafe_or_unbounded_inputs() {
    let valid = SyslogForwardRecord {
        source_instance: "host-a".into(),
        source_epoch: 1,
        sequence: 1,
        idempotency_key: "key".into(),
        observed_at: "2026-01-01T00:00:00Z".into(),
        line: "<134>1 ts host app - - - message".into(),
    };
    assert!(!invalid_record(&valid));
    assert!(invalid_record(&SyslogForwardRecord {
        line: String::new(),
        ..valid
    }));
}

#[test]
fn gap_rejects_reversed_window() {
    assert!(invalid_gap(&SyslogForwardGap {
        source_instance: "host-a".into(),
        source_epoch: 1,
        from_sequence: 2,
        to_sequence: 1,
        idempotency_key: "gap".into(),
        observed_at: "2026-01-01T00:00:00Z".into(),
        reason_code: "local_retention_quota".into()
    }));
}

#[test]
fn duplicate_replay_returns_receipt_without_duplicate_canonical_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&crate::config::StorageConfig::for_test(
            dir.path().join("syslog-forward.db"),
        ))
        .unwrap(),
    );
    let request = SyslogForwardRequest {
        records: vec![SyslogForwardRecord {
            source_instance: "host-a".into(),
            source_epoch: 1,
            sequence: 1,
            idempotency_key: "host-a:1:1:record".into(),
            observed_at: "2026-01-01T00:00:00Z".into(),
            line: "<134>1 2026-01-01T00:00:00Z host-a app - - - replay-safe".into(),
        }],
        gaps: vec![],
    };
    let first = persist_request(&pool, request.clone(), "127.0.0.1").unwrap();
    let replay = persist_request(&pool, request, "127.0.0.1").unwrap();
    assert_eq!(first, replay);
    let conn = pool.get().unwrap();
    let logs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE message = 'replay-safe'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let receipts: i64 = conn
        .query_row("SELECT COUNT(*) FROM syslog_forward_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(logs, 1);
    assert_eq!(receipts, 1);
}

#[test]
fn receiver_never_persists_a_path_like_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&crate::config::StorageConfig::for_test(
            dir.path().join("syslog-forward-private.db"),
        ))
        .unwrap(),
    );
    let source = "/Users/jmagar/private/production/access.log";
    persist_request(
        &pool,
        SyslogForwardRequest {
            records: vec![SyslogForwardRecord {
                source_instance: source.into(),
                source_epoch: 1,
                sequence: 1,
                idempotency_key: format!("{source}:1"),
                observed_at: "2026-01-01T00:00:00Z".into(),
                line: "<134>1 2026-01-01T00:00:00Z host-a app - - - safe".into(),
            }],
            gaps: vec![],
        },
        "127.0.0.1",
    )
    .unwrap();
    let conn = pool.get().unwrap();
    let receipt_source: String = conn
        .query_row(
            "SELECT source_instance FROM syslog_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let receipt_key: String = conn
        .query_row(
            "SELECT idempotency_key FROM syslog_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!receipt_source.contains(source));
    assert!(!receipt_key.contains(source));
}
