// scanner_tests.rs
use super::{NORMALIZER_VERSION, process_chunk};
use crate::config::{ErrorDetectionConfig, StorageConfig};
use crate::db::{self, DbPool};
use tempfile::TempDir;

fn test_pool() -> (DbPool, TempDir) {
    let dir = TempDir::new().unwrap();
    let storage = StorageConfig {
        db_path: dir.path().join("test.db"),
        pool_size: 1,
        wal_mode: false,
        ..Default::default()
    };
    let pool = db::init_pool(&storage).unwrap();
    (pool, dir)
}

/// Insert a log row with the current timestamp (so it falls in the 1-hour window).
fn insert_log(conn: &rusqlite::Connection, message: &str, severity: &str, hostname: &str) -> i64 {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, received_at)
         VALUES (?4, ?1, ?2, ?3, ?3, ?4)",
        rusqlite::params![hostname, severity, message, now],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn test_process_chunk_skips_below_cursor() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Insert 3 log rows with severity that gets picked up by scanner
        insert_log(&conn, "error occurred alpha", "err", "host1");
        insert_log(&conn, "error occurred beta", "err", "host1");
        insert_log(&conn, "error occurred gamma", "err", "host1");
    }

    // Set cursor to 2: only row with id > 2 should be processed
    let result = process_chunk(&pool, 2, 200, 1, &[], u8::MAX).unwrap();

    // Only 1 row (id=3) should be returned
    assert_eq!(
        result.rows_in_chunk, 1,
        "only rows with id > cursor should be processed"
    );
    assert_eq!(result.new_cursor, 3);
}

#[test]
fn test_process_chunk_does_not_notify_below_threshold() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Insert fewer rows than the frequency_threshold (threshold=5, insert 3)
        insert_log(&conn, "disk full on /var", "err", "host1");
        insert_log(&conn, "disk full on /var", "err", "host1");
        insert_log(&conn, "disk full on /var", "err", "host1");
    }

    // frequency_threshold=5; only 3 rows inserted
    let result = process_chunk(&pool, 0, 200, 5, &[], u8::MAX).unwrap();
    assert_eq!(result.rows_in_chunk, 3);

    // No outbox row should have been inserted
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM notifications_outbox", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(count, 0, "no outbox notification below frequency_threshold");
}

#[test]
fn test_process_chunk_notifies_above_threshold() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Insert frequency_threshold+1 rows with the SAME message pattern
        // so they all land in the same signature group
        for _ in 0..6 {
            insert_log(
                &conn,
                "connection refused to 127.0.0.1:5432",
                "err",
                "host1",
            );
        }
    }

    // threshold=5; 6 rows inserted → should notify
    let result = process_chunk(&pool, 0, 200, 5, &[], u8::MAX).unwrap();
    assert_eq!(result.rows_in_chunk, 6);

    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "one outbox notification above frequency_threshold"
    );
}

#[test]
fn test_default_excludes_do_not_drop_warning_healthcheck_rows() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        for _ in 0..6 {
            insert_log(
                &conn,
                "GET request for '/' received from 127.0.0.1 using 'curl/8.14.1'",
                "warning",
                "edgehost",
            );
            insert_log(
                &conn,
                "Permission denied reading /home/jmagar/.claude/projects/session.jsonl",
                "warning",
                "edgehost",
            );
        }
    }

    let cfg = ErrorDetectionConfig::default();
    process_chunk(&pool, 0, 200, 5, &cfg.exclude_patterns, u8::MAX).unwrap();

    let conn = pool.get().unwrap();
    let sigs: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT sample_message FROM error_signatures ORDER BY sample_message ASC")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(sigs.len(), 2);
    assert!(
        sigs.iter()
            .any(|sample| sample.contains("GET request for '/'"))
    );
    assert!(
        sigs.iter()
            .any(|sample| sample.contains("Permission denied"))
    );
}

#[test]
fn test_severity_floor_suppresses_warning_but_keeps_err() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Two distinct recurring signatures, both above the frequency threshold:
        // one warning-level, one err-level.
        for _ in 0..6 {
            insert_log(
                &conn,
                "slow query took 900ms on widgets",
                "warning",
                "host1",
            );
            insert_log(
                &conn,
                "connection refused to 127.0.0.1:5432",
                "err",
                "host1",
            );
        }
    }

    // Floor = err (rank 3): the warning signature must NOT page, the err one must.
    let err_floor = crate::db::severity_to_num("err").unwrap();
    let result = process_chunk(&pool, 0, 400, 5, &[], err_floor).unwrap();
    assert_eq!(result.rows_in_chunk, 12);

    let conn = pool.get().unwrap();
    // Both signatures are recorded (searchable), regardless of the notify floor.
    let sigs: i64 = conn
        .query_row("SELECT COUNT(*) FROM error_signatures", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sigs, 2, "both warning and err signatures are recorded");

    // But only the err-level signature fired a notification.
    let notifs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(notifs, 1, "only the err+ signature pages under the floor");
    let notif_sev: String = conn
        .query_row(
            "SELECT severity FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(notif_sev, "err");
}

