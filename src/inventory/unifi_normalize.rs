use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use super::SiteRef;
use crate::inventory::collectors::CollectorOutput;
use crate::inventory::redaction::redact_json;
use crate::inventory::schema::{InventoryNode, NetworkSegment, Provenance, TrustLevel};

const MAX_UNIFI_ITEMS: usize = 200;

pub(super) fn normalize_sites(
    url: &str,
    path: &str,
    body: &Value,
    out: &mut CollectorOutput,
) -> Vec<SiteRef> {
    let items = bounded_items(body, path, out);
    let mut sites = Vec::new();
    for item in items {
        let name = string_field(item, &["name", "siteName"])
            .unwrap_or("site")
            .to_string();
        let internal_reference = string_field(item, &["internalReference", "desc", "name"])
            .unwrap_or("default")
            .to_string();
        let id = string_field(item, &["id", "siteId", "_id"]).map(ToString::to_string);
        let mut details = BTreeMap::new();
        if let Some(id) = &id {
            details.insert("id".to_string(), Value::String(id.clone()));
        }
        details.insert(
            "internal_reference".to_string(),
            Value::String(internal_reference.clone()),
        );
        details.insert("settings".to_string(), redact_json(item));
        merge_network(
            out,
            NetworkSegment {
                name: name.clone(),
                kind: "unifi_site".to_string(),
                members: Vec::new(),
                provenance: provenance(url, path),
                details,
            },
        );
        sites.push(SiteRef {
            id,
            internal_reference,
        });
    }
    sites
}

pub(super) fn normalize_devices(url: &str, path: &str, body: &Value, out: &mut CollectorOutput) {
    for item in bounded_items(body, path, out) {
        let Some(hostname) = string_field(item, &["name", "hostname", "mac", "id", "_id"]) else {
            out.warn(
                path,
                "UniFi device record missing name, hostname, mac, and id; skipped",
            );
            continue;
        };
        let id = string_field(item, &["id", "_id", "mac"])
            .unwrap_or(hostname)
            .to_string();
        let node = InventoryNode {
            id: format!("unifi:{id}"),
            hostname: hostname.to_string(),
            trust_level: TrustLevel::Observed,
            provenance: provenance(url, path),
            roles: vec!["network_device".to_string()],
            ips: string_field(item, &["ip", "ipAddress"])
                .map(|ip| vec![ip.to_string()])
                .unwrap_or_default(),
            os: string_field(item, &["model", "productLine"]).map(ToString::to_string),
            cpu: None,
            memory: None,
            listeners: Vec::new(),
            storage: Vec::new(),
            extras: BTreeMap::from([("settings".to_string(), redact_json(item))]),
        };
        if let Some(existing) = out.nodes.iter_mut().find(|existing| existing.id == node.id) {
            *existing = node;
        } else {
            out.nodes.push(node);
        }
    }
}

pub(super) fn normalize_networks(
    url: &str,
    path: &str,
    site_id: Option<&str>,
    body: &Value,
    out: &mut CollectorOutput,
) -> Vec<String> {
    let items = bounded_items_or_single(body, path, out);
    let mut ids = Vec::new();
    for item in items {
        let name = string_field(item, &["name", "displayName", "purpose"])
            .unwrap_or("network")
            .to_string();
        let id = string_field(item, &["id", "_id", "networkId"]).map(ToString::to_string);
        if let Some(id) = &id {
            ids.push(id.clone());
        }
        let mut details = BTreeMap::new();
        if let Some(site_id) = site_id {
            details.insert("site_id".to_string(), Value::String(site_id.to_string()));
        }
        if let Some(id) = &id {
            details.insert("id".to_string(), Value::String(id.clone()));
        }
        copy_fields(
            item,
            &mut details,
            &[
                "enabled",
                "default",
                "management",
                "purpose",
                "vlanId",
                "vlan",
                "subnet",
                "domainName",
                "domain_name",
            ],
        );
        let dhcp = normalize_dhcp(item);
        if !dhcp.is_empty() {
            details.insert("dhcp".to_string(), Value::Object(dhcp));
        }
        details.insert("settings".to_string(), redact_json(item));
        merge_network(
            out,
            NetworkSegment {
                name,
                kind: "unifi_network".to_string(),
                members: Vec::new(),
                provenance: provenance(url, path),
                details,
            },
        );
    }
    ids
}

