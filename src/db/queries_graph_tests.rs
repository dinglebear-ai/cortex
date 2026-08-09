//! Tests for graph-anchored traversal and log fan-out
//! (`graph_walk_n_hops`, `search_logs_from_graph_related_entities`).

use super::*;
use crate::db::graph::{
    self, GRAPH_WALK_MAX_DEPTH, REL_RUNS_ON, graph_walk_n_hops, refresh_graph_projection,
};
use crate::db::{LogBatchEntry, init_pool, insert_logs_batch};

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

fn insert_rel(conn: &rusqlite::Connection, src: i64, dst: i64, rel: &str) {
    conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, 'log_app_name', 'inferred', 0.5,
                 '2026-01-01T00:00:00Z')",
        rusqlite::params![format!("{src}:{rel}:{dst}"), src, dst, rel],
    )
    .unwrap();
}

fn keys(entities: &[graph::GraphWalkEntity]) -> Vec<String> {
    let mut k: Vec<String> = entities.iter().map(|e| e.canonical_key.clone()).collect();
    k.sort();
    k
}

#[test]
fn graph_walk_single_hop_returns_seed_and_neighbour() {
    let (_d, pool) = test_pool("walk-1hop.db");
    let conn = pool.get().unwrap();
    let a = insert_entity(&conn, graph::ENTITY_TYPE_HOST, "host-a");
    let b = insert_entity(&conn, graph::ENTITY_TYPE_APP, "app-b");
    insert_rel(&conn, b, a, REL_RUNS_ON);

    let reached = graph_walk_n_hops(&conn, &["host-a".to_string()], 1).unwrap();
    assert_eq!(keys(&reached), vec!["app-b", "host-a"]);
}

#[test]
fn graph_walk_two_hops_respects_depth() {
    let (_d, pool) = test_pool("walk-2hop.db");
    let conn = pool.get().unwrap();
    let a = insert_entity(&conn, graph::ENTITY_TYPE_HOST, "a");
    let b = insert_entity(&conn, graph::ENTITY_TYPE_APP, "b");
    let c = insert_entity(&conn, graph::ENTITY_TYPE_APP, "c");
    insert_rel(&conn, a, b, REL_RUNS_ON);
    insert_rel(&conn, b, c, REL_RUNS_ON);

    // depth 1 reaches only the direct neighbour.
    let depth1 = graph_walk_n_hops(&conn, &["a".to_string()], 1).unwrap();
    assert_eq!(keys(&depth1), vec!["a", "b"]);

    // depth 2 reaches the far node too.
    let depth2 = graph_walk_n_hops(&conn, &["a".to_string()], 2).unwrap();
    assert_eq!(keys(&depth2), vec!["a", "b", "c"]);
}

#[test]
fn graph_walk_terminates_on_cycle() {
    let (_d, pool) = test_pool("walk-cycle.db");
    let conn = pool.get().unwrap();
    let a = insert_entity(&conn, graph::ENTITY_TYPE_HOST, "a");
    let b = insert_entity(&conn, graph::ENTITY_TYPE_APP, "b");
    let c = insert_entity(&conn, graph::ENTITY_TYPE_APP, "c");
    // 3-cycle: a → b → c → a
    insert_rel(&conn, a, b, REL_RUNS_ON);
    insert_rel(&conn, b, c, REL_RUNS_ON);
    insert_rel(&conn, c, a, REL_RUNS_ON);

    // UNION (not UNION ALL) dedups visited rows, so the walk converges.
    let reached = graph_walk_n_hops(&conn, &["a".to_string()], GRAPH_WALK_MAX_DEPTH).unwrap();
    assert_eq!(keys(&reached), vec!["a", "b", "c"]);
}

#[test]
fn graph_walk_clamps_depth_and_handles_empty_seed() {
    let (_d, pool) = test_pool("walk-clamp.db");
    let conn = pool.get().unwrap();
    let a = insert_entity(&conn, graph::ENTITY_TYPE_HOST, "a");
    let b = insert_entity(&conn, graph::ENTITY_TYPE_APP, "b");
    insert_rel(&conn, a, b, REL_RUNS_ON);

    // depth 0 is clamped up to 1 (still reaches the direct neighbour).
    let clamped_low = graph_walk_n_hops(&conn, &["a".to_string()], 0).unwrap();
    assert_eq!(keys(&clamped_low), vec!["a", "b"]);

    // Oversized depth is clamped to the ceiling without error.
    let clamped_high = graph_walk_n_hops(&conn, &["a".to_string()], 250).unwrap();
    assert_eq!(keys(&clamped_high), vec!["a", "b"]);

    // Empty seed set returns empty.
    assert!(graph_walk_n_hops(&conn, &[], 3).unwrap().is_empty());
}

