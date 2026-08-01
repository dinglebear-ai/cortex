use futures_util::{StreamExt, stream::FuturesUnordered};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use crate::inventory::adguard::collect_body as collect_adguard_body;
use crate::inventory::collectors::CollectorOutput;
use crate::inventory::limits::{MAX_RAW_ARTIFACT_BYTES, MAX_RAW_BATCH_OUTPUT_BYTES};
use crate::inventory::raw_configs::{collect_compose_body, collect_proxy_body};
use crate::inventory::ssh::{SshContext, configured_hosts as resolve_ssh_hosts};
use crate::inventory::storage::InventoryPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteConfigKind {
    Compose,
    Proxy,
    AdGuard,
}

impl RemoteConfigKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "compose" => Some(Self::Compose),
            "proxy" => Some(Self::Proxy),
            "adguard" => Some(Self::AdGuard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteConfigRecord {
    kind: RemoteConfigKind,
    path: String,
    body: String,
}

pub async fn collect(
    ssh_config: Option<&Path>,
    configured_hosts: &[String],
    ssh_context: &SshContext,
    paths: &InventoryPaths,
    run_id: &str,
    probe_timeout: Duration,
    collector_timeout: Duration,
) -> CollectorOutput {
    let resolution = resolve_ssh_hosts(ssh_config, configured_hosts);
    let mut out = CollectorOutput::new("raw_configs");
    for warning in &resolution.warnings {
        out.warn("host_resolution", warning);
    }
    if resolution.no_usable_explicit_hosts() {
        out.warn(
            "host_resolution",
            "remote config collector skipped because no explicitly configured SSH hosts were usable",
        );
        return out;
    }

    let mut tasks = FuturesUnordered::new();
    let mut pending_hosts = BTreeSet::new();
    for host in resolution.hosts {
        pending_hosts.insert(host.clone());
        let paths = paths.clone();
        let run_id = run_id.to_string();
        let ssh_context = ssh_context.clone();
        tasks.push(async move {
            let host_output =
                collect_host(&host, &ssh_context, &paths, &run_id, probe_timeout).await;
            (host, host_output)
        });
    }

    let deadline = tokio::time::sleep(soft_collector_budget(collector_timeout));
    tokio::pin!(deadline);
    while !tasks.is_empty() {
        tokio::select! {
            next = tasks.next() => {
                let Some((host, host_output)) = next else {
                    break;
                };
                pending_hosts.remove(&host);
                merge_output(&mut out, host_output);
            }
            _ = &mut deadline => {
                if !pending_hosts.is_empty() {
                    out.warn(
                        "remote_config_deadline",
                        format!(
                            "remote config deadline reached; preserved completed hosts and skipped unfinished hosts: {}",
                            pending_hosts.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                    );
                }
                break;
            }
        }
    }
    out
}

async fn collect_host(
    host: &str,
    ssh_context: &SshContext,
    paths: &InventoryPaths,
    run_id: &str,
    timeout: Duration,
) -> CollectorOutput {
    let mut out = CollectorOutput::new("raw_configs");
    let records = remote_records(&mut out, host, ssh_context, timeout).await;
    for record in records {
        let source_path = format!("{host}:{}", record.path);
        match record.kind {
            RemoteConfigKind::Compose => match collect_compose_body(
                Some(host.to_string()),
                source_path,
                record.body,
                paths,
                run_id,
            ) {
                Ok((artifact, project)) => {
                    out.artifacts.push(artifact);
                    out.compose_projects.push(project);
                }
                Err(error) => out.warn("remote_compose", error.to_string()),
            },
            RemoteConfigKind::Proxy => match collect_proxy_body(
                Some(host.to_string()),
                source_path,
                record.body,
                paths,
                run_id,
            ) {
                Ok((artifact, routes)) => {
                    out.artifacts.push(artifact);
                    out.reverse_proxies.extend(routes);
                }
                Err(error) => out.warn("remote_proxy", error.to_string()),
            },
            RemoteConfigKind::AdGuard => match collect_adguard_body(
                Some(host.to_string()),
                source_path,
                record.body,
                paths,
                run_id,
            ) {
                Ok((artifact, service)) => {
                    out.artifacts.push(artifact);
                    out.services.push(service);
                }
                Err(error) => out.warn("remote_adguard", error.to_string()),
            },
        }
    }
    out
}

async fn remote_records(
    out: &mut CollectorOutput,
    host: &str,
    ssh_context: &SshContext,
    timeout: Duration,
) -> Vec<RemoteConfigRecord> {
    let command = config_batch_command();
    match ssh_context
        .run_capped(host, &command, timeout, MAX_RAW_BATCH_OUTPUT_BYTES)
        .await
    {
        Ok(output) if output.status == Some(0) => {
            if output.truncated {
                out.warn(
                    "remote_config_truncated",
                    format!(
                        "ssh config collection output was truncated on {host}; complete records were preserved"
                    ),
                );
            }
            parse_records(&output.stdout)
        }
        Ok(output) => {
            out.warn(
                "remote_config",
                format!("ssh config collection failed on {host}: {}", output.stderr),
            );
            Vec::new()
        }
        Err(error) => {
            out.warn(
                "remote_config",
                format!("ssh config collection failed on {host}: {error}"),
            );
            Vec::new()
        }
    }
}

fn config_batch_command() -> String {
    [
        framed_batch_command("adguard", adguard_find_command()),
        framed_batch_command("proxy", proxy_find_command()),
        framed_batch_command("compose", compose_find_command()),
    ]
    .join("; ")
}

fn compose_find_command() -> &'static str {
    r#"for d in "$HOME/compose" "$HOME/.cortex/compose" "$HOME/.axon/compose" "$HOME/workspace" /mnt/compose /mnt/cache/compose /mnt/user/compose /mnt/appdata /mnt/cache/appdata /mnt/user/appdata /opt /srv; do [ -d "$d" ] && find "$d" -maxdepth 4 -type f \( -name docker-compose.yml -o -name docker-compose.yaml -o -name compose.yml -o -name compose.yaml \) -print 2>/dev/null; done | sort -u | head -200"#
}

fn proxy_find_command() -> &'static str {
    r#"for d in /mnt/appdata/swag/nginx/proxy-confs /mnt/cache/appdata/swag/nginx/proxy-confs /mnt/user/appdata/swag/nginx/proxy-confs "$HOME/swag/nginx/proxy-confs" "$HOME/compose/swag/nginx/proxy-confs"; do [ -d "$d" ] && find "$d" -maxdepth 1 -type f -name '*.conf' -print 2>/dev/null; done | sort -u | head -300"#
}

