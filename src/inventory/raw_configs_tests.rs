use super::*;

#[test]
fn compose_parser_preserves_services_domains_and_ports() {
    let project = parse_compose_project(
        Path::new("/tmp/app/docker-compose.yml"),
        r#"
services:
  app:
    ports:
      - "8080:80/tcp"
    labels:
      - "traefik.http.routers.app.rule=Host(`app.example.test`)"
"#,
    );
    assert_eq!(project.services, vec!["app"]);
    assert!(
        project
            .domains
            .iter()
            .any(|d| d.contains("app.example.test"))
    );
    assert_eq!(project.ports[0].host_port, Some(8080));
}

#[test]
fn compose_parser_preserves_host_ip_in_port_mapping() {
    let port = parse_port_line(r#"- "127.0.0.1:8080:80/tcp""#).unwrap();

    assert_eq!(port.host_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(port.host_port, Some(8080));
    assert_eq!(port.container_port, Some(80));
    assert_eq!(port.protocol, "tcp");
}

#[test]
fn compose_parser_ignores_non_port_colon_scalars() {
    let project = parse_compose_project(
        Path::new("/tmp/app/docker-compose.yml"),
        r#"
services:
  redis:
    image: redis:7
    command: "listen 0.0.0.0:6379"
"#,
    );

    assert!(project.ports.is_empty());
}

#[test]
fn proxy_parser_preserves_server_names_and_upstreams() {
    let routes = parse_proxy_routes(
        Path::new("/tmp/app.conf"),
        "server_name app.example.test; proxy_pass http://app:8080;",
    );
    assert_eq!(routes[0].server_names, vec!["app.example.test"]);
    assert!(routes[0].upstreams[0].contains("http://app:8080"));
}

#[test]
fn proxy_parser_resolves_swag_set_variables() {
    let routes = parse_proxy_routes(
        Path::new("/tmp/adguard.conf"),
        r#"
server {
    server_name adguard.example.test;
    set $upstream_app 100.75.111.118;
    set $upstream_port 3010;
    set $upstream_proto http;
    proxy_pass $upstream_proto://$upstream_app:$upstream_port;
}
"#,
    );

    assert_eq!(routes[0].server_names, vec!["adguard.example.test"]);
    assert_eq!(routes[0].upstreams, vec!["http://100.75.111.118:3010"]);
}

#[test]
fn proxy_parser_skips_empty_routes() {
    assert!(parse_proxy_routes(Path::new("/tmp/app.conf"), "listen 443;").is_empty());
}

#[test]
fn remote_compose_body_preserves_source_host_and_path() {
    let dir = tempfile::tempdir().unwrap();
    let paths = InventoryPaths::new(dir.path().join("inventory"));
    paths.ensure_private_dirs().unwrap();

    let (artifact, project) = collect_compose_body(
        Some("dookie".to_string()),
        "dookie:/home/jmagar/compose/docker-compose.yaml".to_string(),
        "services:\n  axon:\n    ports:\n      - \"3000:3000\"\n".to_string(),
        &paths,
        "run",
    )
    .unwrap();

    assert_eq!(artifact.source_host.as_deref(), Some("dookie"));
    assert_eq!(
        artifact.source_path.as_deref(),
        Some("dookie:/home/jmagar/compose/docker-compose.yaml")
    );
    assert_eq!(project.services, vec!["axon"]);
}
