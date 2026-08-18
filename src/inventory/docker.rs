use chrono::Utc;
use futures_util::{StreamExt, stream};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::http::HttpProbe;
use crate::inventory::schema::{
    InventoryService, MountRef, NetworkSegment, PortMapping, Provenance, TrustLevel,
};

const MAX_CONCURRENT_HOST_PROBES: usize = 8;

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
    let mut network_members: BTreeMap<String, (BTreeSet<String>, Provenance)> = BTreeMap::new();
    let mut responses = stream::iter(hosts.iter().cloned().enumerate())
        .map(|(index, host)| {
            let endpoint = format!("{}/containers/json?all=1", host.trim_end_matches('/'));
            let http = &http;
            async move { (index, http.get_json(&endpoint, HeaderMap::new()).await) }
        })
        .buffer_unordered(MAX_CONCURRENT_HOST_PROBES)
        .collect::<Vec<_>>()
        .await;
    responses.sort_by_key(|(index, _)| *index);
    for (host, (_, response)) in hosts.iter().zip(responses) {
        match response {
            Ok(response) if response.status < 400 => {
                normalize_containers(host, &response.body, &mut out, &mut network_members)
            }
            Ok(response) => out.warn(
                "containers",
                format!("Docker {host} returned HTTP {}", response.status),
            ),
            Err(error) => out.warn("containers", format!("Docker {host} unavailable: {error}")),
        }
    }
    materialize_networks(&mut out, network_members);
    out
}

fn materialize_networks(
    out: &mut CollectorOutput,
    network_members: BTreeMap<String, (BTreeSet<String>, Provenance)>,
) {
    out.networks.extend(
        network_members
            .into_iter()
            .map(|(name, (members, provenance))| NetworkSegment {
                name,
                kind: "docker".to_string(),
                members: members.into_iter().collect(),
                provenance,
                details: Default::default(),
            }),
    );
}

fn normalize_containers(
    host: &str,
    body: &Value,
    out: &mut CollectorOutput,
    network_members: &mut BTreeMap<String, (BTreeSet<String>, Provenance)>,
) {
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
            network_members
                .entry(network.clone())
                .or_insert_with(|| (BTreeSet::new(), provenance(host)))
                .0
                .insert(name.clone());
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