#[test]
fn graph_walk_uses_relationship_indexes() {
    let (_d, pool) = test_pool("walk-plan.db");
    let conn = pool.get().unwrap();
    let plan: Vec<String> = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             WITH RECURSIVE graph_walk(entity_id, depth) AS (
                 SELECT id, 0 FROM graph_entities WHERE canonical_key IN ('a')
                 UNION
                 SELECT CASE WHEN r.src_entity_id = gw.entity_id
                             THEN r.dst_entity_id ELSE r.src_entity_id END,
                        gw.depth + 1
                 FROM graph_relationships r
                 JOIN graph_walk gw
                   ON r.src_entity_id = gw.entity_id OR r.dst_entity_id = gw.entity_id
                 WHERE gw.depth < 6
             )
             SELECT DISTINCT e.entity_type, e.canonical_key
             FROM graph_entities e JOIN graph_walk gw ON e.id = gw.entity_id",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    // The recursive relationship join must be index-served on src/dst entity id,
    // never a full table scan of graph_relationships.
    assert!(
        !plan.iter().any(|p| p == "SCAN graph_relationships"),
        "recursive hop must not full-scan graph_relationships: {plan:?}"
    );
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

fn agent_command_row(ts: &str, host: &str, session: &str, cwd: &str) -> LogBatchEntry {
    let mut row = syslog_row(ts, host, "claude");
    row.source_ip = format!("agent-command://{host}/claude/{session}");
    row.ai_tool = Some("claude".to_string());
    row.ai_project = Some(cwd.to_string());
    row.ai_session_id = Some(session.to_string());
    row.metadata_json = Some(format!(
        r#"{{"source_kind":"agent-command","agent_command":{{"cwd":"{cwd}"}}}}"#
    ));
    row
}

#[test]
fn search_logs_from_graph_fans_out_from_session_to_host_logs() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let (_d, pool) = test_pool("fanout.db");

    insert_logs_batch(
        &pool,
        &[
            // Session sess-7 ran commands on devhost (links ai_session → host).
            agent_command_row(
                "2026-01-01T00:00:00Z",
                "devhost",
                "sess-7",
                "/home/jmagar/workspace/cortex",
            ),
            // Plain syslog on the same host — should be reached via the host edge.
            syslog_row("2026-01-01T00:01:00Z", "devhost", "swag"),
            // Unrelated host — must NOT be returned.
            syslog_row("2026-01-01T00:02:00Z", "edgehost", "authelia"),
        ],
    )
    .unwrap();
    refresh_graph_projection(&pool).unwrap();

    // Seed from the AI session entity; traversal reaches host:devhost.
    let session_key = "cortex:claude:sess-7".to_string();
    let logs = search_logs_from_graph_related_entities(
        &pool,
        &[session_key],
        2,
        None,
        None,
        None,
        100,
        HostFanoutScope::WalkReached,
    )
    .unwrap();

    let hosts: std::collections::HashSet<&str> = logs.iter().map(|l| l.hostname.as_str()).collect();
    assert!(
        hosts.contains("devhost"),
        "must fan out to devhost logs: {hosts:?}"
    );
    assert!(
        !hosts.contains("edgehost"),
        "unrelated host must be excluded"
    );
    assert!(
        logs.iter().any(|l| l.app_name.as_deref() == Some("swag")),
        "the swag syslog row on the related host must be returned"
    );
}

#[test]
fn search_logs_from_graph_respects_source_kind_filter() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let (_d, pool) = test_pool("fanout-source.db");

    insert_logs_batch(
        &pool,
        &[
            agent_command_row(
                "2026-01-01T00:00:00Z",
                "devhost",
                "sess-7",
                "/home/jmagar/workspace/cortex",
            ),
            syslog_row("2026-01-01T00:01:00Z", "devhost", "swag"),
        ],
    )
    .unwrap();
    refresh_graph_projection(&pool).unwrap();

    // Restrict to syslog-udp only → the agent-command row is filtered out.
    let logs = search_logs_from_graph_related_entities(
        &pool,
        &["cortex:claude:sess-7".to_string()],
        2,
        None,
        None,
        Some(&[SourceKind::SyslogUdp]),
        100,
        HostFanoutScope::WalkReached,
    )
    .unwrap();
    assert!(!logs.is_empty(), "syslog row should survive the filter");
    assert!(
        logs.iter()
            .all(|l| !l.source_ip.starts_with("agent-command://")),
        "agent-command rows must be excluded by the source_kind filter"
    );
}

