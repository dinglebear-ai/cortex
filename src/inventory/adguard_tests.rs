use super::*;
use serde_json::Value;

#[test]
fn adguard_config_normalizes_safe_dns_dhcp_and_client_details() {
    let dir = tempfile::tempdir().unwrap();
    let paths = InventoryPaths::new(dir.path().join("inventory"));
    paths.ensure_private_dirs().unwrap();
    let body = r#"
http:
  address: 0.0.0.0:3000
users:
  - name: admin
    password: super-secret-hash
dns:
  bind_hosts: [0.0.0.0]
  port: 53
  upstream_dns: [https://dns.example/dns-query]
  bootstrap_dns: [1.1.1.1]
tls:
  enabled: true
  server_name: dns.home.example
  port_https: 443
  private_key: private-key-material
filters:
  - enabled: true
    url: https://filters.example/block.txt
    name: block
whitelist_filters:
  - enabled: true
    url: https://filters.example/allow.txt
    name: allow
user_rules:
  - '||printer.home^$dnsrewrite=NOERROR;A;10.1.0.20'
dhcp:
  enabled: true
  interface_name: eth0
  dhcpv4:
    gateway_ip: 10.1.0.1
    range_start: 10.1.0.100
    range_end: 10.1.0.200
filtering:
  protection_enabled: true
  rewrites:
    - domain: nas.home
      answer: 10.1.0.10
clients:
  persistent:
    - name: laptop
      ids: [10.1.0.50]
"#;

    let (artifact, service) = collect_body(
        Some("squirts".to_string()),
        "squirts:/mnt/appdata/adguard/etc/config.yaml".to_string(),
        body.to_string(),
        &paths,
        "run",
    )
    .unwrap();

    assert_eq!(artifact.kind, "adguard_config_yaml");
    assert_eq!(service.kind, "adguard_home");
    assert_eq!(service.name, "adguard");
    assert_eq!(service.host.as_deref(), Some("squirts"));
    assert_eq!(service.status.as_deref(), Some("enabled"));
    assert_eq!(service.domains, vec!["dns.home.example"]);
    for section in [
        "dns",
        "tls",
        "filters",
        "whitelist_filters",
        "user_rules",
        "dhcp",
        "filtering",
        "clients",
        "local_records",
    ] {
        assert!(service.details.contains_key(section), "missing {section}");
    }
    let dns = service
        .details
        .get("dns")
        .and_then(Value::as_object)
        .unwrap();
    assert_eq!(dns["upstream_dns"][0], "https://dns.example/dns-query");
    assert_eq!(dns["bootstrap_dns"][0], "1.1.1.1");
    assert!(
        service
            .ports
            .iter()
            .any(|port| { port.host_port == Some(53) && port.protocol == "udp" })
    );
    assert!(service.ports.iter().any(|port| port.host_port == Some(443)));

    let serialized = serde_json::to_string(&service).unwrap();
    assert!(!serialized.contains("super-secret-hash"));
    assert!(!serialized.contains("private-key-material"));
    let cached = std::fs::read_to_string(&artifact.cache_path).unwrap();
    assert!(cached.contains("[REDACTED]"));
    assert!(!cached.contains("super-secret-hash"));
    assert!(!cached.contains("private-key-material"));
}
