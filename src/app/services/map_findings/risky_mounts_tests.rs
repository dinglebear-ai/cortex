use super::*;

fn mount(source: Option<&str>, target: &str, read_only: bool) -> MountRef {
    MountRef {
        source: source.map(str::to_string),
        target: target.to_string(),
        read_only,
    }
}

#[test]
fn classify_mount_covers_risky_source_branches() {
    let docker_rw = classify_mount(&mount(
        Some("/var/run/docker.sock"),
        "/var/run/docker.sock",
        false,
    ))
    .unwrap();
    assert_eq!(docker_rw.reason_code, reason_const::DOCKER_SOCKET_MOUNT);
    assert_eq!(docker_rw.severity, finding_const::SEVERITY_HIGH);

    let docker_ro = classify_mount(&mount(
        Some("/var/run/docker.sock"),
        "/var/run/docker.sock",
        true,
    ))
    .unwrap();
    assert_eq!(docker_ro.severity, finding_const::SEVERITY_MEDIUM);

    let root_rw = classify_mount(&mount(Some("/"), "/host", false)).unwrap();
    assert_eq!(root_rw.reason_code, reason_const::HOST_ROOT_MOUNT);
    assert_eq!(root_rw.severity, finding_const::SEVERITY_HIGH);

    let root_ro = classify_mount(&mount(Some("/"), "/host", true)).unwrap();
    assert_eq!(root_ro.severity, finding_const::SEVERITY_MEDIUM);

    let appdata_rw = classify_mount(&mount(Some("/mnt/user/appdata"), "/config", false)).unwrap();
    assert_eq!(appdata_rw.reason_code, reason_const::APPDATA_ROOT_MOUNT);
    assert_eq!(appdata_rw.severity, finding_const::SEVERITY_MEDIUM);

    let appdata_ro = classify_mount(&mount(Some("/mnt/user/appdata"), "/config", true)).unwrap();
    assert_eq!(appdata_ro.severity, finding_const::SEVERITY_LOW);

    let missing = classify_mount(&mount(None, "/config", false)).unwrap();
    assert_eq!(
        missing.reason_code,
        reason_const::MOUNT_MISSING_SOURCE_DETAIL
    );
    assert_eq!(missing.severity, finding_const::SEVERITY_LOW);
}

#[test]
fn safe_mount_target_renders_sensitive_targets_safely() {
    assert_eq!(
        safe_mount_target("/var/run/docker.sock"),
        "/var/run/docker.sock"
    );
    assert_eq!(safe_mount_target("/"), "/");
    assert_eq!(safe_mount_target("/mnt/user/appdata/app"), "appdata_path");
    assert_eq!(safe_mount_target("relative"), "relative_mount_target");
}

#[test]
fn service_mount_index_uses_service_instance_keys_not_legacy_host_colon_name() {
    let key = crate::db::entity_resolution::service_instance_key("Nashost", " Plex ").unwrap();
    assert_eq!(key, "nashost/plex");
    assert!(!key.contains(':'));
    // Identity that does not canonicalize yields no divergent fallback key.
    assert_eq!(
        crate::db::entity_resolution::service_instance_key("nashost", "---"),
        None
    );
}
