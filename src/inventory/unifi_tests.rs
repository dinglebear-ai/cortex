use super::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn collector_uses_modern_network_details_and_suppresses_successful_fallback_errors() {
    let server = MockServer::start().await;
    let api_key = "test-api-key";

    Mock::given(method("GET"))
        .and(path("/proxy/network/integration/v1/sites"))
        .and(header("x-api-key", api_key))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{
                "id": "site-1",
                "internalReference": "default",
                "name": "Home"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/api/s/default/stat/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"mac": "aa:bb", "ip": "10.1.0.2"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/api/s/default/rest/networkconf"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/integration/v1/sites/site-1/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "net-1", "name": "LAN"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/proxy/network/integration/v1/sites/site-1/networks/net-1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "net-1",
            "name": "LAN",
            "ipv4Configuration": {
                "gatewayIpAddress": "10.1.0.1",
                "dhcpConfiguration": {
                    "enabled": true,
                    "rangeStart": "10.1.0.100",
                    "rangeEnd": "10.1.0.200",
                    "dnsServerIpAddresses": ["10.1.0.8"]
                }
            }
        })))
        .mount(&server)
        .await;

    let out = collect(Some(&server.uri()), Some(api_key), Duration::from_secs(2)).await;

    let network = out
        .networks
        .iter()
        .find(|network| network.kind == "unifi_network" && network.name == "LAN")
        .unwrap();
    assert_eq!(network.details["dhcp"]["gateway"], "10.1.0.1");
    assert_eq!(network.details["dhcp"]["dns_servers"], json!(["10.1.0.8"]));
    assert!(out.nodes.iter().any(|node| node.hostname == "aa:bb"));
    assert!(
        !out.warnings
            .iter()
            .any(|warning| warning.contains("rest/networkconf"))
    );
}

#[tokio::test]
async fn collection_deadline_preserves_completed_unifi_network_details() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/integration/v1/sites"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "site-1", "internalReference": "default", "name": "Home"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/api/s/default/stat/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/api/s/default/rest/networkconf"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/network/integration/v1/sites/site-1/networks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"id": "fast", "name": "Fast"},
                {"id": "slow", "name": "Slow"}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/proxy/network/integration/v1/sites/site-1/networks/fast",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "fast",
            "name": "Fast",
            "ipv4Configuration": {
                "dhcpConfiguration": {"dnsServerIpAddresses": ["10.1.0.8"]}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/proxy/network/integration/v1/sites/site-1/networks/slow",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(json!({"id": "slow", "name": "Slow"})),
        )
        .mount(&server)
        .await;

    let out = collect(
        Some(&server.uri()),
        Some("test-api-key"),
        Duration::from_millis(500),
    )
    .await;

    let fast = out
        .networks
        .iter()
        .find(|network| network.name == "Fast")
        .unwrap();
    assert_eq!(fast.details["dhcp"]["dns_servers"], json!(["10.1.0.8"]));
    assert!(out.networks.iter().any(|network| network.name == "Slow"));
    assert!(out.warnings.iter().any(|warning| {
        warning.contains("preserved completed results")
            && warning.contains("network detail requests")
    }));
}

#[test]
fn unifi_path_segments_are_percent_encoded() {
    assert_eq!(encode_path_segment("default"), "default");
    assert_eq!(encode_path_segment("site /?"), "site%20%2F%3F");
}

#[tokio::test]
async fn missing_optional_config_is_not_a_collection_error() {
    let out = collect(None, None, Duration::from_millis(10)).await;
    assert!(out.errors.is_empty());
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning.contains("skipped"))
    );
}

#[test]
fn unifi_optional_fields_do_not_break_device_normalization() {
    let mut out = CollectorOutput::new("unifi");
    normalize_devices(
        "https://unifi",
        "/proxy/network/api/s/default/stat/device",
        &json!({"data":[{"mac":"aa:bb","ip":"10.0.0.2"}]}),
        &mut out,
    );
    assert_eq!(out.nodes[0].hostname, "aa:bb");
    assert_eq!(out.nodes[0].ips, vec!["10.0.0.2"]);
    assert!(out.nodes[0].extras.contains_key("settings"));
}

#[test]
fn modern_unifi_network_details_include_exact_dhcp_dns_assignments() {
    let mut out = CollectorOutput::new("unifi");
    normalize_networks(
        "https://unifi",
        "/proxy/network/integration/v1/sites/site-1/networks/net-1",
        Some("site-1"),
        &json!({
            "id": "net-1",
            "name": "LAN",
            "enabled": true,
            "vlanId": 1,
            "ipv4Configuration": {
                "gatewayIpAddress": "10.1.0.1",
                "dhcpConfiguration": {
                    "enabled": true,
                    "rangeStart": "10.1.0.100",
                    "rangeEnd": "10.1.0.200",
                    "leaseTimeSeconds": 86400,
                    "dnsServerIpAddresses": ["10.1.0.8", "1.1.1.1"]
                }
            }
        }),
        &mut out,
    );

    let network = &out.networks[0];
    assert_eq!(network.name, "LAN");
    assert_eq!(network.kind, "unifi_network");
    assert_eq!(network.details["site_id"], "site-1");
    assert_eq!(network.details["dhcp"]["enabled"], true);
    assert_eq!(network.details["dhcp"]["range_start"], "10.1.0.100");
    assert_eq!(network.details["dhcp"]["range_end"], "10.1.0.200");
    assert_eq!(
        network.details["dhcp"]["dns_servers"],
        json!(["1.1.1.1", "10.1.0.8"])
    );
    assert!(network.details.contains_key("settings"));
}

