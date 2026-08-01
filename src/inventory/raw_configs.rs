use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::limits::MAX_RAW_ARTIFACT_BYTES;
use crate::inventory::redaction::RedactedArtifact;
use crate::inventory::schema::{
    ArtifactRef, ComposeProject, PortMapping, Provenance, ReverseProxyRoute,
};
use crate::inventory::ssh::SshContext;
use crate::inventory::storage::{InventoryPaths, write_artifact};

#[path = "raw_configs_nginx.rs"]
mod nginx;
use nginx::substitute_nginx_variables;

pub struct CollectOptions<'a> {
    pub compose_paths: &'a [PathBuf],
    pub proxy_paths: &'a [PathBuf],
    pub adguard_paths: &'a [PathBuf],
    pub ssh_config: Option<&'a Path>,
    pub ssh_hosts: &'a [String],
    pub ssh_context: &'a SshContext,
    pub paths: &'a InventoryPaths,
    pub run_id: &'a str,
    pub probe_timeout: Duration,
    pub collector_timeout: Duration,
}

pub async fn collect(options: CollectOptions<'_>) -> CollectorOutput {
    let compose_paths = options.compose_paths.to_vec();
    let proxy_paths = options.proxy_paths.to_vec();
    let adguard_paths = options.adguard_paths.to_vec();
    let paths = options.paths.clone();
    let run_id = options.run_id.to_string();
    let local_paths = paths.clone();
    let local_run_id = run_id.clone();
    let local_deadline = Instant::now() + soft_collector_budget(options.collector_timeout);
    let local = tokio::task::spawn_blocking(move || {
        collect_blocking(
            &compose_paths,
            &proxy_paths,
            &adguard_paths,
            &local_paths,
            &local_run_id,
            local_deadline,
        )
    });
    let remote = crate::inventory::remote_configs::collect(
        options.ssh_config,
        options.ssh_hosts,
        options.ssh_context,
        &paths,
        &run_id,
        options.probe_timeout,
        options.collector_timeout,
    );
    let (local, remote) = tokio::join!(local, remote);
    let mut out = local.unwrap_or_else(|error| {
        let mut out = CollectorOutput::new("raw_configs");
        out.warn(
            "task",
            format!("raw config collection task failed: {error}"),
        );
        out
    });
    merge_output(&mut out, remote);
    out
}

fn collect_blocking(
    compose_paths: &[PathBuf],
    proxy_paths: &[PathBuf],
    adguard_paths: &[PathBuf],
    paths: &InventoryPaths,
    run_id: &str,
    deadline: Instant,
) -> CollectorOutput {
    let mut out = CollectorOutput::new("raw_configs");
    for path in expand_adguard_files(adguard_paths) {
        if stop_local_collection(&mut out, deadline, "AdGuard") {
            return out;
        }
        match crate::inventory::adguard::collect_file(&path, paths, run_id) {
            Ok((artifact, service)) => {
                out.artifacts.push(artifact);
                out.services.push(service);
            }
            Err(error) => out.warn("adguard", error.to_string()),
        }
    }
    for path in expand_files(proxy_paths, &["conf"]) {
        if stop_local_collection(&mut out, deadline, "reverse proxy") {
            return out;
        }
        match collect_proxy_file(&path, paths, run_id) {
            Ok((artifact, routes)) => {
                out.artifacts.push(artifact);
                out.reverse_proxies.extend(routes);
            }
            Err(error) => out.warn("reverse_proxy", error.to_string()),
        }
    }
    for path in expand_files(compose_paths, &["yml", "yaml"]) {
        if stop_local_collection(&mut out, deadline, "Compose") {
            return out;
        }
        match collect_compose_file(&path, paths, run_id) {
            Ok((artifact, project)) => {
                out.artifacts.push(artifact);
                out.compose_projects.push(project);
            }
            Err(error) => out.warn("compose", error.to_string()),
        }
    }
    out
}

fn stop_local_collection(out: &mut CollectorOutput, deadline: Instant, phase: &str) -> bool {
    if Instant::now() < deadline {
        return false;
    }
    out.warn(
        "local_config_deadline",
        format!(
            "local raw config deadline reached before {phase} collection completed; preserved completed results"
        ),
    );
    true
}

