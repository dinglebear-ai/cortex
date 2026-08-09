use super::*;
use crate::config::StorageConfig;
use crate::db::graph;
use crate::db::{DbPool, LogBatchEntry, init_pool, insert_logs_batch};

fn test_pool(name: &str) -> (tempfile::TempDir, DbPool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join(name))).unwrap();
    (dir, pool)
}

fn insert_entity(conn: &rusqlite::Connection, entity_type: &str, key: &str) -> i64 {
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES (?1, ?2, ?2, 'verified')",
        rusqlite::params![entity_type, key],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn syslog_row(ts: &str, host: &str, app: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some(app.to_string()),
        process_id: None,
        message: format!("{app} message"),
        raw: format!("{app} message"),
        source_ip: "10.0.0.5:514".to_string(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(r#"{"source_kind":"syslog-udp"}"#.to_string()),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn search_logs_for_service_instances_uses_service_predicates_not_host_fanout() {
    let (_dir, pool) = test_pool("service-instance-predicates.db");
    let mut plex = syslog_row("2026-01-01T00:01:00Z", "nashost", "plex/plex/plex");
    plex.metadata_json = Some(
        r#"{"source_kind":"agent-docker","agent_docker":{"host":"nashost","container_id":"abc","container_name":"plex","compose_service":"plex","stream":"stdout"}}"#
            .to_string(),
    );
    let mut exact = syslog_row("2026-01-01T00:02:00Z", "nashost", "plex");
    exact.metadata_json = None;
    insert_logs_batch(
        &pool,
        &[
            syslog_row("2026-01-01T00:00:00Z", "nashost", "kernel"),
            plex,
            exact,
            syslog_row("2026-01-01T00:03:00Z", "backuphost", "plex"),
        ],
    )
    .unwrap();

    let rows = search_logs_for_service_instances(
        &pool,
        &["nashost/plex".to_string()],
        None,
        None,
        None,
        50,
    )
    .unwrap();
    // Matches: exact app label, prefixed nested label, structured compose
    // service — all scoped to the instance's host. The kernel row on the
    // same host and the other host's plex row stay out.
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.entry.hostname == "nashost"));
    assert!(
        rows.iter()
            .all(|row| row.entry.app_name.as_deref() != Some("kernel"))
    );
    assert!(
        rows.iter()
            .all(|row| row.inclusion_reason == "service_instance")
    );
    assert!(
        rows.iter()
            .all(|row| row.resolver_status == ResolverStatus::Resolved)
    );
    assert!(rows.iter().all(|row| row.fallback_kind.is_none()));
}

#[test]
fn search_logs_for_service_instances_escapes_like_wildcards() {
    let (_dir, pool) = test_pool("service-instance-like-escape.db");
    insert_logs_batch(
        &pool,
        &[
            // `_` is a LIKE single-char wildcard; an unescaped pattern
            // `my_app/%` would match this row.
            syslog_row("2026-01-01T00:00:00Z", "nashost", "myxapp/x"),
            syslog_row("2026-01-01T00:01:00Z", "nashost", "my_app/x"),
        ],
    )
    .unwrap();
    let rows = search_logs_for_service_instances(
        &pool,
        &["nashost/my_app".to_string()],
        None,
        None,
        None,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry.app_name.as_deref(), Some("my_app/x"));
}

#[test]
fn service_instance_fanout_arms_use_index_search_without_temp_btree() {
    let (_dir, pool) = test_pool("service-instance-plan.db");
    let conn = pool.get().unwrap();
    // Two-arm UNION ALL replica of the search_logs_for_service_instances
    // shape: every arm must be an index search (no `SCAN logs`) and no arm
    // may sort through a temp b-tree — LIMIT pushdown streams each arm off
    // idx_logs_host_time in timestamp order.
    let arm = "SELECT * FROM (SELECT l.id
          FROM logs l
         WHERE l.hostname = ? AND (l.app_name = ? OR l.app_name LIKE ? ESCAPE '\\' \
        OR json_extract(l.metadata_json, '$.agent_docker.compose_service') = ?)
         ORDER BY l.timestamp DESC, l.id DESC
         LIMIT ?)";
    let sql = format!("EXPLAIN QUERY PLAN {arm} UNION ALL {arm}");
    let plan: Vec<String> = conn
        .prepare(&sql)
        .unwrap()
        .query_map(
            rusqlite::params![
                "nashost",
                "plex",
                "plex/%",
                "plex",
                100,
                "backuphost",
                "plex",
                "plex/%",
                "plex",
                100
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|p| p.contains("USING INDEX idx_logs_host_time")),
        "arms must search idx_logs_host_time: {plan:?}"
    );
    assert!(
        !plan.iter().any(|p| p.starts_with("SCAN logs")),
        "no arm may full-scan logs: {plan:?}"
    );
    assert!(
        !plan.iter().any(|p| p.contains("TEMP B-TREE")),
        "no temp b-tree sort allowed: {plan:?}"
    );
}

