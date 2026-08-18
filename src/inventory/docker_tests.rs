use super::*;
use serde_json::json;

#[test]
fn normalizes_container_ports_labels_and_domains() {
    let mut out = CollectorOutput::new("docker");
    let mut network_members = BTreeMap::new();
    normalize_containers(
        "http://docker.test:2375",
        &json!([{
            "Id": "abc",
            "Names": ["/app"],
            "Image": "repo/app:latest",
            "State": "running",
            "Labels": {
                "com.docker.compose.project": "stack",
                "traefik.http.routers.app.rule": "Host(`app.example.test`)"
            },
            "Ports": [{"PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"}],
            "NetworkSettings": {"Networks": {"net": {}}}
        }]),
        &mut out,
        &mut network_members,
    );
    materialize_networks(&mut out, network_members);
    assert_eq!(out.services[0].name, "app");
    assert_eq!(out.services[0].ports[0].host_port, Some(8080));
    assert!(
        out.services[0]
            .domains
            .iter()
            .any(|d| d.contains("app.example.test"))
    );
    assert_eq!(out.networks[0].name, "net");
}

#[test]
fn normalizes_shared_docker_network_once_with_members() {
    let mut out = CollectorOutput::new("docker");
    let mut network_members = BTreeMap::new();
    normalize_containers(
        "http://docker.test:2375",
        &json!([
            {
                "Id": "a",
                "Names": ["/app-a"],
                "NetworkSettings": {"Networks": {"shared": {}}}
            },
            {
                "Id": "b",
                "Names": ["/app-b"],
                "NetworkSettings": {"Networks": {"shared": {}}}
            }
        ]),
        &mut out,
        &mut network_members,
    );
    materialize_networks(&mut out, network_members);

    assert_eq!(out.networks.len(), 1);
    assert_eq!(out.networks[0].name, "shared");
    assert_eq!(out.networks[0].members, vec!["app-a", "app-b"]);
}