fn adguard_find_command() -> &'static str {
    r#"{ for d in /mnt/appdata/adguard/etc /mnt/cache/appdata/adguard/etc /mnt/user/appdata/adguard/etc "$HOME/adguard" "$HOME/compose/adguard"; do [ -d "$d" ] && find "$d" -maxdepth 2 -type f \( -name config.yaml -o -name AdGuardHome.yaml \) -print 2>/dev/null; done; for f in /opt/AdGuardHome/AdGuardHome.yaml /opt/adguardhome/conf/AdGuardHome.yaml /etc/AdGuardHome.yaml; do [ -f "$f" ] && printf '%s\n' "$f"; done; } | sort -u | head -20"#
}

fn framed_batch_command(kind: &str, find_command: &str) -> String {
    format!(
        r#"{find_command} | while IFS= read -r f; do [ -f "$f" ] || continue; printf '\036{kind}\t%s\n' "$f"; head -c {} -- "$f"; printf '\037\n'; done"#,
        MAX_RAW_ARTIFACT_BYTES + 1
    )
}

fn parse_records(stdout: &str) -> Vec<RemoteConfigRecord> {
    stdout
        .split('\u{1e}')
        .skip(1)
        .filter_map(|record| {
            let (header, framed_body) = record.split_once('\n')?;
            let (kind, path) = header.split_once('\t')?;
            let body = framed_body
                .strip_suffix("\u{1f}\n")
                .or_else(|| framed_body.strip_suffix('\u{1f}'))?;
            Some(RemoteConfigRecord {
                kind: RemoteConfigKind::parse(kind)?,
                path: path.to_string(),
                body: body.trim_end_matches('\n').to_string(),
            })
        })
        .collect()
}

fn soft_collector_budget(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        return timeout;
    }
    let margin_ms = (timeout.as_millis() / 20).clamp(1, 250) as u64;
    timeout.saturating_sub(Duration::from_millis(margin_ms))
}

fn merge_output(out: &mut CollectorOutput, remote: CollectorOutput) {
    out.services.extend(remote.services);
    out.compose_projects.extend(remote.compose_projects);
    out.reverse_proxies.extend(remote.reverse_proxies);
    out.artifacts.extend(remote.artifacts);
    out.errors.extend(remote.errors);
    out.warnings.extend(remote.warnings);
}

#[cfg(test)]
#[path = "remote_configs_tests.rs"]
mod tests;
