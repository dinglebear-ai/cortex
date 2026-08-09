use super::*;
use crate::app::error_detection::normalize::normalize_template;
use crate::config::StorageConfig;
use crate::db::{
    DbPool, LogBatchEntry, ingest_source_kind_health, init_pool, insert_logs_batch,
    prune_timeline_rollup, refresh_timeline_rollup, timeline_rollup_status,
};

fn test_pool() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    (pool, dir)
}

fn entry(ts: &str, host: &str, severity: &str, app: Option<&str>, msg: &str) -> LogBatchEntry {
    entry_with_source_ip(ts, host, severity, app, msg, "127.0.0.1:514")
}

fn entry_with_source_ip(
    ts: &str,
    host: &str,
    severity: &str,
    app: Option<&str>,
    msg: &str,
    source_ip: &str,
) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: severity.to_string(),
        app_name: app.map(String::from),
        process_id: None,
        message: msg.to_string(),
        raw: format!("<14>{ts} {host} {}: {msg}", app.unwrap_or("test")),
        source_ip: source_ip.to_string(),
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

#[test]
fn template_normalises_numbers_ips_uuids() {
    let t = normalize_template(
        "connection refused from 10.0.0.5:42 (id b3a1c0de-1234-5678-9abc-def012345678)",
    );
    assert!(t.contains("<ip>:<n>"));
    assert!(t.contains("<uuid>"));
}

#[test]
fn template_preserves_non_ascii_codepoints() {
    // Multi-byte UTF-8 sequences must round-trip rather than getting split into
    // mojibake by `b as char`.
    let msg = "файл 1234 не найден";
    let t = normalize_template(msg);
    assert!(t.contains("файл"));
    assert!(t.contains("не найден"));
    assert!(t.contains("<n>"));
    assert!(t.is_char_boundary(t.len()));
}