#[test]
fn search_logs_from_graph_empty_for_unknown_seed() {
    let (_d, pool) = test_pool("fanout-empty.db");
    let logs = search_logs_from_graph_related_entities(
        &pool,
        &["does-not-exist".to_string()],
        2,
        None,
        None,
        None,
        100,
        HostFanoutScope::WalkReached,
    )
    .unwrap();
    assert!(logs.is_empty());
}

#[test]
fn topic_resolving_logical_service_with_no_instances_reports_degraded() {
    let (_dir, pool) = test_pool("topic-zero-instance-degraded.db");
    // Stale/unbuilt projection: the logical service entity exists but has no
    // `instance_of` service instances (e.g. right after migration 41 before
    // `cortex graph rebuild` runs). The resolved entity must surface as
    // degraded so the empty service timeline is explained, never silent.
    {
        let conn = pool.get().unwrap();
        insert_entity(&conn, graph::ENTITY_TYPE_LOGICAL_SERVICE, "plex");
    }
    let inputs =
        topic_correlate_inputs(&pool, &["plex".to_string()], 2, None, None, None, 100).unwrap();
    let entity = inputs
        .resolved
        .iter()
        .find(|e| e.entity_type == graph::ENTITY_TYPE_LOGICAL_SERVICE && e.canonical_key == "plex")
        .expect("logical service must resolve");
    assert_eq!(entity.resolver_status, ResolverStatus::Degraded);
    assert!(inputs.logs.is_empty(), "no instances → no service fan-out");
}

#[test]
fn topic_correlate_app_seed_does_not_fan_out_to_whole_host() {
    let (_dir, pool) = test_pool("topic-app-seed-no-host-fanout.db");
    // Bare `plex` app label (no agent-docker metadata) plus an unrelated
    // kernel row on the same host.
    insert_logs_batch(
        &pool,
        &[
            syslog_row("2026-01-01T00:00:00Z", "nashost", "plex"),
            syslog_row("2026-01-01T00:01:00Z", "nashost", "kernel"),
        ],
    )
    .unwrap();
    // Graph: app:plex —emitted_by→ host:nashost (log-identity edge).
    {
        let conn = pool.get().unwrap();
        let app = insert_entity(&conn, graph::ENTITY_TYPE_APP, "plex");
        let host = insert_entity(&conn, graph::ENTITY_TYPE_HOST, "nashost");
        insert_rel(&conn, app, host, graph::REL_EMITTED_BY);
    }

    let inputs =
        topic_correlate_inputs(&pool, &["plex".to_string()], 2, None, None, None, 100).unwrap();
    // The topic resolves to the raw app entity and the walk reaches
    // host:nashost, but the transitively reached host must never drive
    // host-wide log inclusion labelled `resolved`.
    assert!(
        inputs
            .resolved
            .iter()
            .any(|entity| entity.entity_type == graph::ENTITY_TYPE_APP
                && entity.canonical_key == "plex"),
        "topic must resolve the raw app entity: {:?}",
        inputs.resolved
    );
    assert!(
        !inputs.logs.iter().any(|row| {
            row.entry.app_name.as_deref() == Some("kernel")
                && row.resolver_status == ResolverStatus::Resolved
        }),
        "unrelated kernel row on the host must not be included as resolved: {:?}",
        inputs
            .logs
            .iter()
            .map(|row| (
                row.entry.app_name.clone(),
                row.resolver_status,
                row.fallback_kind.clone()
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ambiguous_prefix_candidates_surface_without_log_fanout() {
    let (_dir, pool) = test_pool("topic-ambiguous-prefix.db");
    insert_logs_batch(
        &pool,
        &[syslog_row(
            "2026-01-01T00:00:00Z",
            "nashost",
            "plexmediaserver",
        )],
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        insert_entity(&conn, graph::ENTITY_TYPE_LOGICAL_SERVICE, "plexmediaserver");
    }
    // "plexmedia" prefix-matches logical_service:plexmediaserver: the
    // candidate must surface as ambiguous but contribute ZERO log fan-out.
    let inputs =
        topic_correlate_inputs(&pool, &["plexmedia".to_string()], 2, None, None, None, 100)
            .unwrap();
    let entity = inputs
        .resolved
        .iter()
        .find(|e| e.canonical_key == "plexmediaserver")
        .expect("prefix candidate must surface");
    assert_eq!(entity.match_kind, "prefix");
    assert_eq!(entity.resolver_status, ResolverStatus::Ambiguous);
    assert!(
        inputs.logs.is_empty(),
        "ambiguous candidates must contribute zero log fan-out"
    );
}
