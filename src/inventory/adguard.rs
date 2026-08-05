use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::inventory::limits::MAX_RAW_ARTIFACT_BYTES;
use crate::inventory::redaction::{RedactedArtifact, redact_json};
use crate::inventory::schema::{
    ArtifactRef, InventoryService, PortMapping, Provenance, TrustLevel,
};
use crate::inventory::storage::{InventoryPaths, write_artifact};

const SAFE_SECTIONS: [&str; 9] = [
    "http",
    "dns",
    "tls",
    "filters",
    "whitelist_filters",
    "user_rules",
    "dhcp",
    "filtering",
    "clients",
];

pub(in crate::inventory) fn collect_file(
    path: &Path,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, InventoryService)> {
    let file = File::open(path)?;
    let mut body = String::new();
    file.take((MAX_RAW_ARTIFACT_BYTES + 1) as u64)
        .read_to_string(&mut body)?;
    collect_body(None, path.display().to_string(), body, paths, run_id)
}

pub(in crate::inventory) fn collect_body(
    source_host: Option<String>,
    source_path: String,
    body: String,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, InventoryService)> {
    let service = normalize_service(source_host.clone(), &source_path, &body)
        .with_context(|| format!("parse AdGuard Home config {source_path}"))?;
    let artifact_id = artifact_id(&source_path);
    let artifact = RedactedArtifact::from_text(&body, MAX_RAW_ARTIFACT_BYTES);
    let reference = write_artifact(
        paths,
        run_id,
        &artifact_id,
        &artifact,
        ArtifactRef {
            id: artifact_id.clone(),
            kind: "adguard_config_yaml".to_string(),
            collector: "raw_configs".to_string(),
            source_host,
            source_path: Some(source_path),
            cache_path: String::new(),
            redaction: artifact.status(),
            byte_len: 0,
            truncated: artifact.truncated(),
        },
    )?;
    Ok((reference, service))
}

fn normalize_service(
    source_host: Option<String>,
    source_path: &str,
    body: &str,
) -> Result<InventoryService> {
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(body)?;
    let json = serde_json::to_value(yaml)?;
    let root = json
        .as_object()
        .context("AdGuard Home config root is not a mapping")?;
    let details = safe_details(root);
    let ports = service_ports(&details);
    let domains = details
        .get("tls")
        .and_then(Value::as_object)
        .and_then(|tls| tls.get("server_name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(|name| vec![name.to_string()])
        .unwrap_or_default();
    let status = details
        .get("filtering")
        .and_then(Value::as_object)
        .and_then(|filtering| filtering.get("protection_enabled"))
        .and_then(Value::as_bool)
        .map(|enabled| if enabled { "enabled" } else { "disabled" }.to_string());
    let name = "adguard".to_string();

    Ok(InventoryService {
        id: artifact_id(source_path),
        name,
        kind: "adguard_home".to_string(),
        trust_level: TrustLevel::Verified,
        provenance: Provenance::new(
            source_path.to_string(),
            "source_inventory",
            Utc::now().to_rfc3339(),
        ),
        host: source_host,
        image: None,
        status,
        domains,
        ports,
        mounts: Vec::new(),
        env_keys: Vec::new(),
        labels: BTreeMap::new(),
        details,
    })
}

fn safe_details(root: &Map<String, Value>) -> BTreeMap<String, Value> {
    let mut details = BTreeMap::new();
    for section in SAFE_SECTIONS {
        if let Some(value) = root.get(section) {
            details.insert(section.to_string(), redact_json(value));
        }
    }

    let mut local_records = details
        .get("filtering")
        .and_then(Value::as_object)
        .and_then(|filtering| filtering.get("rewrites"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(rules) = details.get("user_rules").and_then(Value::as_array) {
        local_records.extend(rules.iter().filter_map(|rule| {
            let rule = rule.as_str()?;
            rule.contains("$dnsrewrite=").then(|| {
                Value::Object(Map::from_iter([(
                    "rule".to_string(),
                    Value::String(rule.to_string()),
                )]))
            })
        }));
    }
    if !local_records.is_empty() {
        details.insert("local_records".to_string(), Value::Array(local_records));
    }
    details
}

fn service_ports(details: &BTreeMap<String, Value>) -> Vec<PortMapping> {
    let mut ports = Vec::new();
    if let Some(dns) = details.get("dns").and_then(Value::as_object) {
        let port = dns.get("port").and_then(value_u16);
        let bind_hosts = dns
            .get("bind_hosts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![Value::String("0.0.0.0".to_string())]);
        for bind in bind_hosts.iter().filter_map(Value::as_str) {
            push_port(&mut ports, Some(bind.to_string()), port, "udp");
            push_port(&mut ports, Some(bind.to_string()), port, "tcp");
        }
    }
    if let Some(http) = details.get("http").and_then(Value::as_object)
        && let Some(address) = http.get("address").and_then(Value::as_str)
        && let Some((host, port)) = parse_address(address)
    {
        push_port(&mut ports, Some(host), Some(port), "tcp");
    }
    if let Some(tls) = details.get("tls").and_then(Value::as_object)
        && tls.get("enabled").and_then(Value::as_bool).unwrap_or(false)
    {
        for (key, protocol) in [
            ("port_https", "tcp"),
            ("port_dns_over_tls", "tcp"),
            ("port_dns_over_quic", "udp"),
            ("port_dnscrypt", "udp"),
        ] {
            push_port(
                &mut ports,
                None,
                tls.get(key).and_then(value_u16).filter(|port| *port > 0),
                protocol,
            );
        }
    }
    ports
}

fn parse_address(address: &str) -> Option<(String, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    Some((
        host.trim_matches(&['[', ']'][..]).to_string(),
        port.parse().ok()?,
    ))
}

fn value_u16(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn push_port(
    ports: &mut Vec<PortMapping>,
    host_ip: Option<String>,
    port: Option<u16>,
    protocol: &str,
) {
    let Some(port) = port else {
        return;
    };
    let candidate = PortMapping {
        host_ip,
        host_port: Some(port),
        container_port: Some(port),
        protocol: protocol.to_string(),
    };
    if !ports.contains(&candidate) {
        ports.push(candidate);
    }
}

fn artifact_id(source: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(source.as_bytes());
    let digest_hex = format!("{digest:x}");
    format!("adguard:{}", &digest_hex[..32])
}

#[cfg(test)]
#[path = "adguard_tests.rs"]
mod tests;