#[test]
fn list_apps_returns_distinct_apps_with_counts() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:01Z", "h1", "info", Some("nginx"), "hello"),
            entry("2026-01-01T00:00:02Z", "h1", "info", Some("nginx"), "again"),
            entry(
                "2026-01-01T00:00:03Z",
                "h2",
                "info",
                Some("sshd"),
                "auth ok",
            ),
        ],
    )
    .unwrap();

    let apps = list_apps(
        &pool,
        &ListAppsParams {
            hostname: None,
            from: None,
            to: None,
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(apps.total, 2);
    let nginx = apps.apps.iter().find(|a| a.app_name == "nginx").unwrap();
    assert_eq!(nginx.log_count, 2);
    assert_eq!(nginx.host_count, 1);

    // Filter by hostname
    let only_h2 = list_apps(
        &pool,
        &ListAppsParams {
            hostname: Some("h2"),
            from: None,
            to: None,
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(only_h2.apps.len(), 1);
    assert_eq!(only_h2.apps[0].app_name, "sshd");
}

#[test]
fn ingest_source_kind_health_classifies_recent_sources() {
    let (pool, _d) = test_pool();
    let mut syslog = entry(
        "2026-01-01T11:58:00Z",
        "h1",
        "info",
        Some("rsyslogd"),
        "syslog",
    );
    syslog.metadata_json = Some(r#"{"source_kind":"syslog-udp"}"#.to_string());

    let mut otlp = entry("2026-01-01T11:30:00Z", "h1", "info", Some("otel"), "otlp");
    otlp.metadata_json = Some(r#"{"source_kind":"otlp"}"#.to_string());

    let docker = entry_with_source_ip(
        "2026-01-01T10:30:00Z",
        "h2",
        "info",
        Some("container"),
        "docker",
        "docker://h2/container/stdout",
    );

    let mut transcript = entry_with_source_ip(
        "2026-01-01T11:59:00Z",
        "h3",
        "info",
        Some("codex"),
        "transcript",
        "transcript://codex",
    );
    transcript.ai_transcript_path = Some("/tmp/session.jsonl".to_string());

    insert_logs_batch(&pool, &[syslog, otlp, docker, transcript]).unwrap();
    pool.get()
        .unwrap()
        .execute("UPDATE logs SET received_at = timestamp", [])
        .unwrap();

    let rows = ingest_source_kind_health(
        &pool,
        "2026-01-01T12:00:00Z",
        "2026-01-01T11:45:00Z",
        "2026-01-01T11:00:00Z",
        "2025-12-31T12:00:00Z",
    )
    .unwrap();
    let by_kind: std::collections::HashMap<_, _> = rows
        .into_iter()
        .map(|row| (row.source_kind.clone(), row))
        .collect();

    assert_eq!(by_kind["syslog-udp"].last_15m, 1);
    assert_eq!(by_kind["otlp"].last_15m, 0);
    assert_eq!(by_kind["otlp"].last_1h, 1);
    assert_eq!(by_kind["docker-stream"].last_1h, 0);
    assert_eq!(by_kind["docker-stream"].last_24h, 1);
    assert_eq!(by_kind["transcript"].last_15m, 1);
}

#[test]
fn unfiltered_list_apps_uses_inventory_stats_without_scanning_logs() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:01Z", "h1", "info", Some("nginx"), "hello"),
            entry("2026-01-01T00:00:02Z", "h2", "info", Some("nginx"), "again"),
            entry(
                "2026-01-01T00:00:03Z",
                "h2",
                "info",
                Some("sshd"),
                "auth ok",
            ),
        ],
    )
    .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE inventory_backfill_state
             SET completed_at = '2026-01-01T00:00:00Z'
             WHERE name = 'app_source_inventory'",
            [],
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let plan = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             WITH page AS (
                SELECT app_name, log_count, first_seen, last_seen
                FROM app_inventory_stats
                ORDER BY last_seen DESC, app_name ASC
                LIMIT 50 OFFSET 0
             )
             SELECT p.app_name, p.log_count, COUNT(h.hostname), p.first_seen, p.last_seen
             FROM page p
             LEFT JOIN app_host_inventory_stats h ON h.app_name = p.app_name
             GROUP BY p.app_name, p.log_count, p.first_seen, p.last_seen
             ORDER BY p.last_seen DESC, p.app_name ASC",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        !plan.contains("logs"),
        "unfiltered app inventory should not scan logs; got:\n{plan}"
    );
    drop(conn);

    let apps = list_apps(
        &pool,
        &ListAppsParams {
            hostname: None,
            from: None,
            to: None,
            limit: 50,
            offset: 0,
        },
    )
    .unwrap();
    let nginx = apps.apps.iter().find(|a| a.app_name == "nginx").unwrap();
    assert_eq!(nginx.log_count, 2);
    assert_eq!(nginx.host_count, 2);
}

#[test]
fn list_apps_to_filter_excludes_future_entries() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[entry(
            "2026-01-01T00:00:01Z",
            "h1",
            "info",
            Some("nginx"),
            "msg",
        )],
    )
    .unwrap();

    // All entries are inserted with received_at = now(). A `to` in the far past
    // should exclude them all; a `to` in the far future should include them.
    let none = list_apps(
        &pool,
        &ListAppsParams {
            hostname: None,
            from: None,
            to: Some("2000-01-01T00:00:00Z"),
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert!(
        none.apps.is_empty(),
        "to=2000 should exclude all entries inserted now"
    );

    let all = list_apps(
        &pool,
        &ListAppsParams {
            hostname: None,
            from: None,
            to: Some("9999-01-01T00:00:00Z"),
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert!(!all.apps.is_empty(), "to=9999 should include all entries");
}

#[test]
fn inventory_stats_decrement_when_logs_are_deleted() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry_with_source_ip(
                "2026-01-01T00:00:01Z",
                "h1",
                "info",
                Some("nginx"),
                "hello",
                "10.0.0.1:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:02Z",
                "h2",
                "info",
                Some("nginx"),
                "again",
                "10.0.0.1:514",
            ),
        ],
    )
    .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE inventory_backfill_state
             SET completed_at = '2026-01-01T00:00:00Z'
             WHERE name = 'app_source_inventory'",
            [],
        )
        .unwrap();

    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM logs WHERE hostname = 'h1'", [])
        .unwrap();
    drop(conn);

    let apps = list_apps(
        &pool,
        &ListAppsParams {
            hostname: None,
            from: None,
            to: None,
            limit: 50,
            offset: 0,
        },
    )
    .unwrap();
    let nginx = apps.apps.iter().find(|a| a.app_name == "nginx").unwrap();
    assert_eq!(nginx.log_count, 1);
    assert_eq!(nginx.host_count, 1);

    let source_ips = list_source_ips(
        &pool,
        &ListSourceIpsParams {
            limit: 50,
            offset: 0,
        },
    )
    .unwrap();
    let ip = source_ips
        .source_ips
        .iter()
        .find(|entry| entry.source_ip == "10.0.0.1:514")
        .unwrap();
    assert_eq!(ip.log_count, 1);
    assert_eq!(ip.host_count, 1);
}

