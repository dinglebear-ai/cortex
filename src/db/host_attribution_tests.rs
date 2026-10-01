use super::*;
use crate::config::StorageConfig;

fn fixture() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = super::super::init_pool(&StorageConfig::for_test(dir.path().join("attribution.db")))
        .unwrap();
    (pool, dir)
}

fn forwarded(host: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: "2026-09-30T00:00:00Z".into(),
        hostname: "agent-shared_bearer".into(),
        facility: None,
        severity: "info".into(),
        app_name: None,
        process_id: None,
        message: "forwarded evidence".into(),
        raw: "forwarded evidence".into(),
        source_ip: "agent-ai-transcript://100.120.242.29".into(),
        docker_checkpoint: None,
        ai_tool: Some("codex".into()),
        ai_project: Some("p".into()),
        ai_session_id: Some("s".into()),
        ai_transcript_path: None,
        metadata_json: Some(
            serde_json::json!({"provenance":{
                "authenticated_forwarder":"shared_bearer","transport_peer":"100.120.242.29",
                "hostname_claim":host,"trust":"claimed"
            }})
            .to_string(),
        ),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn future_projection_preserves_auth_identity_and_separates_devices() {
    let (pool, _dir) = fixture();
    let entries = [
        forwarded("tootie"),
        forwarded("macpoo"),
        forwarded("tootie"),
    ];
    super::super::insert_logs_batch(&pool, &entries).unwrap();
    let conn = pool.get().unwrap();
    let raw: Vec<String> = conn
        .prepare("SELECT hostname FROM logs ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(raw, vec!["agent-shared_bearer"; 3]);
    let counts = list_forwarded_host_counts(&conn).unwrap();
    assert_eq!(counts.len(), 2);
    assert_eq!(
        counts
            .iter()
            .find(|c| c.hostname == "tootie")
            .unwrap()
            .log_count,
        2
    );
    assert_eq!(
        counts
            .iter()
            .find(|c| c.hostname == "macpoo")
            .unwrap()
            .log_count,
        1
    );
    assert!(
        counts
            .iter()
            .all(|c| c.original_hostname == "agent-shared_bearer")
    );
    let mismatched_receipts:i64=conn.query_row(
        "SELECT COUNT(*) FROM forwarded_log_hosts f JOIN logs l ON l.id=f.log_id WHERE f.received_at<>l.received_at",[],|row|row.get(0)).unwrap();
    assert_eq!(mismatched_receipts, 0);
    let mismatched_host_clock: i64 = conn.query_row(
        "SELECT COUNT(*) FROM forwarded_host_counts c WHERE c.first_seen <> (SELECT MIN(received_at) FROM forwarded_log_hosts f WHERE f.hostname=c.hostname AND f.original_hostname=c.original_hostname) OR c.last_seen <> (SELECT MAX(received_at) FROM forwarded_log_hosts f WHERE f.hostname=c.hostname AND f.original_hostname=c.original_hostname)", [], |row| row.get(0)).unwrap();
    assert_eq!(
        mismatched_host_clock, 0,
        "device inventory uses server receipt time, not sender clock"
    );
    conn.execute("DELETE FROM logs WHERE id=(SELECT MIN(id) FROM logs)", [])
        .unwrap();
    assert_eq!(
        list_forwarded_host_counts(&conn)
            .unwrap()
            .iter()
            .find(|c| c.hostname == "tootie")
            .unwrap()
            .log_count,
        1
    );
    conn.execute("DELETE FROM logs", []).unwrap();
    assert!(list_forwarded_host_counts(&conn).unwrap().is_empty());
}

#[test]
fn rollback_discards_both_evidence_and_projection() {
    let (pool, _dir) = fixture();
    let mut conn = super::super::write_conn(&pool).unwrap();
    let tx = conn.transaction().unwrap();
    super::super::ingest::insert_logs_batch_in_tx(&tx, &[forwarded("tootie")]).unwrap();
    assert_eq!(list_forwarded_host_counts(&tx).unwrap()[0].log_count, 1);
    tx.rollback().unwrap();
    assert!(list_forwarded_host_counts(&conn).unwrap().is_empty());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn historical_batches_resume_without_double_counting_live_rows() {
    let (pool, _dir) = fixture();
    // Legacy rows bypass the new ingest projection, as they did before migration.
    let conn = pool.get().unwrap();
    let metadata = forwarded("tootie").metadata_json.unwrap();
    for (id, source) in [
        (1, "ordinary://log"),
        (2, "agent-ai-transcript://100.120.242.29"),
        (3, "ordinary://log"),
        (4, "agent-ai-transcript://100.120.242.29"),
    ] {
        conn.execute("INSERT INTO logs(id,timestamp,hostname,severity,message,raw,source_ip,metadata_json) VALUES(?1,'2026-09-30T00:00:00Z','agent-shared_bearer','info','legacy','legacy',?2,?3)",params![id,source,metadata]).unwrap();
    }
    conn.execute("DELETE FROM forwarded_raw_host_counts", [])
        .unwrap();
    conn.execute("UPDATE forwarded_host_backfill SET high_water=4", [])
        .unwrap();
    drop(conn);
    let first = backfill_batch(&pool, 2).unwrap();
    assert_eq!(first.scanned, 2);
    assert_eq!(first.attributed, 1);
    assert_eq!(first.cursor, 2);
    assert!(!first.complete);
    // Future ingress is already projected and lies beyond the frozen high water.
    super::super::insert_logs_batch(&pool, &[forwarded("macpoo")]).unwrap();
    let second = backfill_batch(&pool, 2).unwrap();
    assert_eq!(second.cursor, 4);
    assert_eq!(second.attributed, 1);
    let terminal = backfill_batch(&pool, 2).unwrap();
    assert!(terminal.complete);
    assert_eq!(terminal.high_water, 4);
    let conn = pool.get().unwrap();
    let counts = list_forwarded_host_counts(&conn).unwrap();
    assert_eq!(counts.iter().map(|c| c.log_count).sum::<i64>(), 3);
    // Replay is idempotent, including a snapshot that overlaps live projection.
    conn.execute("DELETE FROM forwarded_raw_host_counts", [])
        .unwrap();
    conn.execute(
        "UPDATE forwarded_host_backfill SET cursor=0,high_water=5,complete=0",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(backfill_batch(&pool, 1000).unwrap().attributed, 0);
    assert_eq!(
        list_forwarded_host_counts(&pool.get().unwrap())
            .unwrap()
            .iter()
            .map(|c| c.log_count)
            .sum::<i64>(),
        3
    );
}

#[test]
fn unproved_and_oversized_metadata_remain_unattributed() {
    let (pool, _dir) = fixture();
    let mut unknown = forwarded("unknown");
    let mut invalid = forwarded("macpoo");
    invalid.source_ip = "agent-ai-transcript://100.0.0.9".into();
    unknown.metadata_json = Some("x".repeat(65537));
    super::super::insert_logs_batch(&pool, &[unknown, invalid]).unwrap();
    assert!(
        list_forwarded_host_counts(&pool.get().unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn deletion_lineage_and_device_names_survive_fk_cleanup_modes() {
    for foreign_keys in [false, true] {
        let (pool, _dir) = fixture();
        super::super::insert_logs_batch(&pool, &[forwarded("tootie"), forwarded("macpoo")])
            .unwrap();
        let conn = pool.get().unwrap();
        conn.pragma_update(None, "foreign_keys", foreign_keys)
            .unwrap();
        conn.execute("DELETE FROM logs", []).unwrap();
        assert!(list_forwarded_host_counts(&conn).unwrap().is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM forwarded_log_hosts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let device_lineage: Vec<String> = conn
            .prepare("SELECT hostname FROM forwarded_deleted_log_hosts ORDER BY log_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(device_lineage, vec!["tootie", "macpoo"]);
        let raw_lineage: Vec<String> = conn
            .prepare("SELECT hostname FROM stream_deleted_log_lineage ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(raw_lineage, vec!["agent-shared_bearer"; 2]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM forwarded_host_names", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        // Existing stream-lineage pruning also removes its additive device lane.
        conn.execute("UPDATE stream_deleted_log_lineage SET deleted_at=0", [])
            .unwrap();
        drop(conn);
        assert_eq!(
            super::super::prune_expired_stream_lineage(&pool).unwrap(),
            2
        );
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM forwarded_deleted_log_hosts",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM forwarded_host_names", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

fn reset_backfill_snapshot(conn: &Connection) {
    conn.execute_batch(
        "DELETE FROM forwarded_raw_host_counts;
         UPDATE forwarded_host_backfill SET cursor=0,
             high_water=(SELECT COALESCE(MAX(id),0) FROM logs),complete=0;",
    )
    .unwrap();
}

#[test]
fn bounded_backfill_repairs_inflated_and_phantom_raw_host_counts() {
    let (pool, _dir) = fixture();
    let conn = pool.get().unwrap();
    for (id, host, receipt) in [
        (1, "tootie", "2026-01-01T00:00:00Z"),
        (2, "macpoo", "2026-03-01T00:00:00Z"),
        (3, "unknown", "2026-02-01T00:00:00Z"),
    ] {
        conn.execute(
            "INSERT INTO logs(id,timestamp,hostname,severity,message,raw,source_ip,metadata_json,received_at)
             VALUES(?1,'2026-09-30T00:00:00Z','agent-shared_bearer','info','legacy','legacy',?2,?3,?4)",
            params![id,forwarded(host).source_ip,forwarded(host).metadata_json,receipt],
        ).unwrap();
    }
    conn.execute_batch(
        "INSERT INTO hosts(hostname,first_seen,last_seen,log_count)
         VALUES('agent-shared_bearer','obsolete','obsolete',1000),
               ('phantom','obsolete','obsolete',999);",
    )
    .unwrap();
    reset_backfill_snapshot(&conn);
    drop(conn);
    let first = backfill_batch(&pool, 2).unwrap();
    assert_eq!(first.scanned, 2);
    assert!(!first.complete);
    let terminal = backfill_batch(&pool, 2).unwrap();
    assert_eq!(terminal.scanned, 1);
    assert!(terminal.complete);
    let conn = pool.get().unwrap();
    let retained: (i64, String, String) = conn
        .query_row(
            "SELECT log_count,first_seen,last_seen FROM hosts WHERE hostname='agent-shared_bearer'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        retained,
        (
            3,
            "2026-01-01T00:00:00Z".into(),
            "2026-03-01T00:00:00Z".into()
        )
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM hosts WHERE hostname='phantom'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let attributed = list_forwarded_host_counts(&conn).unwrap();
    assert_eq!(attributed.iter().map(|c| c.log_count).sum::<i64>(), 2);
    assert_eq!(
        retained.0 - attributed.iter().map(|c| c.log_count).sum::<i64>(),
        1,
        "unproved retained evidence remains in its raw principal bucket"
    );
    drop(conn);
    // Completion disables repair accounting. Repeated worker starts cannot
    // overwrite the regular ingest counter with the completed snapshot.
    super::super::insert_logs_batch(&pool, &[forwarded("tootie")]).unwrap();
    assert!(backfill_batch(&pool, 1).unwrap().complete);
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT log_count FROM hosts WHERE hostname='agent-shared_bearer'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4
    );
    assert_eq!(
        conn.query_row(
            "SELECT log_count FROM forwarded_raw_host_counts WHERE hostname='agent-shared_bearer'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
}

#[test]
fn repair_accounts_concurrent_mutations_and_resumes_after_pool_restart() {
    let (pool, dir) = fixture();
    super::super::insert_logs_batch(
        &pool,
        &[
            forwarded("tootie"),
            forwarded("macpoo"),
            forwarded("tootie"),
            forwarded("macpoo"),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    reset_backfill_snapshot(&conn);
    drop(conn);
    assert_eq!(backfill_batch(&pool, 2).unwrap().cursor, 2);
    let conn = pool.get().unwrap();
    // Passed IDs already contributed; unvisited IDs have not contributed yet.
    conn.execute("DELETE FROM logs WHERE id IN(1,4)", [])
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT log_count FROM forwarded_raw_host_counts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(conn);
    super::super::insert_logs_batch(&pool, &[forwarded("macpoo")]).unwrap();
    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM logs WHERE id=5", []).unwrap();
    drop(conn);
    super::super::insert_logs_batch(&pool, &[forwarded("tootie")]).unwrap();
    let conn = pool.get().unwrap();
    // Normal IDs are AUTOINCREMENT, but imported explicit ID reuse behind the
    // cursor must also be accounted without waiting for an impossible rescan.
    conn.execute("INSERT INTO logs(id,timestamp,hostname,severity,message,raw,source_ip)
        VALUES(1,'2026-09-30T00:00:00Z','agent-shared_bearer','info','reused','reused','ordinary://log')", []).unwrap();
    assert_eq!(
        conn.query_row("SELECT log_count FROM forwarded_raw_host_counts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(conn);
    drop(pool);
    let pool = super::super::init_pool(&StorageConfig::for_test(dir.path().join("attribution.db")))
        .unwrap();
    let terminal = backfill_batch(&pool, 2).unwrap();
    assert_eq!(terminal.scanned, 1);
    assert_eq!(terminal.high_water, 4);
    assert!(terminal.complete);
    let conn = pool.get().unwrap();
    let actual: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE hostname='agent-shared_bearer'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(actual, 4);
    assert_eq!(
        conn.query_row(
            "SELECT log_count FROM hosts WHERE hostname='agent-shared_bearer'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        actual
    );
    assert_eq!(
        list_forwarded_host_counts(&conn)
            .unwrap()
            .iter()
            .map(|c| c.log_count)
            .sum::<i64>(),
        3
    );
    drop(conn);
    assert_eq!(backfill_batch(&pool, 2).unwrap().scanned, 0);
}

#[test]
fn sparse_historical_schema_defers_lineage_expiry_until_dependency_exists() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE logs(id INTEGER PRIMARY KEY,hostname TEXT);")
        .unwrap();
    install_schema(&conn).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='forwarded_lineage_expiry'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("CREATE TABLE stream_deleted_log_lineage(id INTEGER PRIMARY KEY);")
        .unwrap();
    install_lineage_expiry_trigger(&conn).unwrap();
    install_lineage_expiry_trigger(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO stream_deleted_log_lineage VALUES(1);
        INSERT INTO forwarded_deleted_log_hosts VALUES(1,'tootie',unixepoch());
        DELETE FROM stream_deleted_log_lineage WHERE id=1;",
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM forwarded_deleted_log_hosts",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
