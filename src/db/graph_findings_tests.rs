use super::*;
use crate::config::StorageConfig;
use crate::inventory::schema::{
    HomelabInventory, InventoryNode, InventoryService, MountRef, Provenance, ReverseProxyRoute,
    TrustLevel,
};

fn test_pool() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("graph-findings.db"));
    let pool = crate::db::init_pool(&storage).unwrap();
    (pool, dir)
}

fn provenance(source: &str) -> Provenance {
    Provenance::new(source, "app_inventory", "2026-01-01T00:00:00Z".to_string())
}

fn seed_inventory(pool: &DbPool, services: usize) {
    let mut inventory = HomelabInventory::empty(
        "graph-findings-test".to_string(),
        "2026-01-01T00:00:00Z".to_string(),
    );
    inventory.nodes.push(InventoryNode {
        id: "node:edgehost".to_string(),
        hostname: "edgehost".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:edgehost"),
        roles: Vec::new(),
        ips: Vec::new(),
        os: None,
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    for idx in 0..services {
        let name = format!("svc-{idx}");
        inventory.services.push(InventoryService {
            id: format!("container:edgehost:{name}"),
            name: name.clone(),
            kind: "container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance("docker:edgehost"),
            host: Some("edgehost".to_string()),
            image: None,
            status: Some("running".to_string()),
            domains: Vec::new(),
            ports: Vec::new(),
            mounts: vec![MountRef {
                source: Some("/var/run/docker.sock".to_string()),
                target: "/var/run/docker.sock".to_string(),
                read_only: false,
            }],
            env_keys: Vec::new(),
            labels: Default::default(),
            details: Default::default(),
        });
    }
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:one.example.test".to_string(),
        server_names: vec!["one.example.test".to_string()],
        upstreams: vec!["svc-0:80".to_string()],
        provenance: provenance("swag:edgehost:/redacted.conf"),
    });
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    crate::db::graph::refresh_graph_projection(pool).unwrap();
    crate::db::graph_inventory::project_inventory(pool, &inventory).unwrap();
}

fn seed_inventory_without_route_target(pool: &DbPool) {
    let mut inventory = HomelabInventory::empty(
        "graph-findings-no-target-test".to_string(),
        "2026-01-01T00:00:00Z".to_string(),
    );
    inventory.nodes.push(InventoryNode {
        id: "node:edgehost".to_string(),
        hostname: "edgehost".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:edgehost"),
        roles: Vec::new(),
        ips: Vec::new(),
        os: None,
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.reverse_proxies.push(ReverseProxyRoute {
        id: "proxy:orphan.example.test".to_string(),
        server_names: vec!["orphan.example.test".to_string()],
        upstreams: vec!["missing-service:80".to_string()],
        provenance: provenance("swag:edgehost:/redacted.conf"),
    });
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    crate::db::graph::refresh_graph_projection(pool).unwrap();
    crate::db::graph_inventory::project_inventory(pool, &inventory).unwrap();
}

#[test]
fn public_route_findings_return_route_target_and_evidence() {
    let (pool, _dir) = test_pool();
    seed_inventory(&pool, 2);

    let rows = list_public_route_findings(&pool, 10).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].domain_key, "one.example.test");
    assert_eq!(rows[0].proxy_key, "proxy:one.example.test");
    // Canonical service-instance key (`host/service`), never `host:service`.
    assert_eq!(rows[0].service_key.as_deref(), Some("edgehost/svc-0"));
    assert!(rows[0].exposes_evidence_id.is_some());
    assert!(rows[0].routes_evidence_id.is_some());
}

#[test]
fn public_route_findings_return_domain_without_route_target() {
    let (pool, _dir) = test_pool();
    seed_inventory_without_route_target(&pool);

    let rows = list_public_route_findings(&pool, 10).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].domain_key, "orphan.example.test");
    assert_eq!(rows[0].proxy_key, "proxy:orphan.example.test");
    assert!(rows[0].service_key.is_none());
    assert!(rows[0].routes_evidence_id.is_none());
    assert!(rows[0].exposes_evidence_id.is_some());
}

#[test]
fn mount_findings_are_bounded_and_use_relationship_type_index() {
    let (pool, _dir) = test_pool();
    seed_inventory(&pool, 25);

    let rows = list_mount_relationship_findings(&pool, 5).unwrap();
    let plan = relationship_type_query_plan(&pool, crate::db::graph::REL_MOUNTS).unwrap();

    assert_eq!(rows.len(), 5);
    assert!(
        plan.iter()
            .any(|row| row.contains("idx_graph_relationships_type_seen")),
        "expected type-specific graph index in query plan: {plan:?}"
    );
    assert!(
        !plan.iter().any(|row| row == "SCAN graph_relationships"),
        "findings query must not broad-scan graph relationships: {plan:?}"
    );
}