#[test]
fn list_source_ips_truncated_when_over_limit() {
    let (pool, _d) = test_pool();
    // Insert 3 entries with distinct source IPs; request limit=2 to force truncation.
    insert_logs_batch(
        &pool,
        &[
            entry_with_source_ip(
                "2026-01-01T00:00:01Z",
                "h1",
                "info",
                None,
                "a",
                "10.0.0.1:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:02Z",
                "h1",
                "info",
                None,
                "b",
                "10.0.0.2:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:03Z",
                "h1",
                "info",
                None,
                "c",
                "10.0.0.3:514",
            ),
        ],
    )
    .unwrap();

    let result = list_source_ips(
        &pool,
        &ListSourceIpsParams {
            limit: 2,
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(result.total, 3, "total should reflect all 3 distinct IPs");
    assert_eq!(
        result.source_ips.len(),
        2,
        "page should contain only limit=2 IPs"
    );
}

#[test]
fn unfiltered_list_source_ips_uses_inventory_stats_without_scanning_logs() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry_with_source_ip("2026-01-01T00:00:01Z", "h1", "info", None, "a", "10.0.0.1"),
            entry_with_source_ip("2026-01-01T00:00:02Z", "h2", "info", None, "b", "10.0.0.1"),
            entry_with_source_ip("2026-01-01T00:00:03Z", "h3", "info", None, "c", "10.0.0.2"),
        ],
    )
    .unwrap();

    let conn = pool.get().unwrap();
    let plan = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             WITH page AS (
                SELECT source_ip, log_count, first_seen, last_seen
                FROM source_ip_inventory_stats
                ORDER BY log_count DESC, source_ip ASC
                LIMIT 50 OFFSET 0
             )
             SELECT p.source_ip, p.log_count, p.first_seen, p.last_seen,
                    h.hostname, h.log_count, h.first_seen, h.last_seen
             FROM page p
             LEFT JOIN source_ip_host_inventory_stats h ON h.source_ip = p.source_ip
             ORDER BY p.log_count DESC, p.source_ip ASC, h.log_count DESC, h.hostname ASC",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        !plan.contains("logs"),
        "unfiltered source inventory should not scan logs; got:\n{plan}"
    );
    drop(conn);

    let result = list_source_ips(
        &pool,
        &ListSourceIpsParams {
            limit: 50,
            offset: 0,
        },
    )
    .unwrap();
    let ip = result
        .source_ips
        .iter()
        .find(|entry| entry.source_ip == "10.0.0.1")
        .unwrap();
    assert_eq!(ip.log_count, 2);
    assert_eq!(ip.host_count, 2);
}

#[test]
fn list_source_ips_chatty_ip_does_not_suppress_others() {
    // One IP with many hostnames must not crowd out other distinct IPs.
    let (pool, _d) = test_pool();
    let mut entries = vec![];
    // ip1 logs from 20 different hostnames
    for i in 0..20 {
        entries.push(entry_with_source_ip(
            "2026-01-01T00:00:01Z",
            &format!("host-{i}"),
            "info",
            None,
            "msg",
            "10.0.0.1:514",
        ));
    }
    // ip2 logs once
    entries.push(entry_with_source_ip(
        "2026-01-01T00:00:02Z",
        "h2",
        "info",
        None,
        "msg",
        "10.0.0.2:514",
    ));
    insert_logs_batch(&pool, &entries).unwrap();

    let result = list_source_ips(
        &pool,
        &ListSourceIpsParams {
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(result.total, 2);
    assert!(
        result
            .source_ips
            .iter()
            .any(|e| e.source_ip == "10.0.0.2:514"),
        "ip2 must appear even though ip1 has many hostnames"
    );
}

#[test]
fn timeline_buckets_by_hour() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:10:00Z", "h1", "info", None, "a"),
            entry("2026-01-01T00:50:00Z", "h1", "info", None, "b"),
            entry("2026-01-01T01:05:00Z", "h1", "info", None, "c"),
        ],
    )
    .unwrap();
    // hour/day/week/month read the timeline_hourly rollup; populate it first
    // (the background task does this in prod).
    refresh_timeline_rollup(&pool).unwrap();
    let pts = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0].count, 2);
    assert_eq!(pts[1].count, 1);
}

#[test]
fn patterns_clusters_by_template() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry(
                "2026-01-01T00:00:01Z",
                "h1",
                "err",
                None,
                "disk 1234 failed",
            ),
            entry(
                "2026-01-01T00:00:02Z",
                "h1",
                "err",
                None,
                "disk 9999 failed",
            ),
            entry("2026-01-01T00:00:03Z", "h2", "err", None, "disk 5 failed"),
            entry("2026-01-01T00:00:04Z", "h1", "info", None, "all good"),
        ],
    )
    .unwrap();
    let (rows, truncated) = fetch_pattern_rows(&pool, None, None, None, None, None, 100).unwrap();
    let (pats, scanned) = cluster_pattern_rows(rows, 10);
    assert!(!truncated);
    assert_eq!(scanned, 4);
    let top = &pats[0];
    assert_eq!(top.count, 3);
    assert_eq!(top.host_count, 2);
}

#[test]
fn fetch_pattern_rows_limit_is_bound_and_clamped() {
    let (sql, bindings, scan_limit) = pattern_rows_sql(
        Some("2026-01-01T00:00:00Z"),
        Some("2026-01-01T01:00:00Z"),
        Some("h1"),
        Some("sshd"),
        Some(&["err".to_string(), "warning".to_string()]),
        PATTERN_SCAN_LIMIT_MAX + 1,
    );
    assert_eq!(scan_limit, PATTERN_SCAN_LIMIT_MAX);
    assert!(
        sql.contains("LIMIT ?"),
        "fetch_pattern_rows must bind LIMIT instead of interpolating it: {sql}"
    );
    assert!(
        bindings.iter().any(|value| {
            matches!(
                value,
                rusqlite::types::Value::Integer(limit)
                    if *limit == i64::from(PATTERN_SCAN_LIMIT_MAX + 1)
            )
        }),
        "fetch_pattern_rows should bind scan_limit+1 for truncation detection, got: {bindings:?}"
    );
}

