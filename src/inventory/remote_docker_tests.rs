use super::*;
use serde_json::json;

#[test]
fn normalizes_remote_docker_inspect() {
    let mut out = CollectorOutput::new("remote_docker");
    normalize_inspect(
        "devhost",
        &json!([{
            "Id": "abc",
            "Name": "/axon",
            "Config": {
                "Image": "axon:latest",
                "Env": ["TOKEN=secret", "RUST_LOG=info"],
                "Labels": {
                    "com.docker.compose.project": "axon",
                    "traefik.http.routers.axon.rule": "Host(`axon.example.internal`)"
                }
            },
            "State": {"Status": "running", "Health": {"Status": "healthy"}},
            "NetworkSettings": {
                "Ports": {"8765/tcp": [{"HostIp": "0.0.0.0", "HostPort": "8765"}]},
                "Networks": {"shared": {}}
            },
            "Mounts": [{"Source": "/srv/axon", "Destination": "/app", "RW": false}]
        }]),
        &mut out,
    );

    let service = &out.services[0];
    assert_eq!(service.name, "axon");
    assert_eq!(service.status.as_deref(), Some("running (healthy)"));
    assert_eq!(service.env_keys, vec!["TOKEN", "RUST_LOG"]);
    assert_eq!(service.mounts[0].target, "/app");
    assert!(
        service
            .domains
            .iter()
            .any(|domain| domain == "axon.example.internal")
    );
    assert_eq!(out.networks[0].members, vec!["axon"]);
}

#[test]
fn normalizes_compact_inspect_lines() {
    let mut out = CollectorOutput::new("remote_docker");
    normalize_inspect_lines(
        "edgehost",
        "\"abc\"\t\"/swag\"\t\"linuxserver/swag\"\t\"running\"\t\"\"\t{\"com.docker.compose.project\":\"swag\",\"traefik.http.routers.swag.rule\":\"Host(`swag.example.internal`)\"}\t{\"443/tcp\":[{\"HostIp\":\"0.0.0.0\",\"HostPort\":\"443\"}]}\t{\"proxy\":{}}\t[{\"Source\":\"/mnt/appdata/swag\",\"Destination\":\"/config\",\"RW\":true}]\t[\"URL=redacted\",\"PUID=99\"]\n",
        &mut out,
    );

    let service = &out.services[0];
    assert_eq!(service.name, "swag");
    assert_eq!(service.image.as_deref(), Some("linuxserver/swag"));
    assert_eq!(service.ports[0].host_port, Some(443));
    assert_eq!(service.env_keys, vec!["URL", "PUID"]);
    assert!(
        service
            .domains
            .iter()
            .any(|domain| domain == "swag.example.internal")
    );
}
