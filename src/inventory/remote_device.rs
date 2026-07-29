use chrono::Utc;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::schema::{
    InventoryNode, ListenerFact, Provenance, StorageSummary, TrustLevel,
};
use crate::inventory::ssh::{SshContext, configured_hosts as resolve_ssh_hosts};

pub async fn collect(
    ssh_config: Option<&Path>,
    configured_hosts: &[String],
    ssh_context: &SshContext,
    timeout: Duration,
) -> CollectorOutput {
    let resolution = resolve_ssh_hosts(ssh_config, configured_hosts);
    let mut out = CollectorOutput::new("remote_device");
    for warning in &resolution.warnings {
        out.warn("host_resolution", warning);
    }
    if resolution.no_usable_explicit_hosts() {
        out.warn(
            "host_resolution",
            "remote device collector skipped because no explicitly configured SSH hosts were usable",
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
                "remote_device",
                format!("remote device task failed: {error}"),
            ),
        }
    }
    out
}

async fn collect_host(host: String, ssh_context: SshContext, timeout: Duration) -> CollectorOutput {
    let mut out = CollectorOutput::new("remote_device");
    match ssh_context.run(&host, device_command(), timeout).await {
        Ok(output) if output.status == Some(0) => normalize_host(&host, &output.stdout, &mut out),
        Ok(output) => out.warn(
            "probe",
            format!("remote device probe failed on {host}: {}", output.stderr),
        ),
        Err(error) => out.warn(
            "probe",
            format!("remote device probe failed on {host}: {error}"),
        ),
    }
    out
}

fn normalize_host(host_alias: &str, body: &str, out: &mut CollectorOutput) {
    let mut facts = BTreeMap::<String, String>::new();
    let mut ips = Vec::new();
    let mut listeners = Vec::new();
    let mut storage = Vec::new();

    for line in body.lines() {
        if let Some(value) = line.strip_prefix("ip=") {
            push_unique(&mut ips, value.to_string());
        } else if let Some(value) = line.strip_prefix("listener=") {
            if let Some(listener) = parse_listener(value) {
                listeners.push(listener);
            }
        } else if let Some(value) = line.strip_prefix("storage=") {
            if let Some(summary) = parse_storage(host_alias, value) {
                storage.push(summary);
            }
        } else if let Some((key, value)) = line.split_once('=')
            && !value.trim().is_empty()
        {
            facts.insert(key.to_string(), value.trim().to_string());
        }
    }
    for ip in facts
        .get("tailscale_ip")
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|ip| !ip.is_empty())
    {
        push_unique(&mut ips, ip.to_string());
    }

    let collected_at = Utc::now().to_rfc3339();
    let hostname = facts
        .get("hostname")
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| host_alias.to_string());
    let mut extras = BTreeMap::new();
    insert_extra(&mut extras, "ssh_alias", host_alias);
    insert_optional_extra(&mut extras, "fqdn", facts.get("fqdn"));
    insert_optional_extra(&mut extras, "cores", facts.get("cores"));
    insert_optional_extra(&mut extras, "gpu", facts.get("gpu"));
    insert_optional_extra(&mut extras, "tailscale_ip", facts.get("tailscale_ip"));

    out.nodes.push(InventoryNode {
        id: format!("host:{hostname}"),
        hostname,
        trust_level: TrustLevel::Observed,
        provenance: Provenance::new(
            format!("{host_alias}:ssh device probe"),
            "source_inventory",
            collected_at,
        ),
        roles: vec!["ssh_inventory_host".to_string()],
        ips,
        os: facts
            .get("os")
            .or_else(|| facts.get("kernel"))
            .cloned()
            .filter(|value| !value.is_empty()),
        cpu: facts.get("cpu").cloned().filter(|value| !value.is_empty()),
        memory: facts
            .get("memory")
            .cloned()
            .filter(|value| !value.is_empty()),
        listeners,
        storage: storage.clone(),
        extras,
    });
    out.storage.extend(storage);
}

fn parse_listener(line: &str) -> Option<ListenerFact> {
    let cols = line.split_whitespace().collect::<Vec<_>>();
    let protocol = cols.first()?.to_ascii_lowercase();
    let bind = cols.get(4)?.to_string();
    let port = bind.rsplit(':').next().and_then(|part| part.parse().ok());
    Some(ListenerFact {
        protocol,
        bind,
        port,
        process: None,
    })
}

fn parse_storage(host: &str, line: &str) -> Option<StorageSummary> {
    let cols = line.split('\t').collect::<Vec<_>>();
    if cols.len() < 4 {
        return None;
    }
    let mount = cols[3].to_string();
    Some(StorageSummary {
        id: format!("storage:{host}:{mount}"),
        mount,
        fs_type: Some(cols[0].to_string()),
        total_bytes: cols[1].parse::<u64>().ok().map(|kb| kb * 1024),
        available_bytes: cols[2].parse::<u64>().ok().map(|kb| kb * 1024),
        provenance: Provenance::new(
            format!("{host}:df -PT"),
            "source_inventory",
            Utc::now().to_rfc3339(),
        ),
    })
}

fn device_command() -> &'static str {
    r#"printf 'hostname=%s\n' "$(hostname 2>/dev/null)"
printf 'fqdn=%s\n' "$(hostname -f 2>/dev/null)"
printf 'os=%s\n' "$(. /etc/os-release 2>/dev/null; printf '%s' "${PRETTY_NAME:-}")"
printf 'kernel=%s\n' "$(uname -srmo 2>/dev/null)"
printf 'cpu=%s\n' "$(lscpu 2>/dev/null | awk -F: '/Model name|Model name:/ {gsub(/^[ \t]+/, "", $2); print $2; exit}')"
printf 'cores=%s\n' "$(nproc 2>/dev/null)"
printf 'memory=%s\n' "$(free -h 2>/dev/null | awk '/^Mem:/ {print $2; exit}')"
if command -v tailscale >/dev/null 2>&1; then printf 'tailscale_ip=%s\n' "$(tailscale ip -4 2>/dev/null | paste -sd, -)"; fi
ip -o -4 addr show scope global 2>/dev/null | awk '{split($4,a,"/"); print "ip=" a[1]}' | head -50
ss -lntu 2>/dev/null | awk 'NR > 1 {print "listener=" $0}' | head -200
df -PT -x tmpfs -x devtmpfs -x overlay -x squashfs 2>/dev/null | awk 'NR > 1 {print "storage=" $2 "\t" $3 "\t" $5 "\t" $7}' | head -80
if command -v nvidia-smi >/dev/null 2>&1; then
  printf 'gpu=%s\n' "$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | paste -sd, -)"
elif command -v lspci >/dev/null 2>&1; then
  printf 'gpu=%s\n' "$(lspci 2>/dev/null | grep -Ei 'vga|3d|display|nvidia|amd|radeon|intel' | head -5 | paste -sd ';' -)"
fi"#
}

fn insert_extra(extras: &mut BTreeMap<String, serde_json::Value>, key: &str, value: &str) {
    extras.insert(key.to_string(), json!(value));
}

fn insert_optional_extra(
    extras: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    value: Option<&String>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        insert_extra(extras, key, value);
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn merge_output(out: &mut CollectorOutput, remote: CollectorOutput) {
    out.nodes.extend(remote.nodes);
    out.storage.extend(remote.storage);
    out.errors.extend(remote.errors);
    out.warnings.extend(remote.warnings);
}

#[cfg(test)]
#[path = "remote_device_tests.rs"]
mod tests;