#[test]
fn fetch_log_by_id_returns_raw() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[entry("2026-01-01T00:00:01Z", "h1", "info", None, "hello")],
    )
    .unwrap();
    let row = fetch_log_by_id(&pool, 1).unwrap().unwrap();
    assert_eq!(row.message, "hello");
    assert!(row.raw.contains("hello"));
}

#[test]
fn context_around_returns_neighbours() {
    let (pool, _d) = test_pool();
    let mut entries = Vec::new();
    for i in 0..10 {
        entries.push(entry(
            &format!("2026-01-01T00:00:{:02}Z", i),
            "h1",
            "info",
            None,
            &format!("msg {i}"),
        ));
    }
    insert_logs_batch(&pool, &entries).unwrap();
    let r = ContextRef {
        id: Some(5),
        hostname: "h1".to_string(),
        timestamp: "2026-01-01T00:00:04Z".to_string(),
    };
    let (before, after) = context_around(&pool, &r, 3, 3).unwrap();
    assert_eq!(before.len(), 3);
    assert_eq!(after.len(), 3);
    assert!(before.last().unwrap().timestamp.as_str() < "2026-01-01T00:00:04Z");
    assert!(after.first().unwrap().timestamp.as_str() > "2026-01-01T00:00:04Z");
}

#[test]
fn context_timestamp_only_anchor_splits_symmetrically() {
    // Two rows share the exact reference timestamp; with id=None they must not
    // all land on one side. The before/after split is strict on `< ts` / `> ts`,
    // so simultaneous rows are excluded from both — consistent regardless of id ordering.
    let (pool, _d) = test_pool();
    let mut entries = Vec::new();
    for i in 0..5 {
        entries.push(entry(
            &format!("2026-01-01T00:00:{:02}Z", i),
            "h1",
            "info",
            None,
            "msg",
        ));
    }
    // Two rows at the exact reference time.
    entries.push(entry("2026-01-01T00:00:05Z", "h1", "info", None, "ref-a"));
    entries.push(entry("2026-01-01T00:00:05Z", "h1", "info", None, "ref-b"));
    for i in 6..10 {
        entries.push(entry(
            &format!("2026-01-01T00:00:{:02}Z", i),
            "h1",
            "info",
            None,
            "msg",
        ));
    }
    insert_logs_batch(&pool, &entries).unwrap();

    let r = ContextRef {
        id: None,
        hostname: "h1".to_string(),
        timestamp: "2026-01-01T00:00:05Z".to_string(),
    };
    let (before, after) = context_around(&pool, &r, 10, 10).unwrap();
    // 5 strictly-less timestamps, 4 strictly-greater. Neither contains a row at 05.
    assert_eq!(before.len(), 5);
    assert_eq!(after.len(), 4);
    assert!(
        before
            .iter()
            .all(|r| r.timestamp.as_str() < "2026-01-01T00:00:05Z")
    );
    assert!(
        after
            .iter()
            .all(|r| r.timestamp.as_str() > "2026-01-01T00:00:05Z")
    );
}

fn ai_entry(ts: &str, tool: &str, project: &str, session_id: &str, message: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: "host-a".to_string(),
        facility: Some("local0".to_string()),
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: message.to_string(),
        source_ip: "127.0.0.1:514".to_string(),
        docker_checkpoint: None,
        ai_tool: Some(tool.to_string()),
        ai_project: Some(project.to_string()),
        ai_session_id: Some(session_id.to_string()),
        ai_transcript_path: Some(format!("{project}/{session_id}.jsonl")),
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn usage_blocks_group_into_five_hour_windows() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            ai_entry(
                "2026-01-01T00:00:00Z",
                "claude",
                "/tmp/project",
                "sess-1",
                "one",
            ),
            ai_entry(
                "2026-01-01T04:59:59Z",
                "claude",
                "/tmp/project",
                "sess-1",
                "two",
            ),
            ai_entry(
                "2026-01-01T05:00:00Z",
                "claude",
                "/tmp/project",
                "sess-2",
                "three",
            ),
        ],
    )
    .unwrap();

    let result = get_ai_usage_blocks(
        &pool,
        &AiUsageBlocksParams {
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-01-01T06:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.blocks.len(), 2);
    assert_eq!(result.blocks[0].event_count, 1);
    assert_eq!(result.blocks[1].event_count, 2);
}

