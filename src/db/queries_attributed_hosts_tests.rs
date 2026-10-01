use super::*;
use crate::db::HostSourceKind;

#[path = "queries_principal_collision_tests.rs"]
mod principal_collisions;

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
    let principal = hosts
        .iter()
        .find(|row| row.hostname == "agent-shared_bearer")
        .unwrap();
    assert_eq!(principal.log_count, 4);
    assert_eq!(principal.source_kind, HostSourceKind::ForwardingPrincipal);
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
    let (first_seen, last_seen): (String, String) = conn
        .query_row(
            "SELECT first_seen,last_seen FROM hosts WHERE hostname='agent-shared_bearer'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (principal.first_seen.as_str(), principal.last_seen.as_str()),
        (first_seen.as_str(), last_seen.as_str())
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
fn claimed_hosts_do_not_link_to_heartbeat_ids() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            forwarded_entry("workstation.local", "10.0.0.8", 1),
            forwarded_entry("serverhost", "10.0.0.8", 2),
            forwarded_entry("unknown", "10.0.0.8", 3),
            make_entry(
                "2026-09-30T12:00:04Z",
                "direct-box",
                "info",
                "direct evidence",
            ),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    for (id, name) in [
        ("stable-workstation", "workstation.local"),
        ("stable-serverhost", "serverhost"),
        ("stable-direct", "direct-box"),
    ] {
        conn.execute("INSERT INTO host_heartbeats
            (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
            VALUES (?1,?2,'10.0.0.8:41000','2026-09-30T12:00:00Z','2026-09-30T12:00:00Z','boot-a',60,1,5,0,'3.17.0','linux','x86_64','{}')", rusqlite::params![id,name]).unwrap();
    }
    drop(conn);
    let hosts = list_hosts(&pool).unwrap();
    for name in ["workstation.local", "serverhost"] {
        let claim = hosts.iter().find(|h| h.hostname == name).unwrap();
        assert_eq!(claim.source_kind, HostSourceKind::ClaimedHost);
        assert!(
            claim.host_id.is_none(),
            "a claim must not bind heartbeat identity"
        );
    }
    assert_eq!(
        hosts
            .iter()
            .find(|h| h.hostname == "direct-box")
            .unwrap()
            .host_id
            .as_deref(),
        Some("stable-direct")
    );
    assert!(
        hosts
            .iter()
            .find(|h| h.hostname == "agent-shared_bearer")
            .unwrap()
            .host_id
            .is_none()
    );

    // Direct evidence still requires an unambiguous stable identity.
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO host_heartbeats
        (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
        VALUES ('other-direct','direct-box','10.0.0.8:41001','2026-09-30T12:00:01Z','2026-09-30T12:00:01Z','boot-b',60,1,5,0,'3.17.0','linux','x86_64','{}')", []).unwrap();
    drop(conn);
    assert!(
        list_hosts(&pool)
            .unwrap()
            .iter()
            .find(|h| h.hostname == "direct-box")
            .unwrap()
            .host_id
            .is_none()
    );
}

#[test]
fn direct_and_claimed_rows_with_same_name_keep_claim_trust() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            make_entry("2026-09-30T12:00:00Z", "edge-node-a", "info", "direct"),
            forwarded_entry("edge-node-a", "10.0.0.8", 1),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO host_heartbeats
        (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
        VALUES ('stable-edge-node-a','edge-node-a','10.0.0.8:41000','2026-09-30T12:00:00Z','2026-09-30T12:00:00Z','boot-a',60,1,5,0,'3.17.0','linux','x86_64','{}')", []).unwrap();
    drop(conn);
    let host = list_hosts(&pool)
        .unwrap()
        .into_iter()
        .find(|entry| entry.hostname == "edge-node-a")
        .unwrap();
    assert_eq!(host.log_count, 2);
    assert_eq!(host.source_kind, HostSourceKind::ClaimedHost);
    assert!(host.host_id.is_none());
}

fn named_entry(claim: &str, second: u32) -> LogBatchEntry {
    let mut entry = forwarded_entry(claim, "10.0.0.8", second);
    entry.hostname = "agent-collector-a".into();
    let mut metadata: serde_json::Value =
        serde_json::from_str(entry.metadata_json.as_deref().unwrap()).unwrap();
    metadata["forwarded_provenance"]["authenticated_forwarder"] = "collector-a".into();
    metadata["forwarded_provenance"]["trust"] = "verified_forwarder_claimed_host".into();
    entry.metadata_json = Some(metadata.to_string());
    entry
}

#[test]
fn proved_named_principals_keep_source_identity_separate_from_device_aliases() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            named_entry("agent-collector-a.local", 1),
            named_entry("edge-node-b", 2),
            make_entry("2026-09-30T12:00:03Z", "agent-os", "info", "direct"),
            make_entry(
                "2026-09-30T12:00:04Z",
                "AGENT-OS.local",
                "info",
                "direct alias",
            ),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    for name in ["agent-collector-a", "agent-collector-a.local", "agent-os"] {
        conn.execute("INSERT INTO host_heartbeats
            (host_id,hostname,source_ip,sampled_at,received_at,boot_id,uptime_secs,sequence,collection_ms,partial,agent_version,os,architecture,metadata_json)
            VALUES (?1,?1,'10.0.0.8:41000','2026-09-30T12:00:00Z','2026-09-30T12:00:00Z','boot-a',60,1,5,0,'3.17.0','linux','x86_64','{}')", [name]).unwrap();
    }
    drop(conn);
    let hosts = list_hosts(&pool).unwrap();
    let principal = hosts
        .iter()
        .find(|h| h.hostname == "agent-collector-a")
        .unwrap();
    assert_eq!(principal.source_kind, HostSourceKind::ForwardingPrincipal);
    assert_eq!(principal.log_count, 2);
    assert_eq!(principal.aliases, ["agent-collector-a"]);
    assert!(principal.host_id.is_none());
    let claim = hosts
        .iter()
        .find(|h| h.hostname == "agent-collector-a.local")
        .unwrap();
    assert_eq!(claim.source_kind, HostSourceKind::ClaimedHost);
    assert_eq!(claim.log_count, 1);
    assert!(claim.host_id.is_none());
    let direct = hosts.iter().find(|h| h.hostname == "agent-os").unwrap();
    assert_eq!(direct.source_kind, HostSourceKind::Host);
    assert_eq!(direct.log_count, 2);
    assert_eq!(direct.host_id.as_deref(), Some("agent-os"));
    assert_eq!(
        hosts
            .iter()
            .filter(|h| matches!(
                h.source_kind,
                HostSourceKind::Host | HostSourceKind::ClaimedHost
            ))
            .count(),
        3
    );
    for (host, count) in [
        ("agent-collector-a", 2),
        ("AGENT-COLLECTOR-A", 0),
        ("agent-collector-a.local", 1),
        ("agent-os", 2),
    ] {
        assert_eq!(
            tail_logs(&pool, Some(host), None, None, None, 10)
                .unwrap()
                .len(),
            count
        );
        for query in [None, Some("forwarded".to_string())] {
            if host == "agent-os" && query.is_some() {
                continue;
            }
            assert_eq!(
                search_logs(
                    &pool,
                    &SearchParams {
                        host: Some(host.into()),
                        query,
                        limit: Some(10),
                        ..Default::default()
                    }
                )
                .unwrap()
                .len(),
                count
            );
        }
        assert_eq!(
            crate::db::durable_stream_page(
                &pool,
                &crate::db::DurableStreamParams {
                    hostname: Some(host.into()),
                    limit: 10,
                    include_bounds: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .rows
            .len(),
            count
        );
    }
}

#[test]
fn named_principal_proof_survives_last_attributed_row_deletion() {
    let (pool, _dir) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            named_entry("agent-collector-a.local", 1),
            named_entry("unknown", 2),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM logs WHERE id=1", []).unwrap();
    // Emulate the retention path's raw registry adjustment independently from
    // attribution cleanup, which must retain the observed principal namespace.
    conn.execute(
        "UPDATE hosts SET log_count=1 WHERE hostname='agent-collector-a'",
        [],
    )
    .unwrap();
    assert!(
        crate::db::host_attribution::list_forwarded_host_counts(&conn)
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let hosts = list_hosts(&pool).unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].hostname, "agent-collector-a");
    assert_eq!(hosts[0].source_kind, HostSourceKind::ForwardingPrincipal);
    assert!(hosts[0].host_id.is_none());
    assert_eq!(
        tail_logs(&pool, Some("agent-collector-a.local"), None, None, None, 10)
            .unwrap()
            .len(),
        0
    );
    let deleted = crate::db::durable_stream_page(
        &pool,
        &crate::db::DurableStreamParams {
            hostname: Some("agent-collector-a.local".into()),
            limit: 10,
            include_bounds: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(2));
    let principal = crate::db::durable_stream_page(
        &pool,
        &crate::db::DurableStreamParams {
            hostname: Some("agent-collector-a".into()),
            limit: 10,
            include_bounds: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(principal.rows.len(), 1);
    assert_eq!(principal.rows[0].id, 2);
}
