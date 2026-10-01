use super::*;

fn forwarded_entry(host: &str, peer: &str, second: u32) -> LogBatchEntry {
    let mut entry = make_entry(
        &format!("2026-09-30T12:00:{second:02}Z"),
        "agent-shared_bearer",
        "info",
        "forwarded device evidence",
    );
    entry.app_name = Some("collector-test".to_string());
    entry.source_ip = format!("agent-syslog://{peer}");
    entry.metadata_json = Some(
        serde_json::json!({
            "forwarded_provenance": {
                "hostname_claim": host,
                "authenticated_forwarder": "shared_bearer",
                "transport_peer": peer,
                "trust": "claimed"
            }
        })
        .to_string(),
    );
    entry
}

#[test]
fn shared_credential_and_peer_do_not_merge_device_host_views() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            make_entry(
                "2026-09-30T12:00:00Z",
                "workstation",
                "info",
                "direct evidence",
            ),
            forwarded_entry("workstation.local", "10.0.0.8", 1),
            forwarded_entry("file-server", "10.0.0.8", 2),
            forwarded_entry("workstation.local", "10.0.0.8", 3),
            forwarded_entry("unknown", "10.0.0.8", 4),
        ],
    )
    .unwrap();
    let hosts = list_hosts(&pool).unwrap();
    let workstation = hosts
        .iter()
        .find(|row| row.hostname == "workstation")
        .unwrap();
    assert_eq!(workstation.log_count, 3);
    assert_eq!(
        workstation.aliases,
        vec!["workstation", "workstation.local"]
    );
    assert_eq!(
        hosts
            .iter()
            .find(|row| row.hostname == "file-server")
            .unwrap()
            .log_count,
        1
    );
    assert_eq!(
        hosts
            .iter()
            .find(|row| row.hostname == "agent-shared_bearer")
            .unwrap()
            .log_count,
        1
    );
    let conn = pool.get().unwrap();
    let raw_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE hostname='agent-shared_bearer'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        raw_count, 4,
        "attribution must preserve raw source identity"
    );
}

#[test]
fn device_tail_and_search_include_only_its_attributed_rows() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            forwarded_entry("workstation", "10.0.0.8", 1),
            forwarded_entry("file-server", "10.0.0.8", 2),
            forwarded_entry("workstation", "10.0.0.8", 3),
        ],
    )
    .unwrap();
    let tail = tail_logs(&pool, Some("WORKSTATION"), None, None, None, 1).unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].hostname, "workstation");
    assert_eq!(tail[0].timestamp, "2026-09-30T12:00:03Z");
    for query in [None, Some("forwarded".to_string())] {
        let rows = search_logs(
            &pool,
            &SearchParams {
                query,
                host: Some("workstation".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.hostname == "workstation"));
        assert!(rows.iter().all(|row| {
            row.metadata_json
                .as_deref()
                .unwrap()
                .contains("shared_bearer")
        }));
    }
    let filtered = tail_logs(
        &pool,
        Some("workstation"),
        None,
        Some("collector-test"),
        None,
        10,
    )
    .unwrap();
    assert_eq!(filtered.len(), 2);
    let raw_projection = page_agent_projection_logs(&pool, 0, 10).unwrap();
    assert!(
        raw_projection
            .iter()
            .all(|row| row.hostname == "agent-shared_bearer")
    );
}

#[test]
fn device_filters_do_not_expand_to_unrelated_dns_domains() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            make_entry("2026-09-30T12:00:00Z", "db", "info", "direct"),
            forwarded_entry("db.prod.example", "10.0.0.8", 1),
            forwarded_entry("db.local", "10.0.0.8", 2),
        ],
    )
    .unwrap();
    let rows = tail_logs(&pool, Some("db"), None, None, None, 10).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "competing qualified names must retain ambiguity"
    );
    assert_eq!(rows[0].hostname, "db");
    let rows = search_logs(
        &pool,
        &SearchParams {
            host: Some("db".to_string()),
            app: Some("collector-test".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn attributed_host_lookup_is_an_indexed_bounded_probe() {
    let (pool, _dir) = test_pool();
    let plan = query_plan(
        &pool,
        "SELECT log_id FROM forwarded_log_hosts WHERE hostname=?1 ORDER BY timestamp DESC,log_id DESC LIMIT ?2",
        &[
            rusqlite::types::Value::Text("workstation".to_string()),
            rusqlite::types::Value::Integer(10),
        ],
    );
    assert!(plan.contains("SEARCH forwarded_log_hosts"), "{plan}");
    assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
    let params = SearchParams {
        host: Some("workstation".to_string()),
        app: Some("collector-test".to_string()),
        severity_in: Some(vec!["info".to_string(), "err".to_string()]),
        ..Default::default()
    };
    let (sql, bindings) =
        super::super::attributed_hosts::query_sql(&params, "workstation", 10, true);
    let plan = query_plan(&pool, &sql, &bindings);
    assert!(
        plan.contains("SEARCH a USING COVERING INDEX idx_forwarded_log_hosts_host_time")
            || plan.contains("SEARCH a USING INDEX idx_forwarded_log_hosts_host_time"),
        "{plan}"
    );
    assert!(
        plan.contains("SEARCH l USING INTEGER PRIMARY KEY"),
        "{plan}"
    );
    assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
    assert!(!plan.contains("SCAN logs"), "{plan}");
}

#[test]
fn attributed_hosts_link_only_unambiguous_stable_device_ids() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            forwarded_entry("workstation.local", "10.0.0.8", 1),
            forwarded_entry("serverhost", "10.0.0.8", 2),
            forwarded_entry("unknown", "10.0.0.8", 3),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    for (id, name) in [
        ("stable-workstation", "workstation.local"),
        ("stable-serverhost", "serverhost"),
    ] {
        conn.execute("INSERT INTO host_heartbeats
            (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
            VALUES (?1,?2,'10.0.0.8:41000','2026-09-30T12:00:00Z','2026-09-30T12:00:00Z','boot-a',60,1,5,0,'3.17.0','linux','x86_64','{}')", rusqlite::params![id,name]).unwrap();
    }
    drop(conn);
    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(
        hosts
            .iter()
            .find(|h| h.hostname == "workstation.local")
            .unwrap()
            .host_id
            .as_deref(),
        Some("stable-workstation")
    );
    assert_eq!(
        hosts
            .iter()
            .find(|h| h.hostname == "serverhost")
            .unwrap()
            .host_id
            .as_deref(),
        Some("stable-serverhost")
    );
    assert!(
        hosts
            .iter()
            .find(|h| h.hostname == "agent-shared_bearer")
            .unwrap()
            .host_id
            .is_none()
    );
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO host_heartbeats
        (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
        VALUES ('other-workstation','workstation.local','10.0.0.8:41001','2026-09-30T12:00:01Z','2026-09-30T12:00:01Z','boot-b',60,1,5,0,'3.17.0','linux','x86_64','{}')", []).unwrap();
    drop(conn);
    assert!(
        list_hosts(&pool)
            .unwrap()
            .iter()
            .find(|h| h.hostname == "workstation.local")
            .unwrap()
            .host_id
            .is_none(),
        "shared names cannot merge distinct stable IDs"
    );
}
