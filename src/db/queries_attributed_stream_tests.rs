use super::*;
use crate::config::StorageConfig;

fn fixture() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&StorageConfig::for_test(
        dir.path().join("device-stream.db"),
    ))
    .unwrap();
    (pool, dir)
}

fn forwarded(host: &str, second: u32) -> crate::db::LogBatchEntry {
    crate::db::LogBatchEntry {
        timestamp: format!("2026-09-30T12:00:{second:02}Z"),
        hostname: "agent-shared_bearer".into(),
        facility: None,
        severity: "info".into(),
        app_name: Some("collector-test".into()),
        process_id: None,
        message: format!("evidence from {host}"),
        raw: format!("evidence from {host}"),
        source_ip: "agent-ai-transcript://192.0.2.8".into(),
        docker_checkpoint: None,
        ai_tool: Some("codex".into()),
        ai_project: Some("project".into()),
        ai_session_id: Some("reused-session".into()),
        ai_transcript_path: None,
        metadata_json: Some(
            serde_json::json!({"provenance":{
                "authenticated_forwarder":"shared_bearer","transport_peer":"192.0.2.8",
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

fn params(host: &str) -> DurableStreamParams {
    DurableStreamParams {
        hostname: Some(host.into()),
        limit: 100,
        include_bounds: true,
        ..Default::default()
    }
}

fn ids(page: &DurableStreamPage) -> Vec<i64> {
    page.rows.iter().map(|row| row.id).collect()
}

#[test]
fn mixed_shared_principal_and_peer_keep_device_streams_separate() {
    let (pool, _dir) = fixture();
    crate::db::insert_logs_batch(
        &pool,
        &[
            forwarded("workstation", 1),
            forwarded("serverhost", 2),
            forwarded("workstation", 3),
            forwarded("unknown", 4),
        ],
    )
    .unwrap();
    let mac = crate::db::durable_stream_page(&pool, &params("WORKSTATION")).unwrap();
    assert_eq!(ids(&mac), vec![1, 3]);
    assert_eq!(mac.minimum_watermark, Some(1));
    assert_eq!(mac.high_watermark, 3);
    assert!(mac.rows.iter().all(|row| row.hostname == "workstation"));
    assert!(mac.rows.iter().all(|row| {
        row.metadata_json
            .as_deref()
            .unwrap()
            .contains("shared_bearer")
    }));
    let serverhost = crate::db::durable_stream_page(&pool, &params("serverhost")).unwrap();
    assert_eq!(ids(&serverhost), vec![2]);
    assert_eq!(serverhost.minimum_watermark, Some(2));
    assert_eq!(serverhost.high_watermark, 2);
    assert!(
        serverhost
            .rows
            .iter()
            .all(|row| row.hostname == "serverhost")
    );
}

#[test]
fn snapshot_pagination_merges_direct_and_projected_rows_without_new_arrivals() {
    let (pool, _dir) = fixture();
    let mut direct = forwarded("workstation", 4);
    direct.hostname = "workstation".into();
    direct.source_ip = "direct://log".into();
    direct.metadata_json = None;
    crate::db::insert_logs_batch(
        &pool,
        &[
            forwarded("workstation", 1),
            forwarded("serverhost", 2),
            forwarded("workstation", 3),
            direct,
        ],
    )
    .unwrap();
    let first = crate::db::durable_stream_page(
        &pool,
        &DurableStreamParams {
            limit: 1,
            ..params("workstation")
        },
    )
    .unwrap();
    assert_eq!(ids(&first), vec![1]);
    assert_eq!(first.high_watermark, 4);
    crate::db::insert_logs_batch(&pool, &[forwarded("workstation", 5)]).unwrap();
    let next = crate::db::durable_stream_page(
        &pool,
        &DurableStreamParams {
            after_id: 1,
            high_watermark: Some(first.high_watermark),
            limit: 2,
            include_bounds: false,
            ..params("workstation")
        },
    )
    .unwrap();
    assert_eq!(ids(&next), vec![3, 4]);
    assert_eq!(next.high_watermark, 4);
    assert_eq!(next.minimum_watermark, None);
    let finished = crate::db::durable_stream_page(
        &pool,
        &DurableStreamParams {
            after_id: 4,
            high_watermark: Some(4),
            include_bounds: false,
            ..params("workstation")
        },
    )
    .unwrap();
    assert!(finished.rows.is_empty());
    let follow = crate::db::durable_stream_page(
        &pool,
        &DurableStreamParams {
            after_id: 4,
            include_bounds: false,
            ..params("workstation")
        },
    )
    .unwrap();
    assert_eq!(ids(&follow), vec![5]);
}

#[test]
fn device_stream_retention_floor_survives_deletion_of_its_last_live_row() {
    let (pool, _dir) = fixture();
    crate::db::insert_logs_batch(
        &pool,
        &[
            forwarded("workstation", 1),
            forwarded("serverhost", 2),
            forwarded("workstation", 3),
        ],
    )
    .unwrap();
    pool.get()
        .unwrap()
        .execute("DELETE FROM logs WHERE id=1", [])
        .unwrap();
    let retained = crate::db::durable_stream_page(&pool, &params("workstation")).unwrap();
    assert_eq!(ids(&retained), vec![3]);
    assert_eq!(retained.minimum_watermark, Some(2));
    assert_eq!(retained.high_watermark, 3);
    pool.get()
        .unwrap()
        .execute("DELETE FROM logs WHERE id=3", [])
        .unwrap();
    let deleted = crate::db::durable_stream_page(&pool, &params("workstation")).unwrap();
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(4));
    assert_eq!(deleted.high_watermark, 3);
    let names: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM forwarded_host_names WHERE hostname='workstation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(names, 1);
    let unrelated = crate::db::durable_stream_page(&pool, &params("serverhost")).unwrap();
    assert_eq!(ids(&unrelated), vec![2]);
    assert_eq!(unrelated.minimum_watermark, Some(2));
    assert_eq!(unrelated.high_watermark, 2);
}

#[test]
fn fully_qualified_session_stream_keeps_raw_identity_and_retention_bounds() {
    let (pool, _dir) = fixture();
    crate::db::insert_logs_batch(
        &pool,
        &[forwarded("workstation", 1), forwarded("serverhost", 2)],
    )
    .unwrap();
    let scope = DurableStreamParams {
        hostname: Some("agent-shared_bearer".into()),
        ai_project: Some("project".into()),
        ai_tool: Some("codex".into()),
        ai_session_id: Some("reused-session".into()),
        limit: 100,
        include_bounds: true,
        ..Default::default()
    };
    let raw = crate::db::durable_stream_page(&pool, &scope).unwrap();
    assert_eq!(ids(&raw), vec![1, 2]);
    assert!(
        raw.rows
            .iter()
            .all(|row| row.hostname == "agent-shared_bearer")
    );
    assert_eq!(raw.high_watermark, 2);
    let physical_scope = DurableStreamParams {
        hostname: Some("workstation".into()),
        ..scope.clone()
    };
    assert!(
        crate::db::durable_stream_page(&pool, &physical_scope)
            .unwrap()
            .rows
            .is_empty()
    );
    pool.get().unwrap().execute("DELETE FROM logs", []).unwrap();
    let deleted = crate::db::durable_stream_page(&pool, &scope).unwrap();
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(3));
    assert_eq!(deleted.high_watermark, 2);
}

#[test]
fn device_stream_bounds_respect_app_and_severity_after_retention() {
    let (pool, _dir) = fixture();
    let mut first = forwarded("workstation", 1);
    first.severity = "err".into();
    let mut other_app = forwarded("workstation", 3);
    other_app.severity = "err".into();
    other_app.app_name = Some("other-app".into());
    crate::db::insert_logs_batch(&pool, &[first, forwarded("workstation", 2), other_app]).unwrap();
    let filtered = DurableStreamParams {
        app_name: Some("collector-test".into()),
        severity: Some("err".into()),
        ..params("workstation")
    };
    let first = crate::db::durable_stream_page(&pool, &filtered).unwrap();
    assert_eq!(ids(&first), vec![1]);
    assert_eq!(first.high_watermark, 1);
    pool.get().unwrap().execute("DELETE FROM logs", []).unwrap();
    let deleted = crate::db::durable_stream_page(&pool, &filtered).unwrap();
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(2));
    assert_eq!(deleted.high_watermark, 1);
}

#[test]
fn attributed_stream_pages_and_bounds_use_keyset_indexes() {
    let (pool, _dir) = fixture();
    let conn = pool.get().unwrap();
    let request = params("workstation");
    for bound in [
        None,
        Some(("retained", true)),
        Some(("retained", false)),
        Some(("deleted", false)),
    ] {
        let (sql, values) = statement(&request, "workstation", true, bound);
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        let index = if bound.is_some_and(|(mode, _)| mode == "deleted") {
            "idx_forwarded_deleted_hosts_host_id"
        } else {
            "idx_forwarded_log_hosts_host_id"
        };
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
        assert!(!plan.contains("SCAN l"), "{plan}");
    }
}
