use chrono::Utc;
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::http::HttpProbe;
use crate::inventory::schema::{
    InventoryService, MountRef, NetworkSegment, PortMapping, Provenance, TrustLevel,
};

pub async fn collect(hosts: &[String], timeout: Duration) -> CollectorOutput {
    let mut out = CollectorOutput::new("docker");
    if hosts.is_empty() {
        out.warn(
            "config",
            "CORTEX_DOCKER_HOSTS not set; Docker API collection skipped",
        );
        return out;
    }
    let Ok(http) = HttpProbe::new(timeout) else {
        out.warn("http", "failed to initialize Docker HTTP client");
        return out;
    };
    for host in hosts {
        match http
            .get_json(
                &format!("{}/containers/json?all=1", host.trim_end_matches('/')),
                HeaderMap::new(),
            )
            .await
        {
            Ok(response) if response.status < 400 => {
                normalize_containers(host, &response.body, &mut out)
            }
            Ok(response) => out.warn(
                "containers",
                format!("Docker {host} returned HTTP {}", response.status),
            ),
            Err(error) => out.warn("containers", format!("Docker {host} unavailable: {error}")),
        }
    }
    out
}

fn normalize_containers(host: &str, body: &Value, out: &mut CollectorOutput) {
    let Some(items) = body.as_array() else {
        out.warn(
            "containers",
            format!("Docker {host} response was not an array"),
        );
        return;
    };
    for item in items.iter().take(200) {
        let id = item.get("Id").and_then(Value::as_str).unwrap_or("unknown");
        let name = item
            .get("Names")
            .and_then(Value::as_array)
            .and_then(|names| names.first())
            .and_then(Value::as_str)
            .unwrap_or(id)
            .trim_start_matches('/')
            .to_string();
        let labels = string_map(item.get("Labels"));
        let ports = parse_ports(item.get("Ports"));
        let networks = item
            .get("NetworkSettings")
            .and_then(|v| v.get("Networks"))
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for network in &networks {
            if let Some(existing) = out
                .networks
                .iter_mut()
                .find(|segment| segment.name == *network && segment.kind == "docker")
            {
                if !existing.members.contains(&name) {
                    existing.members.push(name.clone());
                }
            } else {
                out.networks.push(NetworkSegment {
                    name: network.clone(),
                    kind: "docker".to_string(),
                    members: vec![name.clone()],
                    provenance: provenance(host),
                    details: Default::default(),
                });
            }
        }
        out.services.push(InventoryService {
            id: format!("docker:{host}:{id}"),
            name,
            kind: "docker_container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(host),
            host: Some(host.to_string()),
            image: item
                .get("Image")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            status: item
                .get("State")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            domains: labels
                .iter()
                .filter(|(k, _)| k.contains("rule") || k.contains("host"))
                .flat_map(|(_, v)| extract_domainish(v))
                .collect(),
            ports,
            mounts: Vec::<MountRef>::new(),
            env_keys: Vec::new(),
            labels: labels
                .into_iter()
                .filter(|(key, _)| {
                    key.starts_with("com.docker.compose")
                        || key.contains("traefik")
                        || key.contains("swag")
                })
                .collect::<BTreeMap<_, _>>(),
            details: Default::default(),
        });
    }
}

fn parse_ports(value: Option<&Value>) -> Vec<PortMapping> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|port| PortMapping {
            host_ip: port
                .get("IP")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            host_port: port
                .get("PublicPort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok()),
            container_port: port
                .get("PrivatePort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok()),
            protocol: port
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or("tcp")
                .to_string(),
        })
        .collect()
}

pub(in crate::inventory) fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn provenance(host: &str) -> Provenance {
    Provenance::new(
        format!("{host}/containers/json"),
        "source_inventory",
        Utc::now().to_rfc3339(),
    )
}

pub(in crate::inventory) fn extract_domainish(line: &str) -> Vec<String> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .filter(|part| part.contains('.') && part.len() > 3)
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