#[test]
fn test_severity_floor_admits_signature_more_severe_than_floor() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // crit is *more* severe than the err floor — the `rank <= floor` gate
        // must let it page (guards against inverting the comparison).
        for _ in 0..6 {
            insert_log(&conn, "kernel panic on cpu 3", "crit", "host1");
        }
    }
    let err_floor = crate::db::severity_to_num("err").unwrap();
    process_chunk(&pool, 0, 200, 5, &[], err_floor).unwrap();

    let conn = pool.get().unwrap();
    let notifs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(notifs, 1, "crit is above the err floor and must page");
}

#[test]
fn test_severity_floor_at_warning_pages_warnings() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        for _ in 0..6 {
            insert_log(
                &conn,
                "slow query took 900ms on widgets",
                "warning",
                "host1",
            );
        }
    }
    // A warning floor (the least-severe selectable level) admits warning-level
    // signatures — the floor is configurable downward, not hard-wired to err.
    let warn_floor = crate::db::severity_to_num("warning").unwrap();
    process_chunk(&pool, 0, 200, 5, &[], warn_floor).unwrap();

    let conn = pool.get().unwrap();
    let notifs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(notifs, 1, "a warning floor lets warning signatures page");
}

#[test]
fn test_process_chunk_skips_excluded_patterns() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Notification-delivery log lines (the feedback loop): well above the
        // frequency threshold, but they must be skipped entirely.
        for _ in 0..10 {
            insert_log(
                &conn,
                "apprise: Sent Gotify notification",
                "warning",
                "nashost",
            );
        }
        // A genuine error that must still be detected and counted.
        for _ in 0..6 {
            insert_log(&conn, "disk full on /var", "err", "host1");
        }
    }

    let exclude = vec!["Sent Gotify notification".to_string()];
    let result = process_chunk(&pool, 0, 200, 5, &exclude, u8::MAX).unwrap();
    // All 16 rows advance the cursor (skipped rows are not reprocessed).
    assert_eq!(result.rows_in_chunk, 16);

    let conn = pool.get().unwrap();
    // The excluded pattern must not be tracked as a signature at all.
    let excluded_sig: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM error_signatures WHERE template LIKE '%notification%'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(excluded_sig, 0, "excluded rows must not become signatures");

    // Exactly one notification — for the real error, not the delivery logs.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(count, 1, "only the genuine error notifies; loop is broken");
}

#[test]
fn test_process_chunk_suppressed_when_acked() {
    let (pool, _dir) = test_pool();
    {
        let conn = pool.get().unwrap();
        // Insert enough rows to trigger notification
        for _ in 0..6 {
            insert_log(&conn, "oom: process killed repeatedly", "err", "host1");
        }
    }

    // First pass: process the chunk so the signature is written
    let result = process_chunk(&pool, 0, 200, 5, &[], u8::MAX).unwrap();
    assert_eq!(result.rows_in_chunk, 6);

    // Manually acknowledge the signature
    {
        let conn = pool.get().unwrap();
        // Find the signature hash that was written
        let hash: String = conn
            .query_row(
                "SELECT signature_hash FROM error_signatures LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap();

        // Clear previous outbox entries so we can check fresh state
        conn.execute("DELETE FROM notifications_outbox", [])
            .unwrap();

        // Set acknowledged_at on the signature
        conn.execute(
            "UPDATE error_signatures SET acknowledged_at = '2026-01-01T00:00:00.000Z',
             acknowledged_by = 'test'
             WHERE signature_hash = ?1 AND normalizer_version = ?2",
            rusqlite::params![hash, NORMALIZER_VERSION],
        )
        .unwrap();
    }

    // Insert more rows so there is something to scan past the previous cursor
    {
        let conn = pool.get().unwrap();
        for _ in 0..3 {
            insert_log(&conn, "oom: process killed repeatedly", "err", "host1");
        }
    }

    // Second pass: with cursor advanced past original rows
    let cursor = result.new_cursor;
    let result2 = process_chunk(&pool, cursor, 200, 5, &[], u8::MAX).unwrap();
    assert_eq!(result2.rows_in_chunk, 3);

    // Outbox should still be empty because the signature is acked
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications_outbox WHERE rule_id = 'unaddressed_error_signature'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(count, 0, "no notification when signature is acknowledged");
}
