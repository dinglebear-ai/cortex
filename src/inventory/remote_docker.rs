use chrono::Utc;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::docker::{extract_domainish, string_map};
use crate::inventory::schema::{
    InventoryService, MountRef, NetworkSegment, PortMapping, Provenance, TrustLevel,
};
use crate::inventory::ssh::{SshContext, configured_hosts as resolve_ssh_hosts};

pub async fn collect(
    ssh_config: Option<&Path>,
    configured_hosts: &[String],
    ssh_context: &SshContext,
    timeout: Duration,
) -> CollectorOutput {
    let resolution = resolve_ssh_hosts(ssh_config, configured_hosts);
    let mut out = CollectorOutput::new("remote_docker");
    for warning in &resolution.warnings {
        out.warn("host_resolution", warning);
    }
    if resolution.no_usable_explicit_hosts() {
        out.warn(
            "host_resolution",
            "remote Docker collector skipped because no explicitly configured SSH hosts were usable",
        );
        return out;
    }
    let mut handles = Vec::new();
    for host in resolution.hosts {
        let ssh_context = ssh_context.clone();
        handles.push(tokio::spawn(async move {
            collect_host(host, ssh_context, timeout).await
        }));
    }

    for handle in handles {
        match handle.await {
            Ok(host_output) => merge_output(&mut out, host_output),
            Err(error) => out.warn(
                "remote_docker",
                format!("remote Docker task failed: {error}"),
            ),
        }
    }
    out
}

async fn collect_host(host: String, ssh_context: SshContext, timeout: Duration) -> CollectorOutput {
    let mut out = CollectorOutput::new("remote_docker");
    match ssh_context.run(&host, docker_command(), timeout).await {
        Ok(output) if output.status == Some(0) && !output.stdout.trim().is_empty() => {
            normalize_inspect_lines(&host, &output.stdout, &mut out);
        }
        Ok(output) if output.status == Some(0) => {}
        Ok(output) => out.warn(
            "inspect",
            format!("remote Docker inspect failed on {host}: {}", output.stderr),
        ),
        Err(error) => out.warn(
            "inspect",
            format!("remote Docker inspect failed on {host}: {error}"),
        ),
    }
    out
}

