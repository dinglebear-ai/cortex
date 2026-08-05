use super::*;
use crate::config::StorageConfig;
use crate::db::{LogBatchEntry, graph, init_pool, insert_logs_batch};
use crate::inventory::schema::{
    ArtifactRef, ComposeProject, HomelabInventory, InventoryNode, InventoryService, NetworkSegment,
    PortMapping, Provenance, RedactionStatus, ReverseProxyRoute, TrustLevel,
};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn relationship_count(conn: &rusqlite::Connection, rel_type: &str, reason: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*)
           FROM graph_relationships r
           JOIN graph_entities src ON src.id = r.src_entity_id
           JOIN graph_entities dst ON dst.id = r.dst_entity_id
          WHERE r.relationship_type = ?1
            AND r.reason_code = ?2
            AND src.canonical_key <> ''
            AND dst.canonical_key <> ''",
        rusqlite::params![rel_type, reason],
        |row| row.get(0),
    )
    .unwrap()
}

fn provenance(source: &str, kind: &str) -> Provenance {
    Provenance::new(source, kind, "2026-01-01T00:00:00Z".to_string())
}

fn basic_inventory() -> HomelabInventory {
    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.nodes.push(InventoryNode {
        id: "node:dookie".to_string(),
        hostname: "dookie".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:dookie", "source_inventory"),
        roles: Vec::new(),
        ips: vec!["10.1.0.6".to_string()],
        os: Some("Ubuntu".to_string()),
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory
}

fn log_entry(message: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hostname: "writer-test".to_string(),
        facility: Some("daemon".to_string()),
        severity: "info".to_string(),
        app_name: Some("test".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: format!("<14>{message}"),
        source_ip: "127.0.0.1:1514".to_string(),
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
fn inventory_projection_marks_never_built_graph_ready() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-status.db"),
    ))
    .unwrap();

    assert_eq!(
        graph::graph_projection_status(&pool)
            .unwrap()
            .projection_status,
        "never_built"
    );

    project_inventory(&pool, &basic_inventory()).unwrap();

    let status = graph::graph_projection_status(&pool).unwrap();
    assert_eq!(status.projection_status, "ready");
    assert!(status.last_completed_at.is_some());
    assert!(!status.is_degraded);
}

#[test]
fn inventory_projection_keeps_heartbeat_only_hosts_resolvable() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-heartbeat-host.db"),
    ))
    .unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO host_heartbeats (
             host_id, hostname, source_ip, sampled_at, received_at, boot_id,
             uptime_secs, sequence, collection_ms, partial, agent_version,
             os, architecture, metadata_json
         ) VALUES ('host-dookie', 'dookie', '10.1.0.6:1514',
                   '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'boot-a',
                   60, 1, 5, 0, '0.1.0', 'linux', 'x86_64', '{}')",
        [],
    )
    .unwrap();
    let heartbeat_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO host_heartbeats_latest (
             host_id, heartbeat_id, hostname, sampled_at, received_at,
             partial, agent_version, os, architecture, metadata_json
         ) VALUES ('host-dookie', ?1, 'dookie', '2026-01-01T00:00:00Z',
                   '2026-01-01T00:00:00Z', 0, '0.1.0', 'linux', 'x86_64', '{}')",
        [heartbeat_id],
    )
    .unwrap();
    drop(conn);

    let inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    project_inventory(&pool, &inventory).unwrap();

    let conn = pool.get().unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
              WHERE entity_type = 'host' AND canonical_key = 'dookie'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*)
               FROM graph_entity_aliases a
               JOIN graph_entities e ON e.id = a.entity_id
              WHERE e.entity_type = 'host'
                AND e.canonical_key = 'dookie'
                AND a.alias_type = 'hostname'
                AND a.alias_key = 'dookie'"
        ),
        1
    );
}

