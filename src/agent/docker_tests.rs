use super::*;

#[test]
fn container_display_name_prefers_first_docker_name_without_leading_slash() {
    assert_eq!(
        container_display_name(
            "abcdef1234567890",
            Some(vec!["/cortex".to_string(), "/alias".to_string()])
        ),
        "cortex"
    );
}

#[test]
fn container_display_name_falls_back_to_short_id_when_names_missing() {
    assert_eq!(
        container_display_name("abcdef1234567890", Some(Vec::new())),
        "abcdef123456"
    );
    assert_eq!(container_display_name("short", None), "short");
}

#[test]
fn container_app_name_is_flat_compose_service_when_present() {
    // Compose project is intentionally ignored: canonical identity (project,
    // service, container id/name, image) now rides in structured
    // `metadata_json.agent_docker`, so APP-NAME only needs a short,
    // human-readable, non-slash label.
    let labels = HashMap::from([
        (
            "com.docker.compose.project".to_string(),
            "cortex".to_string(),
        ),
        (
            "com.docker.compose.service".to_string(),
            "server".to_string(),
        ),
    ]);

    assert_eq!(container_app_name("cortex-1", &labels), "server");
}

#[test]
fn container_app_name_falls_back_to_container_name_without_service_label() {
    assert_eq!(container_app_name("cortex-1", &HashMap::new()), "cortex-1");
}

#[test]
fn container_app_name_never_contains_a_slash() {
    let labels = HashMap::from([
        (
            "com.docker.compose.project".to_string(),
            "go.opentelemetry.io/collector".to_string(),
        ),
        (
            "com.docker.compose.service".to_string(),
            "receiver".to_string(),
        ),
    ]);
    assert!(!container_app_name("cortex-1", &labels).contains('/'));
    assert!(!container_app_name("cortex-1", &HashMap::new()).contains('/'));
}

#[test]
fn should_forward_container_logs_excludes_cortex_server_containers() {
    let compose_cortex = HashMap::from([
        (
            "com.docker.compose.project".to_string(),
            "cortex".to_string(),
        ),
        (
            "com.docker.compose.service".to_string(),
            "cortex".to_string(),
        ),
    ]);
    assert!(!should_forward_container_logs("cortex", &compose_cortex));

    let plain_cortex = HashMap::new();
    assert!(!should_forward_container_logs("cortex", &plain_cortex));

    let agent = HashMap::from([(
        "com.docker.compose.service".to_string(),
        "cortex-agent".to_string(),
    )]);
    assert!(should_forward_container_logs("cortex-agent", &agent));

    let unrelated = HashMap::from([
        ("com.docker.compose.project".to_string(), "lab".to_string()),
        (
            "com.docker.compose.service".to_string(),
            "labby".to_string(),
        ),
    ]);
    assert!(should_forward_container_logs("labby", &unrelated));
}

#[test]
fn connect_constructs_http_client_without_eager_network_io() {
    connect("http://127.0.0.1:2375").unwrap();
}

#[cfg(unix)]
#[test]
fn connect_constructs_unix_socket_client_without_eager_network_io() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

    connect(&format!("unix://{}", socket.display())).unwrap();
}

#[test]
fn container_identity_metadata_carries_compose_context() {
    let labels = HashMap::from([
        ("com.docker.compose.project".to_string(), "plex".to_string()),
        ("com.docker.compose.service".to_string(), "plex".to_string()),
        (
            "com.docker.compose.config-hash".to_string(),
            "abc".to_string(),
        ),
    ]);
    let metadata = container_identity_metadata(
        "nashost",
        "abcdef1234567890",
        "plex",
        "stdout",
        Some("lscr.io/linuxserver/plex:latest"),
        &labels,
    );
    assert_eq!(metadata["source_kind"], "agent-docker");
    assert_eq!(metadata["agent_docker"]["host"], "nashost");
    assert_eq!(metadata["agent_docker"]["container_id"], "abcdef1234567890");
    assert_eq!(metadata["agent_docker"]["compose_project"], "plex");
    assert_eq!(metadata["agent_docker"]["compose_service"], "plex");
}

#[test]
fn long_compose_app_name_still_has_structured_metadata() {
    let labels = HashMap::from([
        (
            "com.docker.compose.project".to_string(),
            "very-long-compose-project-name-for-plex-media-stack".to_string(),
        ),
        (
            "com.docker.compose.service".to_string(),
            "very-long-plex-service-name-that-exceeds-the-forty-eight-byte-app-name-limit"
                .to_string(),
        ),
    ]);
    // APP-NAME is flat (compose_service alone) but can still overflow the
    // RFC 5424 48-byte APP-NAME limit on its own; `sanitise_field` (in
    // `agent::syslog_sender`) replaces an overlong APP-NAME wholesale with
    // the `cortex-agent` fallback at send time, but structured
    // `metadata_json.agent_docker` is unaffected either way.
    let app_name = container_app_name("very-long-container-name-for-plex", &labels);
    assert!(app_name.len() > 48);
    let metadata = container_identity_metadata(
        "nashost",
        "abcdef1234567890",
        "very-long-container-name-for-plex",
        "stderr",
        None,
        &labels,
    );
    assert_eq!(
        metadata["agent_docker"]["compose_service"],
        "very-long-plex-service-name-that-exceeds-the-forty-eight-byte-app-name-limit"
    );
}
