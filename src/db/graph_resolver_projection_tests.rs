use super::*;
use crate::config::StorageConfig;
use crate::db::{DbPool, graph, init_pool};

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
fn graph_walk_service_topic_traverses_proof_edges_and_caps_results() {
    let (_dir, pool) = test_pool("service-topic-walk.db");
    let conn = pool.get().unwrap();
    let logical = insert_entity(&conn, "logical_service", "plex");
    let instance = insert_entity(&conn, "service_instance", "nashost/plex");
    let host = insert_entity(&conn, "host", "nashost");
    let app = insert_entity(&conn, "app", "kernel");
    insert_rel(&conn, instance, logical, "instance_of");
    insert_rel(&conn, instance, host, "runs_on");
    // Broad log-identity edge from the host: must NOT be traversed.
    insert_rel(&conn, app, host, "emitted_by");

    let (entities, truncated) = graph_walk_service_topic(&conn, &["plex".to_string()], 3).unwrap();
    let walked = keys(&entities);
    assert!(walked.contains(&"plex".to_string()));
    assert!(walked.contains(&"nashost/plex".to_string()));
    assert!(walked.contains(&"nashost".to_string()));
    assert!(
        !walked.contains(&"kernel".to_string()),
        "service-topic walk must not traverse broad log-identity edges: {walked:?}"
    );
    // Exact reach: seed + instance + host, and nothing else. (A `<= CAP`
    // assertion would be vacuous on a 4-node fixture.)
    assert_eq!(entities.len(), 3, "walk must reach exactly {walked:?}");
    assert!(!truncated, "small fixture must not report truncation");
}

#[test]
fn graph_walk_service_topic_reports_truncated_when_cap_hit() {
    let (_dir, pool) = test_pool("service-topic-walk-cap.db");
    let conn = pool.get().unwrap();
    let seed = insert_entity(&conn, "logical_service", "plex");
    // One more neighbor than GRAPH_SERVICE_TOPIC_ENTITY_CAP so the walk
    // (seed + neighbors) exceeds the cap and must report truncation.
    // Batched in one transaction — one autocommit per row makes this ~1000x
    // slower for no benefit in a test fixture.
    conn.execute_batch("BEGIN;").unwrap();
    for i in 0..(GRAPH_SERVICE_TOPIC_ENTITY_CAP + 1) {
        let neighbor = insert_entity(&conn, "host", &format!("host-{i}"));
        insert_rel(&conn, seed, neighbor, "runs_on");
    }
    conn.execute_batch("COMMIT;").unwrap();

    let (entities, truncated) = graph_walk_service_topic(&conn, &["plex".to_string()], 1).unwrap();
    assert_eq!(
        entities.len(),
        GRAPH_SERVICE_TOPIC_ENTITY_CAP,
        "result must be capped at GRAPH_SERVICE_TOPIC_ENTITY_CAP"
    );
    assert!(
        truncated,
        "walk reaching more than the cap must report truncated=true"
    );
}

#[test]
fn stale_service_topology_cleanup_removes_old_canonical_rows() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("stale-service-cleanup.db"),
    ))
    .unwrap();
    let mut conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES
            ('service', 'nashost:plex', 'plex', 'log', 'fixture', 'inferred'),
            ('service', 'nashost:plex:plex', 'nashost/plex/plex', 'log', 'fixture', 'inferred'),
            ('app', 'plex/plex/plex', 'plex/plex/plex', 'log', 'fixture', 'claimed')",
        [],
    )
    .unwrap();
    // Seed more legacy rows than one cleanup chunk (2000) so the chunked
    // delete loop must iterate, plus dependent alias/relationship/evidence
    // rows to exercise every phase, and one unrelated host that must survive.
    {
        let tx = conn.transaction().unwrap();
        for i in 0..2_500 {
            tx.execute(
                "INSERT INTO graph_entities
                    (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
                 VALUES ('service', ?1, 'svc', 'log', 'fixture', 'inferred')",
                [format!("host{i}:svc{i}")],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO graph_entities
                (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
             VALUES ('host', 'nashost', 'nashost', 'log', 'fixture', 'verified')",
            [],
        )
        .unwrap();
        tx.execute_batch(
            "INSERT INTO graph_entity_aliases
                (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
             SELECT id, 'app_name', canonical_key, canonical_key, 'log', 'inferred'
               FROM graph_entities WHERE entity_type = 'service';
             INSERT INTO graph_relationships
                (relationship_key, src_entity_id, dst_entity_id, relationship_type,
                 reason_code, trust_level, confidence)
             SELECT s.id || ':runs_on:' || h.id, s.id, h.id, 'runs_on',
                    'docker_service_label', 'inferred', 0.5
               FROM graph_entities s, graph_entities h
              WHERE s.entity_type = 'service' AND h.canonical_key = 'nashost';
             INSERT INTO graph_relationship_evidence
                (relationship_id, evidence_key, source_kind, source_id,
                 observed_at, reason_code, trust_level)
             SELECT id, 'ev:' || id, 'log', 'fixture',
                    '2026-01-01T00:00:00Z', 'docker_service_label', 'inferred'
               FROM graph_relationships;",
        )
        .unwrap();
        tx.commit().unwrap();
    }
    // Release the fixture connection: the cleanup takes its own per chunk.
    drop(conn);
    cleanup_legacy_service_topology(&pool).unwrap();
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_entities
              WHERE entity_type = 'service'
                 OR (entity_type = 'app' AND canonical_key = 'plex/plex/plex')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    // Dependent rows are fully gone across every chunked phase.
    for table in [
        "graph_entity_aliases",
        "graph_relationships",
        "graph_relationship_evidence",
    ] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "{table} must be emptied by cleanup");
    }
    // The unrelated host entity survives the cleanup.
    let hosts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_entities WHERE canonical_key = 'nashost'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hosts, 1);
}