#[test]
fn usage_blocks_honors_requested_limit() {
    let (pool, _d) = test_pool();
    for i in 0..3 {
        insert_logs_batch(
            &pool,
            &[ai_entry(
                "2026-01-01T00:00:00Z",
                "claude",
                &format!("/tmp/project-{i}"),
                &format!("sess-{i}"),
                "usage block",
            )],
        )
        .unwrap();
    }

    let result = get_ai_usage_blocks(
        &pool,
        &AiUsageBlocksParams {
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-01-01T01:00:00Z".into()),
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.blocks.len(), 2);
    assert_eq!(result.total_blocks, 2);
    assert!(result.truncated);
}

#[test]
fn usage_blocks_total_blocks_equals_len_when_truncated() {
    // When truncated, total_blocks == blocks.len() (the limit); truncated flag
    // is the authoritative indicator that more groups exist.
    let (pool, _d) = test_pool();
    let mut entries = Vec::new();
    for i in 0..1002 {
        entries.push(ai_entry(
            "2026-01-01T00:00:00Z",
            "claude",
            &format!("/tmp/project-{i}"),
            &format!("sess-{i}"),
            "usage block",
        ));
    }
    insert_logs_batch(&pool, &entries).unwrap();

    let result = get_ai_usage_blocks(
        &pool,
        &AiUsageBlocksParams {
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-07-31T00:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.blocks.len(), 1000);
    assert_eq!(result.total_blocks, 1000);
    assert!(result.truncated);
}

#[test]
fn project_context_returns_recent_entries() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            ai_entry(
                "2026-01-01T00:00:00Z",
                "claude",
                "/tmp/project",
                "sess-1",
                "one",
            ),
            ai_entry(
                "2026-01-01T00:01:00Z",
                "claude",
                "/tmp/project",
                "sess-2",
                "two",
            ),
        ],
    )
    .unwrap();
    let result = get_ai_project_context(
        &pool,
        &AiProjectContextParams {
            project: "/tmp/project".into(),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.project, "/tmp/project");
    assert_eq!(result.event_count, 2);
    assert_eq!(result.recent_entries.len(), 1);
}

#[test]
fn project_context_snippets_are_bounded_to_256_chars() {
    let (pool, _d) = test_pool();
    let long_message = "a".repeat(300);
    insert_logs_batch(
        &pool,
        &[ai_entry(
            "2026-01-01T00:00:00Z",
            "claude",
            "/tmp/project",
            "sess-1",
            &long_message,
        )],
    )
    .unwrap();

    let result = get_ai_project_context(
        &pool,
        &AiProjectContextParams {
            project: "/tmp/project".into(),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.recent_entries[0].message.chars().count(), 256);
}

#[test]
fn clock_skew_plan_uses_received_at_range_index() {
    let (pool, _d) = test_pool();
    let conn = pool.get().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {CLOCK_SKEW_SQL}");
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(rusqlite::params!["2026-01-01T00:00:00Z"], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let plan = rows.join("\n");

    assert!(
        plan.contains("idx_logs_received_at"),
        "clock_skew must range-scan received_at; got:\n{plan}"
    );
    assert!(
        !plan.contains("SCAN logs USING INDEX idx_logs_hostname"),
        "clock_skew must not scan the hostname index for grouped recent windows; got:\n{plan}"
    );
}

#[test]
fn clock_skew_limits_hosts_in_skew_order() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:00Z", "h-low", "info", None, "low"),
            entry("2026-01-01T00:00:00Z", "h-high", "info", None, "high"),
            entry("2026-01-01T00:00:00Z", "h-mid", "info", None, "mid"),
        ],
    )
    .unwrap();

    let conn = pool.get().unwrap();
    for (host, received_at) in [
        ("h-low", "2026-01-01T00:00:10Z"),
        ("h-high", "2026-01-01T00:10:00Z"),
        ("h-mid", "2026-01-01T00:01:00Z"),
    ] {
        conn.execute(
            "UPDATE logs SET received_at = ?1 WHERE hostname = ?2",
            rusqlite::params![received_at, host],
        )
        .unwrap();
    }
    drop(conn);

    let result = clock_skew(&pool, "2026-01-01T00:00:00Z", Some(2)).unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].hostname, "h-high");
    assert_eq!(result[1].hostname, "h-mid");
}

#[test]
fn summarize_range_counts_errors() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:01Z", "h1", "err", Some("a"), "x"),
            entry("2026-01-01T00:00:02Z", "h1", "info", Some("a"), "y"),
            entry("2026-01-01T00:00:03Z", "h2", "warning", Some("b"), "z"),
        ],
    )
    .unwrap();
    let summary = summarize_range(&pool, "2026-01-01T00:00:00Z", "2026-01-01T00:00:10Z").unwrap();
    assert_eq!(summary.total_logs, 3);
    assert_eq!(summary.total_errors, 2);
    assert_eq!(summary.top_apps.len(), 2);
}

