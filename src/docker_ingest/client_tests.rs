use bollard::container::LogOutput;
use bytes::Bytes;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::docker_ingest::parser::log_output_to_entry;

#[tokio::test]
async fn mocked_docker_engine_fixture_lists_containers_and_maps_log_frame() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/(v[0-9.]+/)?containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "Id": "abcdef1234567890",
                "Names": ["/swag-1"],
                "Image": "lscr.io/linuxserver/swag:latest",
                "Labels": {
                    "com.docker.compose.project": "edge",
                    "com.docker.compose.service": "swag"
                }
            }
        ])))
        .mount(&server)
        .await;

    let client = DockerHostClient::connect(&server.uri()).expect("client should connect");
    let containers = client
        .list_containers()
        .await
        .expect("mock Docker API should list containers");

    assert_eq!(containers.len(), 1);
    let container = &containers[0];
    assert_eq!(container.name, "swag-1");
    assert_eq!(container.app_name(), "swag");

    let entry = log_output_to_entry(
        "edgehost",
        container,
        LogOutput::StdOut {
            message: Bytes::from_static(b"2026-06-12T12:34:56.789Z live docker fixture marker\n"),
        },
    )
    .expect("log frame should parse")
    .expect("stdout frame should produce an entry");

    assert_eq!(entry.hostname, "edgehost");
    assert_eq!(entry.app_name.as_deref(), Some("swag"));
    assert_eq!(entry.source_ip, "docker://edgehost/swag-1/stdout");
    assert_eq!(entry.message, "live docker fixture marker");
    assert_eq!(
        entry
            .docker_checkpoint
            .as_ref()
            .expect("stream rows carry checkpoint")
            .container_id,
        "abcdef1234567890"
    );
}