fn soft_collector_budget(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        return timeout;
    }
    let margin_ms = (timeout.as_millis() / 20).clamp(1, 250) as u64;
    timeout.saturating_sub(Duration::from_millis(margin_ms))
}

fn collect_compose_file(
    path: &Path,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, ComposeProject)> {
    let body = read_bounded_text(path)?;
    collect_compose_body(None, path.display().to_string(), body, paths, run_id)
}

pub(in crate::inventory) fn collect_compose_body(
    source_host: Option<String>,
    source_path: String,
    body: String,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, ComposeProject)> {
    let artifact_id = artifact_id("compose", &source_path);
    let artifact = RedactedArtifact::from_text(&body, MAX_RAW_ARTIFACT_BYTES);
    let reference = write_artifact(
        paths,
        run_id,
        &artifact_id,
        &artifact,
        ArtifactRef {
            id: artifact_id.clone(),
            kind: "compose_yaml".to_string(),
            collector: "raw_configs".to_string(),
            source_host,
            source_path: Some(source_path.clone()),
            cache_path: String::new(),
            redaction: artifact.status(),
            byte_len: 0,
            truncated: artifact.truncated(),
        },
    )?;
    Ok((
        reference,
        parse_compose_project(Path::new(&source_path), artifact.body()),
    ))
}

fn collect_proxy_file(
    path: &Path,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, Vec<ReverseProxyRoute>)> {
    let body = read_bounded_text(path)?;
    collect_proxy_body(None, path.display().to_string(), body, paths, run_id)
}

pub(in crate::inventory) fn collect_proxy_body(
    source_host: Option<String>,
    source_path: String,
    body: String,
    paths: &InventoryPaths,
    run_id: &str,
) -> Result<(ArtifactRef, Vec<ReverseProxyRoute>)> {
    let artifact_id = artifact_id("proxy", &source_path);
    let artifact = RedactedArtifact::from_text(&body, MAX_RAW_ARTIFACT_BYTES);
    let reference = write_artifact(
        paths,
        run_id,
        &artifact_id,
        &artifact,
        ArtifactRef {
            id: artifact_id.clone(),
            kind: "reverse_proxy_conf".to_string(),
            collector: "raw_configs".to_string(),
            source_host,
            source_path: Some(source_path.clone()),
            cache_path: String::new(),
            redaction: artifact.status(),
            byte_len: 0,
            truncated: artifact.truncated(),
        },
    )?;
    Ok((
        reference,
        parse_proxy_routes(Path::new(&source_path), artifact.body()),
    ))
}

fn read_bounded_text(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut body = String::new();
    file.take((MAX_RAW_ARTIFACT_BYTES + 1) as u64)
        .read_to_string(&mut body)?;
    Ok(body)
}

// Best-effort Compose discovery for redacted inventory summaries. This is not a
// full YAML parser: it treats `services:` as a top-level marker, service names as
// two-space-indented keys directly under that section, and extracts only
// single-line domains plus list-item port mappings under a `ports:` key through
// `extract_domainish` and `parse_port_line`. Anchors, flow style, interpolation,
// multi-line values, profiles, includes, and nested semantic merges are
// intentionally left to the preserved redacted raw artifact; use a real YAML
// parser if Cortex needs broader Compose correctness.
fn parse_compose_project(path: &Path, body: &str) -> ComposeProject {
    let mut services = Vec::new();
    let mut domains = Vec::new();
    let mut ports = Vec::new();
    let mut in_services = false;
    let mut ports_indent = None;
    for line in body.lines() {
        let trimmed = line.trim();
        let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        if trimmed == "services:" {
            in_services = true;
            continue;
        }
        if trimmed == "ports:" {
            ports_indent = Some(indent);
            continue;
        }
        if in_services && !line.starts_with(' ') && !trimmed.is_empty() {
            in_services = false;
        }
        if let Some(port_indent) = ports_indent
            && !trimmed.is_empty()
            && indent <= port_indent
        {
            ports_indent = None;
        }
        if in_services
            && line.starts_with("  ")
            && !line.starts_with("    ")
            && trimmed.ends_with(':')
        {
            services.push(trimmed.trim_end_matches(':').to_string());
        }
        if trimmed.contains("server_name")
            || trimmed.contains("Host(")
            || trimmed.contains("domain")
        {
            domains.extend(extract_domainish(trimmed));
        }
        if ports_indent.is_some()
            && trimmed.starts_with('-')
            && let Some(port) = parse_port_line(trimmed)
        {
            ports.push(port);
        }
    }
    ComposeProject {
        name: path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("compose")
            .to_string(),
        provenance: Provenance::new(
            path.display().to_string(),
            "source_inventory",
            Utc::now().to_rfc3339(),
        ),
        services,
        compose_files: vec![path.display().to_string()],
        domains,
        ports,
    }
}