#[test]
fn project_inventory_does_not_hold_write_lock_while_preparing_projection() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-lock-scope.db"),
    ))
    .unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    let inventory = basic_inventory();
    let projection_pool = pool.clone();
    let (prepared_tx, prepared_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let projection = thread::spawn(move || {
        project_inventory_with_apply_hook(&projection_pool, &inventory, || {
            prepared_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
        })
        .unwrap();
    });

    prepared_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let writer_pool = pool.clone();
    let (insert_done_tx, insert_done_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        let started = Instant::now();
        insert_logs_batch(
            &writer_pool,
            &[log_entry("write while projection prepares")],
        )
        .unwrap();
        insert_done_tx.send(started.elapsed()).unwrap();
    });

    let writer_timeout = Duration::from_secs(5);
    match insert_done_rx.recv_timeout(writer_timeout) {
        Ok(elapsed) => assert!(
            elapsed < writer_timeout,
            "insert waited for projection preparation for {elapsed:?}"
        ),
        Err(error) => {
            continue_tx.send(()).unwrap();
            projection.join().unwrap();
            writer.join().unwrap();
            panic!("insert was blocked by projection preparation: {error}");
        }
    }

    continue_tx.send(()).unwrap();
    projection.join().unwrap();
    writer.join().unwrap();
}

#[test]
fn project_inventory_adds_topology_entities_relationships_and_evidence() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph.db"),
    ))
    .unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.nodes.push(InventoryNode {
        id: "node:squirts".to_string(),
        hostname: "squirts".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:squirts", "source_inventory"),
        roles: vec!["edge".to_string()],
        ips: vec!["10.1.0.8".to_string()],
        os: Some("Ubuntu".to_string()),
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.services.push(InventoryService {
        id: "container:squirts:swag".to_string(),
        name: "swag".to_string(),
        kind: "container".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("docker:squirts", "app_inventory"),
        host: Some("squirts".to_string()),
        image: Some("lscr.io/linuxserver/swag:latest".to_string()),
        status: Some("running".to_string()),
        domains: vec!["example.tootie.tv".to_string()],
        ports: vec![PortMapping {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(443),
            container_port: Some(443),
            protocol: "tcp".to_string(),
        }],
        mounts: Vec::new(),
        env_keys: vec!["URL".to_string()],
        labels: Default::default(),
        details: Default::default(),
    });
    inventory.compose_projects.push(ComposeProject {
        name: "edge".to_string(),
        provenance: provenance("compose:squirts:/opt/edge/compose.yaml", "app_inventory"),
        services: vec!["swag".to_string()],
        compose_files: vec!["/opt/edge/compose.yaml".to_string()],
        domains: vec!["example.tootie.tv".to_string()],
        ports: Vec::new(),
    });
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:example.tootie.tv".to_string(),
        server_names: vec!["example.tootie.tv".to_string()],
        upstreams: vec!["swag:443".to_string()],
        provenance: provenance("swag:squirts:/config/nginx/proxy.conf", "app_inventory"),
    });
    inventory.artifact_refs.push(ArtifactRef {
        id: "artifact:compose:squirts:edge".to_string(),
        kind: "compose".to_string(),
        collector: "raw_configs".to_string(),
        source_host: Some("squirts".to_string()),
        source_path: Some("/opt/edge/compose.yaml".to_string()),
        cache_path: "/home/jmagar/.cortex/inventory/artifacts/edge.yaml".to_string(),
        redaction: RedactionStatus::Redacted,
        byte_len: 42,
        truncated: false,
    });

    let stats = project_inventory(&pool, &inventory).unwrap();
    assert_eq!(stats.source_row_count, 0);
    assert!(stats.entity_count >= 6);
    assert!(stats.relationship_count >= 4);
    assert!(stats.evidence_count >= 4);

    let conn = pool.get().unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'host' AND canonical_key = 'squirts'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entity_aliases
             WHERE alias_type = 'ip' AND alias_key = '10.1.0.8'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entity_aliases
             WHERE alias_type = 'domain' AND alias_key = 'example.tootie.tv'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'compose_project' AND canonical_key = 'squirts:edge'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'reverse_proxy' AND canonical_key = 'proxy:example.tootie.tv'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'domain' AND canonical_key = 'example.tootie.tv'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'config_artifact'
               AND canonical_key = 'artifact:compose:squirts:edge'"
        ),
        1
    );
    assert_eq!(relationship_count(&conn, "runs_on", "inventory_service"), 1);
    assert_eq!(
        relationship_count(&conn, "defines_service", "compose_config"),
        1
    );
    assert_eq!(
        relationship_count(&conn, "routes_to", "reverse_proxy_config"),
        1
    );
    assert_eq!(
        relationship_count(&conn, "exposes_domain", "reverse_proxy_config"),
        1
    );
    assert_eq!(
        relationship_count(&conn, "has_artifact", "config_artifact"),
        1
    );
    // 6 evidence rows: runs_on, instance_of, defines_service, routes_to,
    // exposes_domain, has_artifact.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_relationship_evidence
             WHERE source_kind IN ('source_inventory', 'app_inventory')"
        ),
        6
    );
    // Hard break: inventory services project as service_instance +
    // logical_service, never legacy `service` rows.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service'"
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'service_instance' AND canonical_key = 'squirts/swag'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
             WHERE entity_type = 'logical_service' AND canonical_key = 'swag'"
        ),
        1
    );
}