#[cfg(test)]
fn normalize_inspect(host: &str, body: &Value, out: &mut CollectorOutput) {
    let Some(items) = body.as_array() else {
        out.warn(
            "inspect",
            format!("Docker inspect output on {host} was not an array"),
        );
        return;
    };
    for item in items.iter().take(250) {
        let id = item.get("Id").and_then(Value::as_str).unwrap_or("unknown");
        let name = item
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .trim_start_matches('/')
            .to_string();
        let labels = string_map(item.get("Config").and_then(|config| config.get("Labels")));
        let ports = parse_inspect_ports(item.get("NetworkSettings").and_then(|v| v.get("Ports")));
        let networks = item
            .get("NetworkSettings")
            .and_then(|v| v.get("Networks"))
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        add_networks(host, &name, networks, out);
        let health = item
            .get("State")
            .and_then(|state| state.get("Health"))
            .and_then(|health| health.get("Status"))
            .and_then(Value::as_str);
        out.services.push(InventoryService {
            id: format!("docker:{host}:{id}"),
            name,
            kind: "docker_container".to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(host),
            host: Some(host.to_string()),
            image: item
                .get("Config")
                .and_then(|config| config.get("Image"))
                .and_then(Value::as_str)
                .or_else(|| item.get("Image").and_then(Value::as_str))
                .map(ToString::to_string),
            status: item
                .get("State")
                .and_then(|state| state.get("Status"))
                .and_then(Value::as_str)
                .map(|status| match health {
                    Some(health) => format!("{status} ({health})"),
                    None => status.to_string(),
                }),
            domains: labels
                .iter()
                .filter(|(k, _)| k.contains("rule") || k.contains("host"))
                .flat_map(|(_, v)| extract_domainish(v))
                .collect(),
            ports,
            mounts: parse_mounts(item.get("Mounts")),
            env_keys: parse_env_keys(item.get("Config").and_then(|config| config.get("Env"))),
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

fn normalize_inspect_lines(host: &str, body: &str, out: &mut CollectorOutput) {
    for line in body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(250)
    {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 10 {
            out.warn(
                "inspect",
                format!(
                    "remote Docker inspect record on {host} had {} fields",
                    fields.len()
                ),
            );
            continue;
        }
        normalize_compact_record(host, &fields, out);
    }
}

fn normalize_compact_record(host: &str, fields: &[&str], out: &mut CollectorOutput) {
    let id = parse_json_string(fields[0]).unwrap_or_else(|| "unknown".to_string());
    let name = parse_json_string(fields[1])
        .unwrap_or_else(|| id.clone())
        .trim_start_matches('/')
        .to_string();
    let image = parse_json_string(fields[2]);
    let status = parse_json_string(fields[3]);
    let health = parse_json_string(fields[4]).filter(|value| !value.is_empty());
    let labels = parse_json_value(fields[5])
        .map(|value| string_map(Some(&value)))
        .unwrap_or_default();
    let ports = parse_json_value(fields[6])
        .map(|value| parse_inspect_ports(Some(&value)))
        .unwrap_or_default();
    let networks = parse_json_value(fields[7])
        .and_then(|value| {
            value
                .as_object()
                .map(|map| map.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    add_networks(host, &name, networks, out);
    let mounts = parse_json_value(fields[8])
        .map(|value| parse_mounts(Some(&value)))
        .unwrap_or_default();
    let env_keys = parse_json_value(fields[9])
        .map(|value| parse_env_keys(Some(&value)))
        .unwrap_or_default();

    out.services.push(InventoryService {
        id: format!("docker:{host}:{id}"),
        name,
        kind: "docker_container".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance(host),
        host: Some(host.to_string()),
        image,
        status: status.map(|status| match health {
            Some(health) => format!("{status} ({health})"),
            None => status,
        }),
        domains: labels
            .iter()
            .filter(|(k, _)| k.contains("rule") || k.contains("host"))
            .flat_map(|(_, v)| extract_domainish(v))
            .collect(),
        ports,
        mounts,
        env_keys,
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

fn parse_inspect_ports(value: Option<&Value>) -> Vec<PortMapping> {
    let Some(map) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for (container, bindings) in map {
        let (container_port, protocol) = parse_container_port(container);
        if let Some(bindings) = bindings.as_array() {
            for binding in bindings {
                ports.push(PortMapping {
                    host_ip: binding
                        .get("HostIp")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    host_port: binding
                        .get("HostPort")
                        .and_then(Value::as_str)
                        .and_then(|p| p.parse().ok()),
                    container_port,
                    protocol: protocol.clone(),
                });
            }
        } else {
            ports.push(PortMapping {
                host_ip: None,
                host_port: None,
                container_port,
                protocol,
            });
        }
    }
    ports
}

fn parse_container_port(value: &str) -> (Option<u16>, String) {
    let (port, protocol) = value.split_once('/').unwrap_or((value, "tcp"));
    (port.parse().ok(), protocol.to_string())
}

fn parse_mounts(value: Option<&Value>) -> Vec<MountRef> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mount| {
            Some(MountRef {
                source: mount
                    .get("Source")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                target: mount
                    .get("Destination")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)?,
                read_only: !mount.get("RW").and_then(Value::as_bool).unwrap_or(true),
            })
        })
        .take(50)
        .collect()
}

fn parse_env_keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|entry| entry.split_once('=').map(|(key, _)| key.to_string()))
        .collect()
}

fn add_networks(host: &str, name: &str, networks: Vec<String>, out: &mut CollectorOutput) {
    for network in networks {
        if let Some(existing) = out
            .networks
            .iter_mut()
            .find(|segment| segment.name == network && segment.kind == "docker")
        {
            if !existing.members.iter().any(|member| member == name) {
                existing.members.push(name.to_string());
            }
        } else {
            out.networks.push(NetworkSegment {
                name: network,
                kind: "docker".to_string(),
                members: vec![name.to_string()],
                provenance: provenance(host),
                details: Default::default(),
            });
        }
    }
}

fn docker_command() -> &'static str {
    "if command -v docker >/dev/null 2>&1; then docker ps -a --format '{{.ID}}' 2>/dev/null | head -250 | xargs -r -n1 docker inspect --format '{{json .Id}}\t{{json .Name}}\t{{json .Config.Image}}\t{{json .State.Status}}\t{{if index .State \"Health\"}}{{json (index .State \"Health\").Status}}{{else}}\"\"{{end}}\t{{json .Config.Labels}}\t{{json .NetworkSettings.Ports}}\t{{json .NetworkSettings.Networks}}\t{{json .Mounts}}\t{{json .Config.Env}}' 2>/dev/null; fi"
}

fn provenance(host: &str) -> Provenance {
    Provenance::new(
        format!("{host}:docker inspect"),
        "source_inventory",
        Utc::now().to_rfc3339(),
    )
}

fn merge_output(out: &mut CollectorOutput, remote: CollectorOutput) {
    out.services.extend(remote.services);
    out.networks.extend(remote.networks);
    out.errors.extend(remote.errors);
    out.warnings.extend(remote.warnings);
}

fn parse_json_string(input: &str) -> Option<String> {
    serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
}

fn parse_json_value(input: &str) -> Option<Value> {
    serde_json::from_str::<Value>(input).ok()
}

#[cfg(test)]
#[path = "remote_docker_tests.rs"]
mod tests;