fn parse_proxy_routes(path: &Path, body: &str) -> Vec<ReverseProxyRoute> {
    let mut variables = BTreeMap::new();
    let mut server_names = Vec::new();
    let mut raw_upstreams = Vec::new();
    for line in body.lines() {
        let uncommented = line.split('#').next().unwrap_or_default();
        for raw_directive in uncommented.split(';') {
            let directive = raw_directive
                .rsplit('{')
                .next()
                .unwrap_or(raw_directive)
                .trim();
            if let Some(rest) = directive.strip_prefix("server_name ") {
                for name in rest.split_whitespace() {
                    push_unique(&mut server_names, name.to_string());
                }
            }
            if let Some(rest) = directive.strip_prefix("set $") {
                let mut parts = rest.split_whitespace();
                if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                    variables.insert(name.to_string(), value.to_string());
                }
            }
            if let Some(rest) = directive.strip_prefix("proxy_pass ") {
                raw_upstreams.push(rest.trim().to_string());
            }
        }
    }
    let mut upstreams = Vec::new();
    for upstream in raw_upstreams {
        push_unique(
            &mut upstreams,
            substitute_nginx_variables(&upstream, &variables),
        );
    }
    if server_names.is_empty() && upstreams.is_empty() {
        return Vec::new();
    }
    vec![ReverseProxyRoute {
        id: artifact_id("route", &path.display().to_string()),
        server_names,
        upstreams,
        provenance: Provenance::new(
            path.display().to_string(),
            "source_inventory",
            Utc::now().to_rfc3339(),
        ),
    }]
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

fn parse_port_line(line: &str) -> Option<PortMapping> {
    let quoted = line
        .trim_start_matches('-')
        .trim()
        .trim_matches(&['"', '\''][..]);
    let parts = quoted.split(':').collect::<Vec<_>>();
    let (host_ip, host, container) = match parts.as_slice() {
        [host, container] => (None, *host, *container),
        [ip, host, container] => (Some((*ip).to_string()), *host, *container),
        _ => return None,
    };
    let host_port = host.parse().ok();
    let container_port = container.split('/').next().and_then(|p| p.parse().ok());
    if host_port.is_none() && container_port.is_none() {
        return None;
    }
    Some(PortMapping {
        host_ip,
        host_port,
        container_port,
        protocol: container.split('/').nth(1).unwrap_or("tcp").to_string(),
    })
}

fn extract_domainish(line: &str) -> Vec<String> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .filter(|part| part.contains('.') && part.len() > 3)
        .map(ToString::to_string)
        .collect()
}

fn expand_adguard_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    expand_files(paths, &["yaml", "yml"])
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "config.yaml" | "AdGuardHome.yaml"))
        })
        .collect()
}

fn expand_files(paths: &[PathBuf], extensions: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_file() {
            out.push(path.clone());
        } else if path.is_dir()
            && let Ok(entries) = std::fs::read_dir(path)
        {
            out.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| extensions.contains(&ext))
            }));
        }
    }
    out.sort();
    out
}

fn merge_output(out: &mut CollectorOutput, remote: CollectorOutput) {
    out.nodes.extend(remote.nodes);
    out.services.extend(remote.services);
    out.compose_projects.extend(remote.compose_projects);
    out.reverse_proxies.extend(remote.reverse_proxies);
    out.networks.extend(remote.networks);
    out.storage.extend(remote.storage);
    out.media_services.extend(remote.media_services);
    out.projects.extend(remote.projects);
    out.artifacts.extend(remote.artifacts);
    out.errors.extend(remote.errors);
    out.warnings.extend(remote.warnings);
}

fn artifact_id(prefix: &str, source: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(source.as_bytes());
    let digest_hex = format!("{digest:x}");
    format!("{prefix}:{}", &digest_hex[..32])
}

#[cfg(test)]
#[path = "raw_configs_tests.rs"]
mod tests;
