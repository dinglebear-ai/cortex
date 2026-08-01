use super::*;
use crate::inventory::config::InventoryConfig;
use crate::inventory::schema::HomelabInventory;
use crate::inventory::storage::{InventoryPaths, ensure_private_dir, write_json_private};
use chrono::Utc;
use std::time::Duration as StdDuration;

#[test]
fn status_reports_missing_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = InventoryConfig {
        root: dir.path().join("inventory"),
        compose_paths: vec![],
        proxy_paths: vec![],
        adguard_paths: vec![],
        ssh_config: None,
        ssh_hosts: vec![],
        project_roots: vec![],
        docker_hosts: vec![],
        unraid_url: None,
        unraid_api_key: None,
        unifi_url: None,
        unifi_api_key: None,
        media_services: vec![],
        collection_deadline: StdDuration::from_secs(1),
        collector_deadline: StdDuration::from_secs(1),
        probe_deadline: StdDuration::from_secs(1),
    };
    let status = inventory_status(&cfg);
    assert_eq!(status.status, "missing");
    assert!(status.is_stale);
}

#[test]
fn status_reports_stale_available_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = InventoryConfig {
        root: dir.path().join("inventory"),
        compose_paths: vec![],
        proxy_paths: vec![],
        adguard_paths: vec![],
        ssh_config: None,
        ssh_hosts: vec![],
        project_roots: vec![],
        docker_hosts: vec![],
        unraid_url: None,
        unraid_api_key: None,
        unifi_url: None,
        unifi_api_key: None,
        media_services: vec![],
        collection_deadline: StdDuration::from_secs(1),
        collector_deadline: StdDuration::from_secs(1),
        probe_deadline: StdDuration::from_secs(1),
    };
    let paths = InventoryPaths::new(cfg.root.clone());
    let inventory = HomelabInventory::empty("run".to_string(), "2000-01-01T00:00:00Z".to_string());
    write_json_private(&paths.normalized_json, &inventory).unwrap();

    let status = inventory_status(&cfg);
    assert_eq!(status.status, "available_stale");
    assert!(status.is_stale);
}

#[test]
fn status_reports_fresh_available_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = InventoryConfig {
        root: dir.path().join("inventory"),
        compose_paths: vec![],
        proxy_paths: vec![],
        adguard_paths: vec![],
        ssh_config: None,
        ssh_hosts: vec![],
        project_roots: vec![],
        docker_hosts: vec![],
        unraid_url: None,
        unraid_api_key: None,
        unifi_url: None,
        unifi_api_key: None,
        media_services: vec![],
        collection_deadline: StdDuration::from_secs(1),
        collector_deadline: StdDuration::from_secs(1),
        probe_deadline: StdDuration::from_secs(1),
    };
    let paths = InventoryPaths::new(cfg.root.clone());
    let inventory = HomelabInventory::empty("run".to_string(), Utc::now().to_rfc3339());
    write_json_private(&paths.normalized_json, &inventory).unwrap();

    let status = inventory_status(&cfg);
    assert_eq!(status.status, "available");
    assert!(!status.is_stale);
}

#[test]
fn status_ignores_non_metadata_cache_shape() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = InventoryConfig {
        root: dir.path().join("inventory"),
        compose_paths: vec![],
        proxy_paths: vec![],
        adguard_paths: vec![],
        ssh_config: None,
        ssh_hosts: vec![],
        project_roots: vec![],
        docker_hosts: vec![],
        unraid_url: None,
        unraid_api_key: None,
        unifi_url: None,
        unifi_api_key: None,
        media_services: vec![],
        collection_deadline: StdDuration::from_secs(1),
        collector_deadline: StdDuration::from_secs(1),
        probe_deadline: StdDuration::from_secs(1),
    };
    let paths = InventoryPaths::new(cfg.root.clone());
    write_json_private(
        &paths.normalized_json,
        &serde_json::json!({
            "generated_at": Utc::now().to_rfc3339(),
            "freshness": {"stale_after_secs": 86400},
            "nodes": "not-an-array"
        }),
    )
    .unwrap();

    let status = inventory_status(&cfg);
    assert_eq!(status.status, "available");
}

#[test]
fn status_reports_corrupt_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = InventoryConfig {
        root: dir.path().join("inventory"),
        compose_paths: vec![],
        proxy_paths: vec![],
        adguard_paths: vec![],
        ssh_config: None,
        ssh_hosts: vec![],
        project_roots: vec![],
        docker_hosts: vec![],
        unraid_url: None,
        unraid_api_key: None,
        unifi_url: None,
        unifi_api_key: None,
        media_services: vec![],
        collection_deadline: StdDuration::from_secs(1),
        collector_deadline: StdDuration::from_secs(1),
        probe_deadline: StdDuration::from_secs(1),
    };
    let paths = InventoryPaths::new(cfg.root.clone());
    ensure_private_dir(paths.normalized_json.parent().unwrap()).unwrap();
    std::fs::write(&paths.normalized_json, "{not json").unwrap();

    let status = inventory_status(&cfg);
    assert_eq!(status.status, "corrupt");
    assert!(
        status
            .warnings
            .iter()
            .any(|warning| warning.contains("normalized cache unreadable"))
    );
}