#[test]
fn top_level_network_enabled_does_not_imply_dhcp_enabled() {
    let mut out = CollectorOutput::new("unifi");
    normalize_networks(
        "https://unifi",
        "/proxy/network/integration/v1/sites/site-1/networks/net-1",
        Some("site-1"),
        &json!({
            "id": "net-1",
            "name": "LAN",
            "enabled": true,
            "ipv4Configuration": {
                "dnsServerType": "auto",
                "dnsServerIpAddresses": ["10.1.0.8", "not-an-ip", "2001:db8::53"]
            }
        }),
        &mut out,
    );

    let dhcp = out.networks[0].details.get("dhcp").unwrap();
    assert!(dhcp.get("enabled").is_none());
    assert_eq!(dhcp["dns_servers"], json!(["10.1.0.8", "2001:db8::53"]));
    assert!(!dhcp.to_string().contains("auto"));
}

#[test]
fn network_detail_merge_preserves_list_fields_and_adds_dhcp() {
    let mut out = CollectorOutput::new("unifi");
    normalize_networks(
        "https://unifi",
        "/proxy/network/integration/v1/sites/site-1/networks",
        Some("site-1"),
        &json!({"data":[{
            "id":"net-1",
            "name":"LAN",
            "vlanId":42,
            "purpose":"corporate"
        }]}),
        &mut out,
    );
    normalize_networks(
        "https://unifi",
        "/proxy/network/integration/v1/sites/site-1/networks/net-1",
        Some("site-1"),
        &json!({
            "id":"net-1",
            "name":"LAN",
            "ipv4Configuration": {
                "dhcpConfiguration": {
                    "enabled": true,
                    "dnsServerIpAddresses": ["10.1.0.8"]
                }
            }
        }),
        &mut out,
    );

    assert_eq!(out.networks.len(), 1);
    let details = &out.networks[0].details;
    assert_eq!(details["vlanId"], 42);
    assert_eq!(details["purpose"], "corporate");
    assert_eq!(details["dhcp"]["enabled"], true);
    assert_eq!(details["dhcp"]["dns_servers"], json!(["10.1.0.8"]));
}

#[test]
fn same_name_networks_with_different_ids_remain_distinct() {
    let mut out = CollectorOutput::new("unifi");
    normalize_networks(
        "https://unifi",
        "/proxy/network/integration/v1/sites/site-1/networks",
        Some("site-1"),
        &json!({"data":[
            {"id":"net-1","name":"LAN"},
            {"id":"net-2","name":"LAN"}
        ]}),
        &mut out,
    );

    assert_eq!(out.networks.len(), 2);
}

#[test]
fn legacy_unifi_networkconf_fields_are_normalized() {
    let mut out = CollectorOutput::new("unifi");
    normalize_networks(
        "https://unifi",
        "/proxy/network/api/s/default/rest/networkconf",
        None,
        &json!({"data":[{
            "_id":"legacy-net",
            "name":"IoT",
            "purpose":"corporate",
            "dhcpd_enabled":true,
            "dhcpd_start":"10.20.0.10",
            "dhcpd_stop":"10.20.0.99",
            "dhcpd_gateway":"10.20.0.1",
            "dhcpd_dns_1":"10.1.0.8",
            "dhcpd_dns_2":"9.9.9.9"
        }]}),
        &mut out,
    );

    let dhcp = &out.networks[0].details["dhcp"];
    assert_eq!(dhcp["enabled"], true);
    assert_eq!(dhcp["gateway"], "10.20.0.1");
    assert_eq!(dhcp["dns_servers"], json!(["10.1.0.8", "9.9.9.9"]));
}

#[test]
fn unifi_sites_and_truncation_are_reported() {
    let mut out = CollectorOutput::new("unifi");
    let sites = normalize_sites(
        "https://unifi",
        "/proxy/network/integration/v1/sites",
        &json!({"data":[{
            "id":"site-1",
            "internalReference":"default",
            "name":"Home"
        }]}),
        &mut out,
    );
    assert_eq!(sites[0].internal_reference, "default");
    assert_eq!(out.networks[0].details["id"], "site-1");
    assert_eq!(out.networks[0].details["internal_reference"], "default");
    assert_eq!(out.networks[0].details["settings"]["id"], "site-1");

    let items = (0..201)
        .map(|idx| json!({"id": format!("dev-{idx}")}))
        .collect::<Vec<_>>();
    normalize_devices(
        "https://unifi",
        "/proxy/network/api/s/default/stat/device",
        &json!({"data": items}),
        &mut out,
    );
    assert_eq!(out.nodes.len(), 200);
    assert!(out.errors.iter().any(|error| error.truncated));
}

#[test]
fn missing_device_identity_is_reported() {
    let mut out = CollectorOutput::new("unifi");
    normalize_devices(
        "https://unifi",
        "/proxy/network/api/s/default/stat/device",
        &json!({"data":[{"ip":"10.0.0.2"}]}),
        &mut out,
    );
    assert!(out.nodes.is_empty());
    assert!(
        out.errors
            .iter()
            .any(|error| error.message.contains("missing"))
    );
}

#[tokio::test]
async fn invalid_api_key_header_reports_config_warning() {
    let out = collect(
        Some("https://unifi"),
        Some("bad\nkey"),
        Duration::from_millis(10),
    )
    .await;
    assert!(out.errors.iter().any(|error| {
        error.phase == "config" && error.message.contains("invalid header characters")
    }));
}