#[test]
fn list_source_ips_aggregates_hostnames() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry_with_source_ip(
                "2026-01-01T00:00:01Z",
                "h1",
                "info",
                None,
                "x",
                "10.0.0.1:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:02Z",
                "h2",
                "info",
                None,
                "x",
                "10.0.0.1:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:03Z",
                "h2",
                "info",
                None,
                "x",
                "10.0.0.1:514",
            ),
            entry_with_source_ip(
                "2026-01-01T00:00:04Z",
                "h3",
                "info",
                None,
                "x",
                "10.0.0.2:514",
            ),
        ],
    )
    .unwrap();
    let result = list_source_ips(
        &pool,
        &ListSourceIpsParams {
            limit: 500,
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(result.total, 2);
    let first = result
        .source_ips
        .iter()
        .find(|e| e.source_ip == "10.0.0.1:514")
        .unwrap();
    assert_eq!(first.host_count, 2);
    assert_eq!(first.log_count, 3);
}

#[test]
fn bucket_week_formats_correctly() {
    // Week bucket uses ISO week number format: "YYYY-WNN"
    assert_eq!(Bucket::Week.strftime_format(), "%Y-W%W");
    assert_eq!(Bucket::parse("week"), Some(Bucket::Week));
    assert_eq!(Bucket::parse("w"), Some(Bucket::Week));
}

#[test]
fn bucket_month_formats_correctly() {
    // Month bucket uses year-month format: "YYYY-MM"
    assert_eq!(Bucket::Month.strftime_format(), "%Y-%m");
    assert_eq!(Bucket::parse("month"), Some(Bucket::Month));
}

#[test]
fn bucket_default_lookback_days_scales_with_bucket_size() {
    assert!(Bucket::Minute.default_lookback_days() < Bucket::Hour.default_lookback_days());
    assert!(Bucket::Hour.default_lookback_days() < Bucket::Day.default_lookback_days());
    assert!(Bucket::Day.default_lookback_days() < Bucket::Week.default_lookback_days());
    assert!(Bucket::Week.default_lookback_days() < Bucket::Month.default_lookback_days());
    assert_eq!(Bucket::Week.default_lookback_days(), 180);
    assert_eq!(Bucket::Month.default_lookback_days(), 730);
}

#[test]
fn timeline_buckets_by_week() {
    let (pool, _d) = test_pool();
    // Two logs in the same week, one in a different week
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-05T00:00:00Z", "h1", "info", None, "a"), // week 1
            entry("2026-01-06T00:00:00Z", "h1", "info", None, "b"), // same week 1
            entry("2026-01-12T00:00:00Z", "h1", "info", None, "c"), // week 2
        ],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    let pts = timeline(
        &pool,
        Bucket::Week,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0].count, 2);
    assert_eq!(pts[1].count, 1);
    // Bucket labels should contain "W"
    assert!(
        pts[0].bucket.contains('W'),
        "week bucket label must contain 'W': {}",
        pts[0].bucket
    );
}

#[test]
fn timeline_buckets_by_month() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:00Z", "h1", "info", None, "a"),
            entry("2026-01-15T00:00:00Z", "h1", "info", None, "b"),
            entry("2026-02-01T00:00:00Z", "h1", "info", None, "c"),
        ],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    let pts = timeline(
        &pool,
        Bucket::Month,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0].count, 2);
    assert_eq!(pts[1].count, 1);
    // Bucket labels should look like "YYYY-MM"
    assert_eq!(pts[0].bucket, "2026-01");
    assert_eq!(pts[1].bucket, "2026-02");
}

// -----------------------------------------------------------------------------
// timeline_hourly rollup (bead syslog-mcp-kcvq)
// -----------------------------------------------------------------------------

/// Hand-compute the live timeline counts directly off `logs`, bypassing the
/// rollup, so a test can assert rollup == live for the unbounded case.
fn live_hour_counts(pool: &DbPool) -> Vec<(String, i64)> {
    let conn = pool.get().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT strftime('%Y-%m-%dT%H:00:00Z', timestamp) AS b, COUNT(*)
             FROM logs GROUP BY b ORDER BY b ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

#[test]
fn timeline_rollup_matches_live_for_hour_unbounded() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-03-01T00:10:00Z", "h1", "info", Some("nginx"), "a"),
            entry("2026-03-01T00:40:00Z", "h2", "err", Some("sshd"), "b"),
            entry("2026-03-01T01:05:00Z", "h1", "info", None, "c"),
            entry("2026-03-01T01:55:00Z", "h1", "warning", Some("nginx"), "d"),
        ],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    // Unbounded range + full refresh => rollup is an EXACT match for live.
    // (A mid-hour `from`/`to` would legitimately differ in the boundary hour
    // because the rollup can only filter at hour granularity — that imprecision
    // is documented and accepted; see timeline_from_rollup.)
    let rollup = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let live = live_hour_counts(&pool);
    assert_eq!(rollup.len(), live.len());
    for (pt, (b, c)) in rollup.iter().zip(live.iter()) {
        assert_eq!(&pt.bucket, b);
        assert_eq!(pt.count, *c);
    }
}