#[test]
fn project_inventory_preserves_existing_entity_ownership_and_hides_config_paths() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-ownership.db"),
    ))
    .unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES ('host', 'squirts', 'squirts', 'log', '42', 'claimed')",
        [],
    )
    .unwrap();
    drop(conn);

    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.nodes.push(InventoryNode {
        id: "node:squirts".to_string(),
        hostname: "squirts".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:squirts", "source_inventory"),
        roles: Vec::new(),
        ips: Vec::new(),
        os: None,
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.artifact_refs.push(ArtifactRef {
        id: "artifact:compose:squirts:edge".to_string(),
        kind: "compose".to_string(),
        collector: "raw_configs".to_string(),
        source_host: Some("squirts".to_string()),
        source_path: Some("/opt/edge/compose.yaml".to_string()),
        cache_path: "/home/jmagar/.cortex/inventory/raw/inv/edge.txt".to_string(),
        redaction: RedactionStatus::Redacted,
        byte_len: 42,
        truncated: false,
    });
    inventory.compose_projects.push(ComposeProject {
        name: "edge".to_string(),
        provenance: provenance("compose:squirts:/opt/edge/compose.yaml", "app_inventory"),
        services: Vec::new(),
        compose_files: vec!["/opt/edge/compose.yaml".to_string()],
        domains: Vec::new(),
        ports: Vec::new(),
    });

    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    let (source_kind, source_id, trust_level): (String, String, String) = conn
        .query_row(
            "SELECT source_kind, source_id, trust_level
               FROM graph_entities
              WHERE entity_type = 'host' AND canonical_key = 'squirts'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(source_kind, "log");
    assert_eq!(source_id, "42");
    assert_eq!(trust_level, "claimed");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
              WHERE entity_type = 'config_artifact'
                AND (display_label LIKE '%/opt/%' OR display_label LIKE '%/.cortex/%')"
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entity_aliases
              WHERE alias_type = 'path'
                 OR alias_value LIKE '%/opt/%'
                 OR alias_value LIKE '%/.cortex/%'"
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_relationship_evidence
              WHERE source_id LIKE '%/opt/%'
                 OR safe_excerpt LIKE '%/opt/%'"
        ),
        0
    );
}

