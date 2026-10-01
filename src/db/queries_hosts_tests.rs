use super::*;

fn host_entry(name: &str, first: &str, last: &str, count: i64) -> HostEntry {
    HostEntry {
        hostname: name.to_string(),
        host_id: None,
        first_seen: first.to_string(),
        last_seen: last.to_string(),
        log_count: count,
        aliases: Vec::new(),
        source_kind: Default::default(),
    }
}

#[test]
fn dedupe_hosts_folds_case_variants() {
    let out = dedupe_hosts(vec![
        host_entry(
            "BACKUPHOST",
            "2026-06-01T00:00:00Z",
            "2026-06-10T00:00:00Z",
            10,
        ),
        host_entry(
            "backuphost",
            "2026-06-02T00:00:00Z",
            "2026-06-12T00:00:00Z",
            5,
        ),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "backuphost");
    assert_eq!(out[0].log_count, 15);
    assert_eq!(out[0].first_seen, "2026-06-01T00:00:00Z"); // earliest
    assert_eq!(out[0].last_seen, "2026-06-12T00:00:00Z"); // latest
}

#[test]
fn dedupe_hosts_folds_fqdn_into_existing_short_name() {
    let out = dedupe_hosts(vec![
        host_entry(
            "nashost",
            "2026-06-01T00:00:00Z",
            "2026-06-10T00:00:00Z",
            100,
        ),
        host_entry(
            "nashost.example.ts.net",
            "2026-06-03T00:00:00Z",
            "2026-06-09T00:00:00Z",
            7,
        ),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "nashost");
    assert_eq!(out[0].log_count, 107);
}

#[test]
fn dedupe_hosts_keeps_fqdn_when_no_matching_short_name() {
    // No bare `host` row exists, so `host.docker.internal` must NOT be folded to
    // `host` — folding there would invent a merge and could mask a real machine.
    let out = dedupe_hosts(vec![host_entry(
        "host.docker.internal",
        "2026-06-01T00:00:00Z",
        "2026-06-10T00:00:00Z",
        42,
    )]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "host.docker.internal");
}

#[test]
fn dedupe_hosts_leaves_ambiguous_self_identifiers_untouched() {
    // localhost and dotless host:user forms are deferred (need source_ip).
    let out = dedupe_hosts(vec![
        host_entry(
            "localhost",
            "2026-06-01T00:00:00Z",
            "2026-06-10T00:00:00Z",
            3,
        ),
        host_entry("devhost", "2026-06-01T00:00:00Z", "2026-06-11T00:00:00Z", 9),
        host_entry(
            "devhost:jmagar",
            "2026-06-01T00:00:00Z",
            "2026-06-05T00:00:00Z",
            2,
        ),
    ]);
    let names: std::collections::HashSet<&str> = out.iter().map(|h| h.hostname.as_str()).collect();
    assert!(names.contains("localhost"));
    assert!(names.contains("devhost"));
    assert!(names.contains("devhost:jmagar")); // colon, no dot → not folded into devhost
    assert_eq!(out.len(), 3);
}

#[test]
fn dedupe_hosts_excludes_blank_hostnames() {
    let out = dedupe_hosts(vec![
        host_entry("", "2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", 3),
        host_entry("   ", "2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", 4),
        host_entry("devhost", "2026-06-01T00:00:00Z", "2026-06-11T00:00:00Z", 9),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "devhost");
}

#[test]
fn dedupe_hosts_orders_by_last_seen_desc() {
    let out = dedupe_hosts(vec![
        host_entry("alpha", "2026-06-01T00:00:00Z", "2026-06-05T00:00:00Z", 1),
        host_entry("bravo", "2026-06-01T00:00:00Z", "2026-06-20T00:00:00Z", 1),
    ]);
    assert_eq!(out[0].hostname, "bravo"); // most recent first
    assert_eq!(out[1].hostname, "alpha");
}

#[test]
fn host_aliases_remain_available_for_exact_filters() {
    let out = dedupe_hosts(vec![
        host_entry("DEVHOST", "2026-06-01", "2026-06-02", 2),
        host_entry("devhost.local.", "2026-06-01", "2026-06-03", 3),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "devhost");
    assert_eq!(out[0].aliases, vec!["DEVHOST", "devhost.local."]);
    assert_eq!(out[0].log_count, 5);
}

#[test]
fn hostname_similarity_does_not_merge_distinct_domains_or_guests() {
    let names = [
        "db",
        "db.prod.example",
        "db.dev.example",
        "db.local",
        "db.alpha.ts.net",
        "db.beta.ts.net",
        "db-wsl",
    ];
    let rows = names
        .iter()
        .map(|name| host_entry(name, "2026-06-01", "2026-06-02", 1))
        .collect();
    let out = dedupe_hosts(rows);
    assert_eq!(out.len(), names.len());
    assert!(out.iter().all(|row| row.log_count == 1));
}

#[test]
fn forwarding_principals_are_not_devices_or_case_insensitive_aliases() {
    let rows = [
        "agent-shared_bearer",
        "bearer-shared-one",
        "bearer-shared-Mac",
        "bearer-shared-mac",
        "localhost",
        "devhost",
    ]
    .iter()
    .map(|name| host_entry(name, "2026-06-01", "2026-06-02", 1))
    .collect();
    let out = dedupe_hosts(rows);
    assert_eq!(out.len(), 6);
    assert_eq!(
        out.iter()
            .filter(|row| row.source_kind == HostSourceKind::ForwardingPrincipal)
            .count(),
        4
    );
    assert_eq!(
        out.iter()
            .filter(|row| row.source_kind == HostSourceKind::Unattributed)
            .count(),
        1
    );
    assert_eq!(
        out.iter()
            .filter(|row| row.source_kind == HostSourceKind::Host)
            .count(),
        1
    );
}

#[test]
fn real_device_names_with_agent_prefix_remain_hosts() {
    let out = dedupe_hosts(vec![
        host_entry("AGENT-OS", "2026-06-01", "2026-06-02", 1),
        host_entry("agent-os", "2026-06-01", "2026-06-03", 2),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "agent-os");
    assert_eq!(out[0].source_kind, HostSourceKind::Host);
    assert_eq!(out[0].log_count, 3);
}

fn attribution_fixture() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = super::super::init_pool(&crate::config::StorageConfig::for_test(
        dir.path().join("alias-history.db"),
    ))
    .unwrap();
    (pool, dir)
}

fn attributed_entry(host: &str, forwarded: bool, second: u32) -> super::super::LogBatchEntry {
    super::super::LogBatchEntry {
        timestamp: format!("2026-09-30T12:00:{second:02}Z"),
        hostname: if forwarded {
            "agent-shared_bearer".into()
        } else {
            host.into()
        },
        facility: None,
        severity: "info".into(),
        app_name: None,
        process_id: None,
        message: "proof device evidence".into(),
        raw: "proof device evidence".into(),
        source_ip: if forwarded {
            "agent-ai-transcript://10.0.0.8".into()
        } else {
            "direct://log".into()
        },
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: forwarded.then(|| {
            serde_json::json!({"provenance": {
                "hostname_claim":host,"authenticated_forwarder":"shared_bearer",
                "transport_peer":"10.0.0.8","trust":"claimed"
            }})
            .to_string()
        }),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

fn device_stream(pool: &DbPool, host: &str) -> super::super::DurableStreamPage {
    super::super::durable_stream_page(
        pool,
        &super::super::DurableStreamParams {
            hostname: Some(host.into()),
            limit: 100,
            include_bounds: true,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn deleted_unrelated_domain_does_not_poison_active_inventory_aliases() {
    let (pool, _dir) = attribution_fixture();
    super::super::insert_logs_batch(
        &pool,
        &[
            attributed_entry("db", false, 1),
            attributed_entry("db.local", true, 2),
            attributed_entry("db.prod.example", true, 3),
        ],
    )
    .unwrap();
    pool.get()
        .unwrap()
        .execute("DELETE FROM logs WHERE id=3", [])
        .unwrap();
    let inventory = list_hosts(&pool).unwrap();
    let db = inventory.iter().find(|host| host.hostname == "db").unwrap();
    assert_eq!(db.aliases, vec!["db", "db.local"]);
    assert_eq!(db.log_count, 2);
    for query in [None, Some("proof".to_string())] {
        let rows = super::super::search_logs(
            &pool,
            &super::super::SearchParams {
                host: Some("db".into()),
                query,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
    }
    let tail = super::super::tail_logs(&pool, Some("db"), None, None, None, 10).unwrap();
    assert_eq!(
        tail.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
    let live = device_stream(&pool, "db");
    assert_eq!(
        live.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(live.minimum_watermark, Some(1));
    assert_eq!(live.high_watermark, 2);
    let unrelated = device_stream(&pool, "db.prod.example");
    assert!(unrelated.rows.is_empty());
    assert_eq!(unrelated.minimum_watermark, Some(4));
    assert_eq!(unrelated.high_watermark, 3);
    pool.get().unwrap().execute("DELETE FROM logs", []).unwrap();
    let deleted = device_stream(&pool, "db");
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(3));
    assert_eq!(deleted.high_watermark, 2);
}

#[test]
fn deleted_bare_name_does_not_relabel_a_live_qualified_host() {
    let (pool, _dir) = attribution_fixture();
    super::super::insert_logs_batch(
        &pool,
        &[
            attributed_entry("db", true, 1),
            attributed_entry("db.local", true, 2),
        ],
    )
    .unwrap();
    pool.get()
        .unwrap()
        .execute("DELETE FROM logs WHERE id=1", [])
        .unwrap();
    let inventory = list_hosts(&pool).unwrap();
    assert!(inventory.iter().any(|host| host.hostname == "db.local"));
    assert!(!inventory.iter().any(|host| host.hostname == "db"));
    let local = device_stream(&pool, "db.local");
    assert_eq!(
        local.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(local.minimum_watermark, Some(2));
    assert_eq!(local.high_watermark, 2);
    assert!(
        super::super::tail_logs(&pool, Some("db"), None, None, None, 10)
            .unwrap()
            .is_empty()
    );
    let bare = device_stream(&pool, "db");
    assert!(bare.rows.is_empty());
    assert_eq!(bare.minimum_watermark, Some(2));
    assert_eq!(bare.high_watermark, 1);
    pool.get().unwrap().execute("DELETE FROM logs", []).unwrap();
    let deleted = device_stream(&pool, "db");
    assert!(deleted.rows.is_empty());
    assert_eq!(deleted.minimum_watermark, Some(3));
    assert_eq!(deleted.high_watermark, 2);
}