#[test]
fn timeline_rollup_incremental_add_does_not_double_count() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-03-01T00:10:00Z", "h1", "info", None, "a"),
            entry("2026-03-01T00:20:00Z", "h1", "info", None, "b"),
        ],
    )
    .unwrap();
    let folded = refresh_timeline_rollup(&pool).unwrap();
    assert_eq!(folded, 2, "first refresh folds both rows");
    // A second refresh with nothing new must be a no-op (watermark current).
    let folded2 = refresh_timeline_rollup(&pool).unwrap();
    assert_eq!(folded2, 0, "no-op refresh folds nothing");
    let pts = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0].count, 2, "no double-count after redundant refresh");

    // Insert MORE into the SAME hour; refresh must ADD, not recount.
    insert_logs_batch(
        &pool,
        &[entry("2026-03-01T00:30:00Z", "h1", "info", None, "c")],
    )
    .unwrap();
    let folded3 = refresh_timeline_rollup(&pool).unwrap();
    assert_eq!(folded3, 1, "only the new row is folded");
    let pts = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        pts[0].count, 3,
        "incremental add yields 3, not double-counted"
    );
}

#[test]
fn timeline_rollup_late_arriving_old_timestamp_lands_in_old_bucket() {
    let (pool, _d) = test_pool();
    // Ingest a recent hour first, refresh.
    insert_logs_batch(
        &pool,
        &[entry("2026-03-01T05:00:00Z", "h1", "info", None, "recent")],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    // Now a NEW (higher-id) row arrives carrying an OLD timestamp.
    insert_logs_batch(
        &pool,
        &[entry("2026-03-01T02:00:00Z", "h1", "info", None, "late")],
    )
    .unwrap();
    let folded = refresh_timeline_rollup(&pool).unwrap();
    assert_eq!(folded, 1);
    let pts = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    // The late row must land in its OWN old (02:00) bucket, not the recent one.
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0].bucket, "2026-03-01T02:00:00Z");
    assert_eq!(pts[0].count, 1);
    assert_eq!(pts[1].bucket, "2026-03-01T05:00:00Z");
    assert_eq!(pts[1].count, 1);
}

#[test]
fn timeline_rollup_null_app_groups_as_none() {
    // BLOCKER regression guard: app_name is stored COALESCE(app_name,'') NOT NULL
    // in the rollup; group_by=app_name must project '' back to '<none>' AND the
    // null-app rows must NOT double-count across refreshes (NULL-distinct PK bug).
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-03-01T00:05:00Z", "h1", "info", None, "no-app-1"),
            entry("2026-03-01T00:10:00Z", "h1", "info", None, "no-app-2"),
            entry("2026-03-01T00:15:00Z", "h1", "info", Some("nginx"), "app-1"),
        ],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    // Refresh again to exercise the ON CONFLICT path for the null-app grain —
    // if NULLs were stored as NULL, this would duplicate rows and inflate counts.
    insert_logs_batch(
        &pool,
        &[entry(
            "2026-03-01T00:20:00Z",
            "h1",
            "info",
            None,
            "no-app-3",
        )],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    let pts = timeline(
        &pool,
        Bucket::Hour,
        TimelineGroupBy::AppName,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let none_total: i64 = pts
        .iter()
        .filter(|p| p.group.as_deref() == Some("<none>"))
        .map(|p| p.count)
        .sum();
    let nginx_total: i64 = pts
        .iter()
        .filter(|p| p.group.as_deref() == Some("nginx"))
        .map(|p| p.count)
        .sum();
    assert_eq!(
        none_total, 3,
        "null-app rows group as <none>, no double-count"
    );
    assert_eq!(nginx_total, 1);
}

#[test]
fn prune_timeline_rollup_drops_buckets_older_than_oldest_log() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-03-01T00:00:00Z", "h1", "info", None, "oldest"),
            entry("2026-03-02T00:00:00Z", "h1", "info", None, "mid"),
            entry("2026-03-03T00:00:00Z", "h1", "info", None, "newest"),
        ],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    // Simulate a retention purge of the oldest log.
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM logs WHERE timestamp = '2026-03-01T00:00:00Z'",
            [],
        )
        .unwrap();
    }
    // Before prune, the old bucket still ghosts in the rollup.
    let deleted = prune_timeline_rollup(&pool).unwrap();
    assert_eq!(deleted, 1, "the single ghost bucket is pruned");
    let pts = timeline(
        &pool,
        Bucket::Day,
        TimelineGroupBy::None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(pts.len(), 2, "only buckets >= oldest remaining log survive");
    assert_eq!(pts[0].bucket, "2026-03-02T00:00:00Z");
}

