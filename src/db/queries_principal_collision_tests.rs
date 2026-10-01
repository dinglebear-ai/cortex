use super::*;
use crate::db::{DurableStreamParams, analytics};

fn entry(principal: &str, claim: &str, second: u32) -> LogBatchEntry {
    let mut row = named_entry(claim, second);
    row.hostname = format!("agent-{principal}");
    row.ai_tool = Some("codex".into());
    row.ai_project = Some("collision-project".into());
    row.ai_session_id = Some("collision-session".into());
    let mut metadata: serde_json::Value =
        serde_json::from_str(row.metadata_json.as_deref().unwrap()).unwrap();
    metadata["forwarded_provenance"]["authenticated_forwarder"] = principal.into();
    row.metadata_json = Some(metadata.to_string());
    row
}

fn fixture() -> (DbPool, tempfile::TempDir) {
    let (pool, dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            // The conflicting claim precedes proof of the other principal.
            // Resolution must be independent of ingest/backfill ordering.
            entry("collector-b", "agent-collector-a", 1),
            entry("collector-a", "device-a", 2),
            entry("collector-b", "AGENT-COLLECTOR-A", 3),
            entry("collector-a", "device-a", 4),
            entry("collector-b", "agent-collector-a.", 5),
            entry("collector-b", "device-b", 6),
        ],
    )
    .unwrap();
    (pool, dir)
}

