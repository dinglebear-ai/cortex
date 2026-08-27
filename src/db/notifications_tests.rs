#[cfg(test)]
mod notifications_db_tests {
    use rusqlite::Connection;

    use crate::db::notifications::{
        AttemptCount, FiringInsertParams, OutboxInsertParams, backoff_next_attempt_at,
        firings_insert, firings_recent, firings_recent_dedup_check, outbox_claim_pending,
        outbox_insert, outbox_mark_dead, outbox_mark_dropped, outbox_mark_sent,
        outbox_schedule_retry,
    };

    fn in_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE notifications_outbox (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 dedup_key TEXT NOT NULL,
                 rule_id TEXT NOT NULL,
                 severity TEXT NOT NULL,
                 hostname TEXT NOT NULL,
                 title TEXT NOT NULL,
                 body TEXT NOT NULL,
                 apprise_urls_json TEXT NOT NULL,
                 apprise_tags TEXT,
                 enqueued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 next_attempt_at TEXT NOT NULL,
                 attempt_count INTEGER NOT NULL DEFAULT 0,
                 last_status_code INTEGER,
                 last_error TEXT,
                 status TEXT NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending','sent','dead','dropped'))
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_dedup_pending
                 ON notifications_outbox(dedup_key) WHERE status = 'pending';
             CREATE TABLE notification_firings (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 outbox_id INTEGER NOT NULL,
                 rule_id TEXT NOT NULL,
                 severity TEXT NOT NULL,
                 hostname TEXT NOT NULL,
                 fired_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 status_code INTEGER,
                 notes TEXT,
                 dedup_key TEXT NOT NULL DEFAULT ''
             );",
        )
        .expect("schema");
        conn
    }

    fn make_params(dedup_key: &str) -> OutboxInsertParams {
        OutboxInsertParams {
            dedup_key: dedup_key.to_string(),
            rule_id: "oom_kill".to_string(),
            severity: "critical".to_string(),
            hostname: "host1".to_string(),
            title: "OOM Kill on host1".to_string(),
            body: "Process was killed".to_string(),
            apprise_urls_json: r#"["gotify://host/token"]"#.to_string(),
            next_attempt_at: "2030-01-01T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn outbox_insert_idempotent() {
        let conn = in_memory_conn();
        let params = make_params("dedup-1");

        // First insert should succeed
        outbox_insert(&conn, &params).expect("first insert");

        // Second insert with same dedup_key should be skipped (idempotent)
        outbox_insert(&conn, &params).expect("second insert (no-op)");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications_outbox WHERE dedup_key = ?1",
                rusqlite::params!["dedup-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "duplicate dedup_key should be suppressed");
    }

    #[test]
    fn outbox_insert_different_keys() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-a")).expect("insert a");
        outbox_insert(&conn, &make_params("key-b")).expect("insert b");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications_outbox", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn outbox_claim_pending_basic() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-c")).expect("insert");

        // Override next_attempt_at to past
        conn.execute(
            "UPDATE notifications_outbox SET next_attempt_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();

        let rows = outbox_claim_pending(&conn, 10).expect("claim");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rule_id, "oom_kill");
    }

    fn expire_next_attempt(conn: &Connection) {
        conn.execute(
            "UPDATE notifications_outbox SET next_attempt_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .expect("expire next_attempt_at");
    }

    #[test]
    fn outbox_claim_leases_rows_so_a_failed_write_back_cannot_redeliver() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-lease")).expect("insert");
        expire_next_attempt(&conn);

        let first = outbox_claim_pending(&conn, 10).expect("first claim");
        assert_eq!(first.len(), 1, "row should be claimable once");

        // Simulates the dispatcher delivering via Apprise and then losing the
        // outbox write-back to a pool timeout: status is still 'pending' and
        // no firing row exists, so dedup cannot suppress a redelivery.
        let second = outbox_claim_pending(&conn, 10).expect("second claim");
        assert!(
            second.is_empty(),
            "a claimed row must stay leased so the next cycle cannot redeliver it"
        );
    }

    #[test]
    fn outbox_claim_reclaims_rows_once_the_lease_expires() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-reclaim")).expect("insert");
        expire_next_attempt(&conn);

        let first = outbox_claim_pending(&conn, 10).expect("first claim");
        assert_eq!(
            first[0].attempt_count,
            AttemptCount::NONE,
            "first claim is attempt 0"
        );

        // Lease expiry: an abandoned in-flight row becomes claimable again so
        // a crash between delivery and write-back is not permanent loss.
        expire_next_attempt(&conn);
        let second = outbox_claim_pending(&conn, 10).expect("second claim");
        assert_eq!(second.len(), 1, "expired lease must be reclaimable");
        assert_eq!(
            second[0].attempt_count,
            AttemptCount::from_stored(1),
            "each claim consumes one attempt so abandoned rows still dead-letter"
        );

        let stored: i64 = conn
            .query_row(
                "SELECT attempt_count FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![second[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 2, "two claims must have consumed two attempts");
    }

    #[test]
    fn outbox_claim_keeps_row_pending_so_dedup_index_still_blocks_reenqueue() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-inflight")).expect("insert");
        expire_next_attempt(&conn);
        outbox_claim_pending(&conn, 10).expect("claim");

        // idx_outbox_dedup_pending is a UNIQUE partial index over
        // status = 'pending'; an in-flight row must keep blocking a
        // re-enqueue of the same dedup_key.
        outbox_insert(&conn, &make_params("key-inflight")).expect("re-insert is a no-op");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications_outbox WHERE dedup_key = ?1",
                rusqlite::params!["key-inflight"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "claimed row must still suppress a re-enqueue");
    }

    #[test]
    fn outbox_write_back_does_not_double_count_the_claimed_attempt() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-count")).expect("insert");
        expire_next_attempt(&conn);
        let rows = outbox_claim_pending(&conn, 10).expect("claim");
        let id = rows[0].id;

        outbox_schedule_retry(&conn, id, "2030-06-01T00:00:00.000Z", "timeout", Some(503))
            .expect("retry");

        let attempt_count: i64 = conn
            .query_row(
                "SELECT attempt_count FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            attempt_count, 1,
            "the claim consumed the attempt; the write-back must not count it again"
        );
    }

    #[test]
    fn outbox_mark_sent_and_dead() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-d")).expect("insert");
        conn.execute(
            "UPDATE notifications_outbox SET next_attempt_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();

        let rows = outbox_claim_pending(&conn, 10).expect("claim");
        let id = rows[0].id;

        outbox_mark_sent(&conn, id, Some(200)).expect("mark sent");

        let status: String = conn
            .query_row(
                "SELECT status FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "sent");
    }

    #[test]
    fn outbox_mark_dropped_test() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-e")).expect("insert");
        let id: i64 = conn
            .query_row("SELECT id FROM notifications_outbox LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        outbox_mark_dropped(&conn, id, "acked").expect("mark dropped");

        let status: String = conn
            .query_row(
                "SELECT status FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "dropped");
    }

    #[test]
    fn outbox_schedule_retry_test() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-f")).expect("insert");
        let id: i64 = conn
            .query_row("SELECT id FROM notifications_outbox LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();

        // Mirror the production cycle: a delivery attempt is always preceded by
        // a claim, and it is the claim that consumes the attempt. Scheduling a
        // retry only records the outcome, so it must not increment again.
        expire_next_attempt(&conn);
        let claimed = outbox_claim_pending(&conn, 10).expect("claim");
        assert_eq!(claimed.len(), 1, "the row should be claimable");

        outbox_schedule_retry(&conn, id, "2030-06-01T00:00:00.000Z", "timeout", Some(503))
            .expect("retry");

        let (attempt_count, last_error, next_attempt_at): (i64, String, String) = conn
            .query_row(
                "SELECT attempt_count, last_error, next_attempt_at
                   FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(attempt_count, 1);
        assert_eq!(last_error, "timeout");
        // The backoff deadline must replace the claim's 300s lease. If this
        // ever stopped being written, every transient retry would silently wait
        // out the lease instead of the 1s first tier, and nothing else would
        // catch it.
        assert_eq!(next_attempt_at, "2030-06-01T00:00:00.000Z");
    }

    #[test]
    fn outbox_mark_dead_test() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-g")).expect("insert");
        let id: i64 = conn
            .query_row("SELECT id FROM notifications_outbox LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        outbox_mark_dead(&conn, id, Some(500), "server error").expect("mark dead");

        let status: String = conn
            .query_row(
                "SELECT status FROM notifications_outbox WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "dead");
    }

    #[test]
    fn firings_insert_and_dedup_check() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-h")).expect("insert");
        let id: i64 = conn
            .query_row("SELECT id FROM notifications_outbox LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();

        firings_insert(
            &conn,
            FiringInsertParams {
                outbox_id: id,
                rule_id: "oom_kill",
                severity: "critical",
                hostname: "host1",
                status_code: Some(200),
                notes: None,
                dedup_key: "oom_kill:host1:key-h",
            },
        )
        .expect("firings insert");

        // Within window, same dedup_key -> should dedup
        let should_dedup =
            firings_recent_dedup_check(&conn, "oom_kill", "host1", "oom_kill:host1:key-h", 3600)
                .expect("dedup check");
        assert!(should_dedup, "should suppress within dedup window");

        // Different hostname -> no dedup
        let no_dedup =
            firings_recent_dedup_check(&conn, "oom_kill", "host2", "oom_kill:host1:key-h", 3600)
                .expect("dedup check 2");
        assert!(!no_dedup, "different host should not dedup");

        // Different dedup_key -> no dedup (this is the key fix: per-signature isolation)
        let no_dedup_dk = firings_recent_dedup_check(
            &conn,
            "oom_kill",
            "host1",
            "oom_kill:host1:other-key",
            3600,
        )
        .expect("dedup check 3");
        assert!(!no_dedup_dk, "different dedup_key should not dedup");
    }

    #[test]
    fn firings_recent_list() {
        let conn = in_memory_conn();
        outbox_insert(&conn, &make_params("key-i")).expect("insert");
        let id: i64 = conn
            .query_row("SELECT id FROM notifications_outbox LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        firings_insert(
            &conn,
            FiringInsertParams {
                outbox_id: id,
                rule_id: "oom_kill",
                severity: "critical",
                hostname: "host1",
                status_code: Some(200),
                notes: None,
                dedup_key: "key-oom",
            },
        )
        .unwrap();
        firings_insert(
            &conn,
            FiringInsertParams {
                outbox_id: id,
                rule_id: "fail2ban_ban",
                severity: "notice",
                hostname: "host2",
                status_code: Some(200),
                notes: None,
                dedup_key: "key-fail2ban",
            },
        )
        .unwrap();

        let all = firings_recent(&conn, 10, None, None).expect("all firings");
        assert_eq!(all.len(), 2);

        let filtered = firings_recent(&conn, 10, Some("oom_kill"), None).expect("filtered");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].rule_id, "oom_kill");
    }

    #[test]
    fn backoff_delays_are_increasing() {
        // Verify the specific backoff schedule: 1s, 5s, 30s, 5min, 30min cap.
        let expected_secs: &[u64] = &[1, 5, 30, 300, 1800, 1800, 1800, 1800];
        let now = chrono::Utc::now();
        for (i, &expected) in expected_secs.iter().enumerate() {
            let s = backoff_next_attempt_at(AttemptCount::from_stored(i as i64));
            let parsed = chrono::DateTime::parse_from_rfc3339(&s)
                .unwrap_or_else(|_| panic!("attempt {i}: invalid ISO8601: {s}"));
            let actual_delay = (parsed.with_timezone(&chrono::Utc) - now).num_seconds();
            // Allow ±2s tolerance for test execution time.
            assert!(
                (actual_delay - expected as i64).abs() <= 2,
                "attempt {i}: expected ~{expected}s delay, got {actual_delay}s"
            );
        }
    }

    #[test]
    fn attempt_count_saturates_instead_of_wrapping_past_u8_max() {
        // Release builds compile without overflow-checks, so a bare `+ 1` on a
        // clamped u8::MAX wraps to 0 and silently parks the row below every
        // dead-letter threshold forever. It must saturate instead.
        let maxed = AttemptCount::from_stored(i64::from(u8::MAX));
        assert_eq!(maxed.consumed(), u8::MAX);
        assert_eq!(maxed.tier(), u8::MAX);

        // A stored counter driven past the u8 domain clamps rather than
        // truncating into a small, below-threshold value.
        let overflowed = AttemptCount::from_stored(i64::from(u8::MAX) + 1);
        assert_eq!(overflowed, maxed);
        assert_eq!(AttemptCount::from_stored(i64::MAX), maxed);

        let default_max_retry_attempts: u8 = 8;
        assert!(
            maxed.consumed() >= default_max_retry_attempts,
            "a saturated row must still clear the dead-letter threshold"
        );
    }

    #[test]
    fn attempt_count_none_is_the_first_backoff_tier() {
        assert_eq!(AttemptCount::NONE.tier(), 0);
        assert_eq!(AttemptCount::NONE.consumed(), 1);
    }
}