#[test]
fn timeline_rollup_status_reports_watermark() {
    let (pool, _d) = test_pool();
    let before = timeline_rollup_status(&pool).unwrap();
    assert_eq!(before.source_max_id, 0);
    assert!(before.refreshed_at.is_none());
    insert_logs_batch(
        &pool,
        &[entry("2026-03-01T00:00:00Z", "h1", "info", None, "a")],
    )
    .unwrap();
    refresh_timeline_rollup(&pool).unwrap();
    let after = timeline_rollup_status(&pool).unwrap();
    assert!(after.source_max_id > 0);
    assert!(after.refreshed_at.is_some());
}

#[test]
fn silent_hosts_merges_case_variants_before_cutoff() {
    // Regression: silent_hosts read the raw, case-sensitive `hosts` table, so a
    // dormant `shart` identity was flagged as silent even though the live `SHART`
    // kept forwarding. Routing through list_hosts() merges case/FQDN variants
    // (latest last_seen wins) first, so the machine is correctly considered alive.
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-06-19T20:00:00Z", "SHART", "info", None, "live"),
            entry("2026-06-11T06:00:00Z", "shart", "info", None, "old"),
            entry("2026-06-11T06:00:00Z", "STEAMY", "info", None, "old"),
        ],
    )
    .unwrap();
    // hosts.last_seen is stamped with insert-time `now`; pin it explicitly.
    let conn = pool.get().unwrap();
    for (host, last_seen) in [
        ("SHART", "2026-06-19T20:00:00.000Z"),
        ("shart", "2026-06-11T06:00:00.000Z"),
        ("STEAMY", "2026-06-11T06:00:00.000Z"),
    ] {
        conn.execute(
            "UPDATE hosts SET last_seen = ?1 WHERE hostname = ?2",
            rusqlite::params![last_seen, host],
        )
        .unwrap();
    }
    drop(conn);

    let now_unix = chrono::DateTime::parse_from_rfc3339("2026-06-19T21:00:00Z")
        .unwrap()
        .timestamp();
    let silent = silent_hosts(&pool, "2026-06-15T00:00:00.000Z", now_unix).unwrap();
    let names: Vec<String> = silent.iter().map(|h| h.hostname.clone()).collect();

    assert!(
        !names.iter().any(|n| n == "shart"),
        "merged shart is live (SHART forwarding) and must not be flagged silent: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "steamy"),
        "genuinely-dormant STEAMY (lowercased) must still be flagged: {names:?}"
    );
}

#[test]
fn clock_skew_merges_case_variants() {
    // Regression: clock_skew GROUP BY hostname was case-sensitive, so `SHART` and
    // `shart` reported as two separate skew rows. They must merge into one host
    // with summed samples and a sample-weighted average skew.
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[
            entry("2026-01-01T00:00:00Z", "SHART", "info", None, "a"),
            entry("2026-01-01T00:00:00Z", "SHART", "info", None, "b"),
            entry("2026-01-01T00:00:00Z", "shart", "info", None, "c"),
        ],
    )
    .unwrap();
    let conn = pool.get().unwrap();
    // SHART rows skew +10s, the shart row skews +40s.
    for (msg, received_at) in [
        ("a", "2026-01-01T00:00:10Z"),
        ("b", "2026-01-01T00:00:10Z"),
        ("c", "2026-01-01T00:00:40Z"),
    ] {
        conn.execute(
            "UPDATE logs SET received_at = ?1 WHERE message = ?2",
            rusqlite::params![received_at, msg],
        )
        .unwrap();
    }
    drop(conn);

    let result = clock_skew(&pool, "2026-01-01T00:00:00Z", None).unwrap();
    assert_eq!(result.len(), 1, "SHART/shart must collapse to one host");
    assert_eq!(result[0].hostname, "shart");
    assert_eq!(result[0].samples, 3);
    // Sample-weighted: (10 + 10 + 40) / 3 = 20.
    assert!(
        (result[0].avg_skew_secs - 20.0).abs() < 0.5,
        "weighted avg skew should be ~20s, got {}",
        result[0].avg_skew_secs
    );
    assert!((result[0].max_skew_secs - 40.0).abs() < 0.5);
}

#[test]
fn feed_cursor_is_never_below_the_greatest_returned_id() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[entry("2026-01-01T00:00:00Z", "h", "info", None, "one")],
    )
    .unwrap();
    let (first, cursor, more) = feed_logs(&pool, Some(0), None, 100).unwrap();
    assert!(!more);
    assert_eq!(first.len(), 1);
    assert!(cursor >= first.iter().map(|row| row.id).max().unwrap());

    insert_logs_batch(
        &pool,
        &[entry("2026-01-01T00:00:01Z", "h", "info", None, "two")],
    )
    .unwrap();
    let (second, next_cursor, _) = feed_logs(&pool, Some(cursor), None, 100).unwrap();
    assert_eq!(second.len(), 1);
    assert!(next_cursor >= second[0].id);
    let (duplicate, _, _) = feed_logs(&pool, Some(next_cursor), None, 100).unwrap();
    assert!(duplicate.is_empty());
}