#[test]
fn project_inventory_does_not_route_to_ambiguous_service_name() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-ambiguous.db"),
    ))
    .unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    for host in ["squirts", "tootie"] {
        inventory.nodes.push(InventoryNode {
            id: format!("node:{host}"),
            hostname: host.to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("ssh:{host}"), "source_inventory"),
            roles: Vec::new(),
            ips: Vec::new(),
            os: None,
            cpu: None,
            memory: None,
            listeners: Vec::new(),
            storage: Vec::new(),
            extras: Default::default(),
        });
        inventory.services.push(InventoryService {
            id: format!("container:{host}:swag"),
            name: "swag".to_string(),
            kind: "container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("docker:{host}"), "app_inventory"),
            host: Some(host.to_string()),
            image: None,
            status: Some("running".to_string()),
            domains: Vec::new(),
            ports: Vec::new(),
            mounts: Vec::new(),
            env_keys: Vec::new(),
            labels: Default::default(),
            details: Default::default(),
        });
    }
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:ambiguous.tootie.tv".to_string(),
        server_names: vec!["ambiguous.tootie.tv".to_string()],
        upstreams: vec!["swag:443".to_string()],
        provenance: provenance("swag:/config/nginx/proxy.conf", "app_inventory"),
    });

    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(relationship_count(&conn, "runs_on", "inventory_service"), 2);
    assert_eq!(
        relationship_count(&conn, "routes_to", "reverse_proxy_config"),
        0
    );
}

#[test]
fn project_inventory_routes_to_service_name_beginning_with_http() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-http-prefix.db"),
    ))
    .unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.nodes.push(InventoryNode {
        id: "node:squirts".to_string(),
        hostname: "squirts".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:squirts", "source_inventory"),
        roles: Vec::new(),
        ips: Vec::new(),
        os: None,
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.services.push(InventoryService {
        id: "container:squirts:http-api".to_string(),
        name: "http-api".to_string(),
        kind: "container".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("docker:squirts", "app_inventory"),
        host: Some("squirts".to_string()),
        image: None,
        status: Some("running".to_string()),
        domains: Vec::new(),
        ports: Vec::new(),
        mounts: Vec::new(),
        env_keys: Vec::new(),
        labels: Default::default(),
        details: Default::default(),
    });
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:http-api.tootie.tv".to_string(),
        server_names: vec!["http-api.tootie.tv".to_string()],
        upstreams: vec!["http://http-api:8080".to_string()],
        provenance: provenance("swag:squirts:/config/nginx/proxy.conf", "app_inventory"),
    });

    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        relationship_count(&conn, "routes_to", "reverse_proxy_config"),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*)
               FROM graph_relationships rel
               JOIN graph_entities dst ON dst.id = rel.dst_entity_id
              WHERE rel.relationship_type = 'routes_to'
                AND dst.entity_type = 'service_instance'
                AND dst.canonical_key = 'squirts/http-api'"
        ),
        1
    );
}

#[test]
fn project_inventory_scopes_compose_projects_and_networks_by_source_host() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-graph-scoped.db"),
    ))
    .unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    for host in ["squirts", "tootie"] {
        inventory.nodes.push(InventoryNode {
            id: format!("node:{host}"),
            hostname: host.to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("ssh:{host}"), "source_inventory"),
            roles: Vec::new(),
            ips: Vec::new(),
            os: None,
            cpu: None,
            memory: None,
            listeners: Vec::new(),
            storage: Vec::new(),
            extras: Default::default(),
        });
        inventory.services.push(InventoryService {
            id: format!("container:{host}:swag"),
            name: "swag".to_string(),
            kind: "container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("docker:{host}"), "app_inventory"),
            host: Some(host.to_string()),
            image: None,
            status: Some("running".to_string()),
            domains: Vec::new(),
            ports: Vec::new(),
            mounts: Vec::new(),
            env_keys: Vec::new(),
            labels: Default::default(),
            details: Default::default(),
        });
        inventory.compose_projects.push(ComposeProject {
            name: "edge".to_string(),
            provenance: provenance(
                &format!("compose:{host}:/opt/edge/compose.yaml"),
                "app_inventory",
            ),
            services: vec!["swag".to_string()],
            compose_files: Vec::new(),
            domains: Vec::new(),
            ports: Vec::new(),
        });
        inventory.networks.push(NetworkSegment {
            name: "bridge".to_string(),
            kind: "docker".to_string(),
            members: vec!["swag".to_string()],
            provenance: provenance(&format!("docker:{host}"), "app_inventory"),
            details: Default::default(),
        });
    }

    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'compose_project'"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'network'"
        ),
        2
    );
    assert_eq!(
        relationship_count(&conn, "defines_service", "compose_config"),
        2
    );
    assert_eq!(
        relationship_count(&conn, "attached_to", "docker_network"),
        2
    );
}