#[test]
fn search_logs_for_service_instances_rejects_legacy_keys() {
    let (_dir, pool) = test_pool("service-instance-legacy-keys.db");
    insert_logs_batch(
        &pool,
        &[syslog_row("2026-01-01T00:00:00Z", "nashost", "plex")],
    )
    .unwrap();
    // Legacy shapes never split into (host, service) and yield no predicate.
    let rows = search_logs_for_service_instances(
        &pool,
        &["nashost:plex".to_string(), "plex/plex/plex".to_string()],
        None,
        None,
        None,
        50,
    )
    .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn service_instance_fanout_truncates_globally_newest_first_across_arms() {
    let (_dir, pool) = test_pool("service-instance-union-truncation.db");
    // Two instances on different hosts, limit + 2 rows each, sharing the
    // same timestamp set so the merge exercises the tie-break.
    let mut rows = Vec::new();
    for i in 0..8 {
        let ts = format!("2026-01-01T00:00:{i:02}Z");
        rows.push(syslog_row(&ts, "nashost", "plex"));
        rows.push(syslog_row(&ts, "backuphost", "plex"));
    }
    insert_logs_batch(&pool, &rows).unwrap();

    let limit = 6;
    let out = search_logs_for_service_instances(
        &pool,
        &["nashost/plex".to_string(), "backuphost/plex".to_string()],
        None,
        None,
        None,
        limit,
    )
    .unwrap();
    // UNION ALL arms each fetch up to `limit`; the Rust merge must truncate
    // to exactly `limit` rows globally.
    assert_eq!(out.len(), limit);
    // Global newest-first with the (timestamp DESC, id DESC) tie-break.
    for pair in out.windows(2) {
        let (a, b) = (&pair[0].entry, &pair[1].entry);
        assert!(
            a.timestamp > b.timestamp || (a.timestamp == b.timestamp && a.id > b.id),
            "rows must be (timestamp DESC, id DESC): {}#{} then {}#{}",
            a.timestamp,
            a.id,
            b.timestamp,
            b.id
        );
    }
    // The globally newest timestamp wins across both arms: both hosts'
    // :07 rows lead the merged result.
    assert!(out[0].entry.timestamp.ends_with(":07Z"));
    assert!(out[1].entry.timestamp.ends_with(":07Z"));
}

#[test]
fn mixed_case_hostname_does_not_match_canonical_instance_key() {
    let (_dir, pool) = test_pool("service-instance-case-miss.db");
    insert_logs_batch(
        &pool,
        &[syslog_row("2026-01-01T00:00:00Z", "Nashost", "plex")],
    )
    .unwrap();
    // Pins the case-sensitivity limitation: canonical keys are lowercase
    // and the log predicates compare with SQLite's default BINARY
    // collation, so a mixed-case syslog hostname ("Nashost") never matches
    // the canonical instance key ("nashost/plex"). Documented in
    // docs/contracts/investigation-graph.md; hostname case normalization
    // at ingest is tracked separately.
    let rows = search_logs_for_service_instances(
        &pool,
        &["nashost/plex".to_string()],
        None,
        None,
        None,
        50,
    )
    .unwrap();
    assert!(rows.is_empty(), "mixed-case hostname must miss (pinned)");
}

// -- syslog-mcp-csukc: resolve_topic_entities index-backed exact/prefix tier --

#[test]
fn glob_prefix_pattern_escapes_glob_metacharacters() {
    // '*', '?' and '[' are GLOB metacharacters; they must be wrapped in a
    // single-character bracket class so they match literally, not as
    // wildcards, before the trailing '*' prefix wildcard is appended.
    assert_eq!(super::glob_prefix_pattern("plex"), "plex*");
    assert_eq!(super::glob_prefix_pattern("a*b"), "a[*]b*");
    assert_eq!(super::glob_prefix_pattern("a?b"), "a[?]b*");
    assert_eq!(super::glob_prefix_pattern("a[b"), "a[[]b*");
    assert_eq!(super::glob_prefix_pattern("a*b?c[d"), "a[*]b[?]c[[]d*");
}

#[test]
fn resolve_topic_entities_exact_and_prefix_use_indexed_tier() {
    let (_dir, pool) = test_pool("resolve-topic-exact-prefix.db");
    let conn = pool.get().unwrap();
    insert_entity(&conn, graph::ENTITY_TYPE_HOST, "nashost");
    insert_entity(&conn, graph::ENTITY_TYPE_APP, "plex");
    insert_entity(&conn, graph::ENTITY_TYPE_APP, "plexmediaserver");

    let resolved = super::resolve_topic_entities(&conn, &["plex".to_string()]).unwrap();
    let exact = resolved
        .iter()
        .find(|e| e.canonical_key == "plex")
        .expect("exact canonical_key match must resolve");
    assert_eq!(exact.match_kind, "exact");
    assert_eq!(exact.resolver_status, ResolverStatus::Resolved);

    let prefix = resolved
        .iter()
        .find(|e| e.canonical_key == "plexmediaserver")
        .expect("prefix canonical_key match must resolve");
    assert_eq!(prefix.match_kind, "prefix");
    assert_eq!(prefix.resolver_status, ResolverStatus::Ambiguous);
}

#[test]
fn resolve_topic_entities_falls_back_to_label_substring_scan() {
    let (_dir, pool) = test_pool("resolve-topic-label-fallback.db");
    let conn = pool.get().unwrap();
    // canonical_key has no relationship to the search term at all; only the
    // human-readable display_label contains it as a mid-string substring.
    // This must still be found by the (unavoidable) label scan tier.
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES (?1, ?2, ?3, 'verified')",
        rusqlite::params![
            graph::ENTITY_TYPE_APP,
            "backup-job-42",
            "nightly backup-job-42 (plex library)"
        ],
    )
    .unwrap();

    let resolved = super::resolve_topic_entities(&conn, &["plex".to_string()]).unwrap();
    let label = resolved
        .iter()
        .find(|e| e.canonical_key == "backup-job-42")
        .expect("label substring match must still resolve via fallback scan");
    assert_eq!(label.match_kind, "label");
    assert_eq!(label.resolver_status, ResolverStatus::Ambiguous);
}