fn stream(pool: &DbPool, hostname: Option<&str>) -> crate::db::DurableStreamPage {
    crate::db::durable_stream_page(
        pool,
        &DurableStreamParams {
            hostname: hostname.map(str::to_owned),
            limit: 20,
            include_bounds: true,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn cross_principal_claims_remain_evidence_without_becoming_device_selectors() {
    let (pool, _dir) = fixture();
    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 4);
    for (name, count, kind) in [
        ("agent-collector-a", 2, HostSourceKind::ForwardingPrincipal),
        ("agent-collector-b", 4, HostSourceKind::ForwardingPrincipal),
        ("device-a", 2, HostSourceKind::ClaimedHost),
        ("device-b", 1, HostSourceKind::ClaimedHost),
    ] {
        let row = hosts.iter().find(|row| row.hostname == name).unwrap();
        assert_eq!(row.log_count, count);
        assert_eq!(row.source_kind, kind);
    }
    for (name, ids) in [
        ("agent-collector-a", vec![4, 2]),
        ("agent-collector-b", vec![6, 5, 3, 1]),
        ("device-a", vec![4, 2]),
        ("AGENT-COLLECTOR-A", vec![]),
        ("agent-collector-a.", vec![]),
    ] {
        let tail = tail_logs(&pool, Some(name), None, None, None, 20).unwrap();
        assert_eq!(tail.iter().map(|row| row.id).collect::<Vec<_>>(), ids);
        for query in [None, Some("forwarded".to_string())] {
            // An app predicate exercises common append_host_selector rather
            // than only the bounded per-host search path.
            let rows = search_logs(
                &pool,
                &SearchParams {
                    host: Some(name.into()),
                    app: Some("collector-test".into()),
                    query,
                    limit: Some(20),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), ids);
        }
        let page = stream(&pool, Some(name));
        assert_eq!(
            page.rows.iter().rev().map(|row| row.id).collect::<Vec<_>>(),
            ids
        );
        let (feed, _, _) = analytics::feed_logs(&pool, Some(0), Some(name), 20).unwrap();
        assert_eq!(feed.iter().rev().map(|row| row.id).collect::<Vec<_>>(), ids);
        let (context, _) = analytics::context_around(
            &pool,
            &analytics::ContextRef {
                id: None,
                hostname: name.into(),
                timestamp: "2026-09-30T12:01:00Z".into(),
            },
            20,
            0,
        )
        .unwrap();
        assert_eq!(
            context.iter().rev().map(|row| row.id).collect::<Vec<_>>(),
            ids
        );
    }
    let page = stream(&pool, Some("agent-collector-a"));
    assert_eq!(page.minimum_watermark, Some(2));
    assert_eq!(page.high_watermark, 4);
    let all = tail_logs(&pool, None, None, None, None, 20).unwrap();
    let unfiltered_stream = stream(&pool, None);
    let (feed, _, _) = analytics::feed_logs(&pool, Some(0), None, 20).unwrap();
    for id in [1, 3, 5] {
        let get = analytics::fetch_log_by_id(&pool, id).unwrap().unwrap();
        assert_eq!(get.hostname, "agent-collector-b");
        assert!(
            get.metadata_json
                .as_deref()
                .unwrap()
                .contains("hostname_claim")
        );
        assert_eq!(
            all.iter().find(|row| row.id == id).unwrap().hostname,
            "agent-collector-b"
        );
        assert_eq!(
            unfiltered_stream
                .rows
                .iter()
                .find(|row| row.id == id)
                .unwrap()
                .hostname,
            "agent-collector-b"
        );
        assert_eq!(
            feed.iter().find(|row| row.id == id).unwrap().hostname,
            "agent-collector-b"
        );
    }
    let receipts = page_agent_projection_logs(&pool, 0, 20).unwrap();
    assert_eq!(
        receipts
            .iter()
            .filter(|row| row.hostname == "agent-collector-b")
            .count(),
        4
    );
    let raw_session = crate::db::durable_stream_page(
        &pool,
        &DurableStreamParams {
            hostname: Some("agent-collector-b".into()),
            ai_project: Some("collision-project".into()),
            ai_tool: Some("codex".into()),
            ai_session_id: Some("collision-session".into()),
            limit: 20,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(raw_session.rows.len(), 4);
    assert!(
        raw_session
            .rows
            .iter()
            .all(|row| row.hostname == "agent-collector-b")
    );
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM forwarded_log_hosts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        6,
        "collision resolution must not rewrite or delete raw projection evidence"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM logs WHERE hostname='agent-collector-a'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
fn colliding_deleted_claims_do_not_raise_principal_stream_floors() {
    let (pool, _dir) = fixture();
    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM logs WHERE id IN(1,3,5)", [])
        .unwrap();
    conn.execute(
        "UPDATE hosts SET log_count=1 WHERE hostname='agent-collector-b'",
        [],
    )
    .unwrap();
    drop(conn);
    let page = stream(&pool, Some("agent-collector-a"));
    assert_eq!(
        page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        [2, 4]
    );
    assert_eq!(page.minimum_watermark, Some(2));
    assert_eq!(page.high_watermark, 4);
    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM logs WHERE id IN(2,4)", [])
        .unwrap();
    conn.execute(
        "UPDATE hosts SET log_count=0 WHERE hostname='agent-collector-a'",
        [],
    )
    .unwrap();
    drop(conn);
    let deleted = stream(&pool, Some("agent-collector-a"));
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(5));
    assert_eq!(deleted.high_watermark, 4);
    for name in ["AGENT-COLLECTOR-A", "agent-collector-a."] {
        let absent = stream(&pool, Some(name));
        assert_eq!(absent.minimum_watermark, None);
        assert_eq!(absent.high_watermark, 0);
    }
}

#[test]
fn normalized_principal_collision_checks_use_catalog_index() {
    let (pool, _dir) = fixture();
    let sql = format!(
        "SELECT {}",
        crate::db::host_attribution::projected_host_guard("?1")
    );
    let plan = query_plan(
        &pool,
        &sql,
        &[rusqlite::types::Value::Text("AGENT-COLLECTOR-A.".into())],
    );
    assert!(
        plan.contains("SEARCH principal")
            && plan.contains("idx_forwarded_principal_names_device_key")
            && !plan.contains("SCAN principal"),
        "{plan}"
    );
}