#[test]
fn reprojection_prunes_stale_resolver_instance_of_edges_when_service_moves_hosts() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-reprojection-prune.db"),
    ))
    .unwrap();

    let inventory_with_plex_on = |host: &str| {
        let mut inventory =
            HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
        inventory.nodes.push(InventoryNode {
            id: format!("node:{host}"),
            hostname: host.to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("ssh:{host}"), "source_inventory"),
            roles: Vec::new(),
            ips: Vec::new(),
            os: None,
            cpu: None,
            memory: None,
            listeners: Vec::new(),
            storage: Vec::new(),
            extras: Default::default(),
        });
        inventory.services.push(InventoryService {
            id: format!("container:{host}:plex"),
            name: "plex".to_string(),
            kind: "container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(&format!("docker:{host}"), "app_inventory"),
            host: Some(host.to_string()),
            image: None,
            status: Some("running".to_string()),
            domains: Vec::new(),
            ports: Vec::new(),
            mounts: Vec::new(),
            env_keys: Vec::new(),
            labels: Default::default(),
            details: Default::default(),
        });
        inventory
    };

    project_inventory(&pool, &inventory_with_plex_on("tootie")).unwrap();
    {
        let conn = pool.get().unwrap();
        assert_eq!(
            relationship_count(&conn, "instance_of", "resolver_instance_of"),
            1
        );
    }

    // Plex moves to shart: re-projection must not leak the stale
    // tootie/plex instance_of edge or leave orphan evidence behind.
    project_inventory(&pool, &inventory_with_plex_on("shart")).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        relationship_count(&conn, "instance_of", "resolver_instance_of"),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*)
               FROM graph_relationships r
               JOIN graph_entities src ON src.id = r.src_entity_id
              WHERE r.relationship_type = 'instance_of'
                AND src.canonical_key = 'tootie/plex'"
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities
              WHERE entity_type = 'service_instance' AND canonical_key = 'tootie/plex'"
        ),
        0
    );
    // No orphan evidence: every evidence row must reference a live edge.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_relationship_evidence e
              WHERE NOT EXISTS (
                  SELECT 1 FROM graph_relationships r WHERE r.id = e.relationship_id
              )"
        ),
        0
    );
}

#[test]
fn double_projection_with_log_and_inventory_instance_of_leaves_no_orphans() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-log-instance-overlap.db"),
    ))
    .unwrap();

    // Log-driven instance_of: agent-docker structured metadata row for
    // (tootie/plex, plex), projected by the log extraction path.
    let mut entry = log_entry("Plex started");
    entry.hostname = "tootie".to_string();
    entry.app_name = Some("plex".to_string());
    entry.metadata_json = Some(
        r#"{"source_kind":"agent-docker","agent_docker":{"host":"tootie","container_id":"abcdef1234567890","container_name":"plex","compose_project":"plex","compose_service":"plex","stream":"stdout"}}"#
            .to_string(),
    );
    insert_logs_batch(&pool, &[entry]).unwrap();
    graph::refresh_graph_projection(&pool).unwrap();

    // Inventory-driven instance_of for the SAME (instance, logical) pair.
    let mut inventory =
        HomelabInventory::empty("inv-test".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.services.push(InventoryService {
        id: "container:tootie:plex".to_string(),
        name: "plex".to_string(),
        kind: "container".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("docker:tootie", "app_inventory"),
        host: Some("tootie".to_string()),
        image: None,
        status: Some("running".to_string()),
        domains: Vec::new(),
        ports: Vec::new(),
        mounts: Vec::new(),
        env_keys: Vec::new(),
        labels: Default::default(),
        details: Default::default(),
    });
    project_inventory(&pool, &inventory).unwrap();

    let snapshot = |conn: &rusqlite::Connection| -> (i64, i64) {
        (
            count(
                conn,
                "SELECT COUNT(*) FROM graph_relationships
                  WHERE relationship_type = 'instance_of'",
            ),
            count(conn, "SELECT COUNT(*) FROM graph_relationship_evidence"),
        )
    };
    let (rels_first, evidence_first) = {
        let conn = pool.get().unwrap();
        snapshot(&conn)
    };

    // Double projection: re-project the same inventory. The log-driven and
    // inventory-driven paths use distinct relationship_key shapes BY DESIGN,
    // so we assert stability and zero orphans — NOT exactly-one edge.
    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    let (rels_second, evidence_second) = snapshot(&conn);
    assert_eq!(
        rels_first, rels_second,
        "instance_of rowcount must be stable"
    );
    assert_eq!(
        evidence_first, evidence_second,
        "evidence rowcount must be stable"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_relationship_evidence e
              WHERE NOT EXISTS (
                  SELECT 1 FROM graph_relationships r WHERE r.id = e.relationship_id
              )"
        ),
        0,
        "no evidence row may reference a dead relationship"
    );
}