#[test]
fn resolve_topic_entities_label_scan_escapes_like_wildcards() {
    let (_dir, pool) = test_pool("resolve-topic-label-escape.db");
    let conn = pool.get().unwrap();
    // A topic term containing a literal '%' must not act as a LIKE wildcard
    // in the label-substring fallback tier — it should only match labels
    // that contain the literal character, not everything.
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES (?1, ?2, ?3, 'verified')",
        rusqlite::params![
            graph::ENTITY_TYPE_APP,
            "cpu-100pct-alert",
            "cpu at 100% alert"
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES (?1, ?2, ?3, 'verified')",
        rusqlite::params![
            graph::ENTITY_TYPE_APP,
            "unrelated-entity",
            "totally unrelated"
        ],
    )
    .unwrap();

    let resolved = super::resolve_topic_entities(&conn, &["100%".to_string()]).unwrap();
    assert!(
        resolved
            .iter()
            .any(|e| e.canonical_key == "cpu-100pct-alert"),
        "literal '100%' substring must still match a label containing it"
    );
    assert!(
        !resolved
            .iter()
            .any(|e| e.canonical_key == "unrelated-entity"),
        "unescaped '%' would wildcard-match everything; escaping must prevent that"
    );
}

#[test]
fn resolve_topic_entities_skips_label_scan_when_indexed_tier_fills_cap() {
    let (_dir, pool) = test_pool("resolve-topic-cap-skips-label.db");
    let conn = pool.get().unwrap();
    // 25 canonical_key prefix matches (== PER_TERM_CAP) plus one entity that
    // would only ever be found via the label substring scan. Once the
    // indexed tier alone fills the per-term cap, the label tier must not
    // contribute any additional candidates for this term.
    for i in 0..25 {
        conn.execute(
            "INSERT INTO graph_entities
                (entity_type, canonical_key, display_label, trust_level)
             VALUES (?1, ?2, ?2, 'verified')",
            rusqlite::params![graph::ENTITY_TYPE_APP, format!("svc-{i:02}")],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES (?1, ?2, ?3, 'verified')",
        rusqlite::params![
            graph::ENTITY_TYPE_APP,
            "unrelated-key",
            "totally unrelated but mentions svc- in passing"
        ],
    )
    .unwrap();

    let resolved = super::resolve_topic_entities(&conn, &["svc-".to_string()]).unwrap();
    assert_eq!(resolved.len(), 25, "per-term cap must still be honored");
    assert!(
        !resolved.iter().any(|e| e.canonical_key == "unrelated-key"),
        "label-only candidate must not appear once the indexed tier fills the cap: {:?}",
        resolved
    );
}