fn normalize_dhcp(item: &Value) -> Map<String, Value> {
    let mut dhcp = Map::new();
    let dhcp_sources = [
        item.pointer("/ipv4Configuration/dhcpConfiguration"),
        item.pointer("/ipv4_configuration/dhcp_configuration"),
        item.get("dhcpConfiguration"),
        item.get("dhcp_configuration"),
    ];
    let ipv4_sources = [
        item.pointer("/ipv4Configuration"),
        item.pointer("/ipv4_configuration"),
    ];
    let legacy_sources = [Some(item)];

    insert_first(
        &mut dhcp,
        "enabled",
        first_field(&dhcp_sources, &["enabled"])
            .or_else(|| first_field(&legacy_sources, &["dhcp_enabled", "dhcpd_enabled"])),
    );
    insert_first(
        &mut dhcp,
        "range_start",
        first_field(&dhcp_sources, &["rangeStart", "range_start"])
            .or_else(|| first_field(&legacy_sources, &["dhcpd_start"])),
    );
    insert_first(
        &mut dhcp,
        "range_end",
        first_field(
            &dhcp_sources,
            &["rangeEnd", "rangeStop", "range_end", "range_stop"],
        )
        .or_else(|| first_field(&legacy_sources, &["dhcpd_stop"])),
    );
    insert_first(
        &mut dhcp,
        "lease_seconds",
        first_field(&dhcp_sources, &["leaseTimeSeconds", "lease_time_seconds"])
            .or_else(|| first_field(&legacy_sources, &["dhcpd_leasetime"])),
    );
    insert_first(
        &mut dhcp,
        "gateway",
        first_field(&ipv4_sources, &["gatewayIpAddress", "gateway_ip"])
            .or_else(|| first_field(&dhcp_sources, &["gatewayIpAddress", "gateway_ip"]))
            .or_else(|| first_field(&legacy_sources, &["dhcpd_gateway"])),
    );

    let mut dns_servers = BTreeSet::new();
    for source in dhcp_sources
        .into_iter()
        .chain(ipv4_sources)
        .chain(legacy_sources)
        .flatten()
    {
        collect_dns_server_addresses(source, &mut dns_servers);
    }
    if !dns_servers.is_empty() {
        dhcp.insert(
            "dns_servers".to_string(),
            Value::Array(dns_servers.into_iter().map(Value::String).collect()),
        );
    }
    dhcp
}

fn insert_first(target: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        target.insert(key.to_string(), redact_json(value));
    }
}

fn collect_dns_server_addresses(value: &Value, out: &mut BTreeSet<String>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, value) in object {
        let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
        let is_address_field = matches!(
            normalized.as_str(),
            "dnsserveripaddresses"
                | "dnsservers"
                | "nameserveripaddresses"
                | "dhcpddns1"
                | "dhcpddns2"
                | "dhcpddns3"
                | "dhcpddns4"
        );
        if is_address_field {
            collect_ip_addresses(value, out);
        }
    }
}

fn collect_ip_addresses(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            for part in value.split([',', ' ']).filter(|part| !part.is_empty()) {
                if let Ok(address) = part.parse::<IpAddr>() {
                    out.insert(address.to_string());
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_ip_addresses(value, out);
            }
        }
        _ => {}
    }
}

fn first_field<'a>(sources: &[Option<&'a Value>], aliases: &[&str]) -> Option<&'a Value> {
    sources
        .iter()
        .flatten()
        .find_map(|source| aliases.iter().find_map(|alias| source.get(*alias)))
}

fn copy_fields(item: &Value, details: &mut BTreeMap<String, Value>, fields: &[&str]) {
    for field in fields {
        if let Some(value) = item.get(*field) {
            details.insert((*field).to_string(), redact_json(value));
        }
    }
}

fn network_scope(network: &NetworkSegment) -> Option<&str> {
    let key = match network.kind.as_str() {
        "unifi_network" => "site_id",
        "unifi_site" => "internal_reference",
        _ => return None,
    };
    network.details.get(key).and_then(Value::as_str)
}

fn merge_network(out: &mut CollectorOutput, mut candidate: NetworkSegment) {
    let candidate_id = candidate.details.get("id").and_then(Value::as_str);
    if let Some(existing) = out.networks.iter_mut().find(|existing| {
        if existing.kind != candidate.kind {
            return false;
        }
        let existing_id = existing.details.get("id").and_then(Value::as_str);
        match (existing_id, candidate_id) {
            (Some(existing_id), Some(candidate_id)) => existing_id == candidate_id,
            _ => {
                existing.name == candidate.name
                    && match (network_scope(existing), network_scope(&candidate)) {
                        (Some(existing_scope), Some(candidate_scope)) => {
                            existing_scope == candidate_scope
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
        }
    }) {
        if existing.name == "network" && candidate.name != "network" {
            existing.name = candidate.name;
        }
        existing.provenance = candidate.provenance;
        for member in candidate.members.drain(..) {
            if !existing.members.contains(&member) {
                existing.members.push(member);
            }
        }
        existing.details.extend(candidate.details);
    } else {
        out.networks.push(candidate);
    }
}

fn bounded_items<'a>(body: &'a Value, path: &str, out: &mut CollectorOutput) -> Vec<&'a Value> {
    let items = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    bounded_slice(items, path, out)
}

fn bounded_items_or_single<'a>(
    body: &'a Value,
    path: &str,
    out: &mut CollectorOutput,
) -> Vec<&'a Value> {
    if let Some(data) = body.get("data") {
        return match data {
            Value::Array(items) => bounded_slice(items, path, out),
            Value::Object(_) => vec![data],
            _ => Vec::new(),
        };
    }
    if let Some(items) = body.as_array() {
        return bounded_slice(items, path, out);
    }
    body.is_object().then_some(body).into_iter().collect()
}

fn bounded_slice<'a>(items: &'a [Value], path: &str, out: &mut CollectorOutput) -> Vec<&'a Value> {
    if items.len() > MAX_UNIFI_ITEMS {
        out.warn(
            path,
            format!(
                "UniFi endpoint {path} returned {} records; truncating to {MAX_UNIFI_ITEMS}",
                items.len()
            ),
        );
        if let Some(error) = out.errors.last_mut() {
            error.truncated = true;
        }
    }
    items.iter().take(MAX_UNIFI_ITEMS).collect()
}

fn string_field<'a>(item: &'a Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| item.get(*field).and_then(Value::as_str))
}

fn provenance(url: &str, path: &str) -> Provenance {
    Provenance::new(
        format!("{}{}", url.trim_end_matches('/'), path),
        "source_inventory",
        Utc::now().to_rfc3339(),
    )
}