#[test]
fn inventory_projection_links_service_instance_to_host_storage_compose_and_route() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-service-instance.db"),
    ))
    .unwrap();
    let mut inventory =
        HomelabInventory::empty("plex-proof".to_string(), "2026-01-01T00:00:00Z".to_string());
    inventory.nodes.push(InventoryNode {
        id: "node:tootie".to_string(),
        hostname: "tootie".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:tootie", "source_inventory"),
        roles: Vec::new(),
        ips: vec!["100.120.242.29".to_string()],
        os: Some("Unraid".to_string()),
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.services.push(InventoryService {
        id: "service:tootie:plex".to_string(),
        name: "plex".to_string(),
        kind: "container".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("docker:tootie", "app_inventory"),
        host: Some("tootie".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        status: Some("running".to_string()),
        domains: vec!["plex.tootie.tv".to_string()],
        ports: vec![PortMapping {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(32400),
            container_port: Some(32400),
            protocol: "tcp".to_string(),
        }],
        mounts: vec![crate::inventory::schema::MountRef {
            source: Some("/mnt/user/media".to_string()),
            target: "/media".to_string(),
            read_only: false,
        }],
        env_keys: Vec::new(),
        labels: Default::default(),
        details: Default::default(),
    });
    inventory.compose_projects.push(ComposeProject {
        name: "plex".to_string(),
        provenance: provenance("compose:tootie:/opt/plex/compose.yaml", "app_inventory"),
        services: vec!["plex".to_string()],
        compose_files: Vec::new(),
        domains: Vec::new(),
        ports: Vec::new(),
    });
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:plex.tootie.tv".to_string(),
        server_names: vec!["plex.tootie.tv".to_string()],
        upstreams: vec!["plex:32400".to_string()],
        provenance: provenance("swag:tootie:/config/nginx/plex.conf", "app_inventory"),
    });
    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service_instance' AND canonical_key = 'tootie/plex'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'logical_service' AND canonical_key = 'plex'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service'"
        ),
        0
    );
    assert_eq!(
        relationship_count(&conn, "instance_of", "resolver_instance_of"),
        1
    );
    assert_eq!(relationship_count(&conn, "runs_on", "inventory_service"), 1);
    assert_eq!(
        relationship_count(&conn, "defines_service", "compose_config"),
        1
    );
    assert_eq!(
        relationship_count(&conn, "routes_to", "reverse_proxy_config"),
        1
    );
    assert_eq!(relationship_count(&conn, "mounts", "storage_probe"), 1);
    // Every service edge terminates at the service_instance, never at a
    // legacy `service` node.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*)
               FROM graph_relationships rel
               JOIN graph_entities dst ON dst.id = rel.dst_entity_id
              WHERE rel.relationship_type IN ('defines_service', 'routes_to')
                AND dst.entity_type <> 'service_instance'"
        ),
        0
    );
}
