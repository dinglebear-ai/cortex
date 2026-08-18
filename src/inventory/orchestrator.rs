use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::Serialize;
use std::collections::HashSet;
use std::time::Instant;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::config::InventoryConfig;
use crate::inventory::limits::{INVENTORY_SCHEMA, MAX_SECTION_ITEMS, cap_vec};
use crate::inventory::schema::{
    CollectionState, CollectorState, GraphProjectionSummary, HomelabInventory,
};
use crate::inventory::storage::{InventoryPaths, RefreshLock, write_json_private};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Serialize)]
pub struct InventoryRefreshReport {
    pub run_id: String,
    pub status: String,
    pub root: String,
    pub normalized_path: String,
    pub collection_state_path: String,
    pub started_at: String,
    pub finished_at: String,
    pub elapsed_ms: u128,
    pub collectors: Vec<CollectorState>,
    pub warnings: Vec<String>,
    pub artifact_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InventoryRefreshOutcome {
    pub report: InventoryRefreshReport,
    pub inventory: HomelabInventory,
}

type NamedOutput = (&'static str, String, String, u128, CollectorOutput);
type CollectorFuture<'a> = Pin<Box<dyn Future<Output = NamedOutput> + Send + 'a>>;

pub async fn refresh_inventory(config: InventoryConfig) -> Result<InventoryRefreshReport> {
    Ok(refresh_inventory_with_inventory(config).await?.report)
}

pub async fn refresh_inventory_with_inventory(
    config: InventoryConfig,
) -> Result<InventoryRefreshOutcome> {
    let paths = InventoryPaths::new(config.root.clone());
    paths.ensure_private_dirs()?;
    let _lock = RefreshLock::acquire(&paths.lock_file)?;
    let started = Utc::now().to_rfc3339();
    let run_id = run_id();
    let timer = Instant::now();
    let mut inventory = HomelabInventory::empty(run_id.clone(), started.clone());
    inventory.graph_projection = Some(GraphProjectionSummary {
        status: "not_projected".to_string(),
        source_kinds_reserved: vec!["source_inventory".to_string(), "app_inventory".to_string()],
        next_queries: vec!["cortex graph rebuild after explicit inventory projection".to_string()],
    });

    let mut states = Vec::new();
    let mut all_warnings = Vec::new();
    let mut collector_outputs = Vec::new();
    let ssh_context = crate::inventory::ssh::SshContext::new(
        crate::inventory::ssh::SshOptions::from_env(config.ssh_config.as_deref()),
    );

    // Run all collectors concurrently. Each collector has an individual timeout,
    // and the whole batch has a separate wall-clock deadline.
    let collector_deadline = config.collector_deadline;
    let probe_deadline = config.probe_deadline;
    const COLLECTOR_NAMES: [&str; 10] = [
        "raw_configs",
        "device",
        "remote_device",
        "docker",
        "remote_docker",
        "tailscale",
        "unraid",
        "unifi",
        "media_stack",
        "projects",
    ];

    let futures: Vec<CollectorFuture<'_>> = vec![
        collector_task(
            "raw_configs",
            collector_deadline,
            crate::inventory::raw_configs::collect(crate::inventory::raw_configs::CollectOptions {
                compose_paths: &config.compose_paths,
                proxy_paths: &config.proxy_paths,
                adguard_paths: &config.adguard_paths,
                ssh_config: config.ssh_config.as_deref(),
                ssh_hosts: &config.ssh_hosts,
                ssh_context: &ssh_context,
                paths: &paths,
                run_id: &run_id,
                probe_timeout: probe_deadline,
                collector_timeout: collector_deadline,
            }),
        ),
        collector_task(
            "device",
            probe_deadline,
            crate::inventory::device::collect(probe_deadline),
        ),
        collector_task(
            "remote_device",
            collector_deadline,
            crate::inventory::remote_device::collect(
                config.ssh_config.as_deref(),
                &config.ssh_hosts,
                &ssh_context,
                probe_deadline,
            ),
        ),
        collector_task(
            "docker",
            collector_deadline,
            crate::inventory::docker::collect(&config.docker_hosts, collector_deadline),
        ),
        collector_task(
            "remote_docker",
            collector_deadline,
            crate::inventory::remote_docker::collect(
                config.ssh_config.as_deref(),
                &config.ssh_hosts,
                &ssh_context,
                collector_deadline,
            ),
        ),
        collector_task(
            "tailscale",
            probe_deadline,
            crate::inventory::tailscale::collect(probe_deadline),
        ),
        collector_task(
            "unraid",
            collector_deadline,
            crate::inventory::unraid::collect(
                config.unraid_url.as_deref(),
                config.unraid_api_key.as_deref(),
                collector_deadline,
            ),
        ),
        collector_task(
            "unifi",
            collector_deadline,
            crate::inventory::unifi::collect(
                config.unifi_url.as_deref(),
                config.unifi_api_key.as_deref(),
                collector_deadline,
            ),
        ),
        collector_task(
            "media_stack",
            collector_deadline,
            crate::inventory::media_stack::collect(&config.media_services, collector_deadline),
        ),
        collector_task(
            "projects",
            collector_deadline,
            crate::inventory::projects::collect(&config.project_roots, probe_deadline),
        ),
    ];

    let results = collect_until_deadline(
        futures,
        &COLLECTOR_NAMES,
        config.collection_deadline,
        &mut all_warnings,
    )
    .await;

    for result in results {
        run_collector(
            result,
            &mut states,
            &mut all_warnings,
            &mut collector_outputs,
        );
    }

    for output in collector_outputs {
        inventory.nodes.extend(output.nodes);
        inventory.services.extend(output.services);
        inventory.compose_projects.extend(output.compose_projects);
        inventory.reverse_proxies.extend(output.reverse_proxies);
        inventory.networks.extend(output.networks);
        inventory.storage.extend(output.storage);
        inventory.media_services.extend(output.media_services);
        inventory.projects.extend(output.projects);
        inventory.artifact_refs.extend(output.artifacts);
        inventory.collection_errors.extend(output.errors);
    }
    apply_section_caps(&mut inventory);
    inventory.recompute_summary();
    let finished = Utc::now().to_rfc3339();
    let has_output = inventory_has_output(&inventory);
    let has_collection_errors = !inventory.collection_errors.is_empty();
    let status = if has_output && !has_collection_errors {
        "success"
    } else {
        "partial"
    }
    .to_string();
    let state = CollectionState {
        schema: INVENTORY_SCHEMA.to_string(),
        run_id: run_id.clone(),
        started_at: started.clone(),
        finished_at: finished.clone(),
        status: status.clone(),
        collectors: states.clone(),
        artifact_refs: inventory.artifact_refs.clone(),
        errors: inventory.collection_errors.clone(),
    };
    write_json_private(&paths.normalized_json, &inventory)
        .context("write normalized inventory cache")?;
    write_json_private(&paths.collection_state_json, &state).context("write collection state")?;
    let report = InventoryRefreshReport {
        run_id,
        status,
        root: paths.root.display().to_string(),
        normalized_path: paths.normalized_json.display().to_string(),
        collection_state_path: paths.collection_state_json.display().to_string(),
        started_at: started,
        finished_at: finished,
        elapsed_ms: timer.elapsed().as_millis(),
        collectors: states,
        warnings: all_warnings,
        artifact_paths: inventory
            .artifact_refs
            .iter()
            .map(|artifact| artifact.cache_path.clone())
            .collect(),
    };
    Ok(InventoryRefreshOutcome { report, inventory })
}

async fn collect_until_deadline<'a>(
    futures: Vec<CollectorFuture<'a>>,
    collector_names: &[&'static str],
    deadline: std::time::Duration,
    warnings: &mut Vec<String>,
) -> Vec<NamedOutput> {
    if deadline.is_zero() {
        drop(futures);
        return collection_timeout_results(
            collector_names.iter().copied(),
            deadline.as_millis(),
            warnings,
        );
    }

    let mut pending: HashSet<&'static str> = collector_names.iter().copied().collect();
    let mut running: FuturesUnordered<CollectorFuture<'a>> = futures.into_iter().collect();
    let mut results = Vec::with_capacity(collector_names.len());
    let sleep = tokio::time::sleep(deadline);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            _ = &mut sleep => break,
            result = running.next() => {
                let Some(result) = result else { break; };
                pending.remove(result.0);
                results.push(result);
            }
        }
    }

    if !pending.is_empty() {
        results.extend(collection_timeout_results(
            pending,
            deadline.as_millis(),
            warnings,
        ));
    }

    // Preserve the historical collector order even though completion is now
    // incremental. Stable report ordering makes diffs and operator tooling
    // deterministic.
    results.sort_by_key(|result| {
        collector_names
            .iter()
            .position(|name| *name == result.0)
            .unwrap_or(usize::MAX)
    });
    results
}

fn collector_task<'a, F>(
    name: &'static str,
    deadline: std::time::Duration,
    future: F,
) -> CollectorFuture<'a>
where
    F: Future<Output = CollectorOutput> + Send + 'a,
{
    Box::pin(async move {
        let started = Utc::now().to_rfc3339();
        let t = Instant::now();
        let out = match tokio::time::timeout(deadline, future).await {
            Ok(output) => output,
            Err(_) => timeout_output(name, deadline),
        };
        (
            name,
            started,
            Utc::now().to_rfc3339(),
            t.elapsed().as_millis(),
            out,
        )
    })
}

fn timeout_output(name: &'static str, deadline: std::time::Duration) -> CollectorOutput {
    let mut out = CollectorOutput::new(name);
    out.warn(
        "collection_timeout",
        format!("collector {name} exceeded {}ms", deadline.as_millis()),
    );
    out
}

fn collection_timeout_results(
    collector_names: impl IntoIterator<Item = &'static str>,
    deadline_ms: u128,
    warnings: &mut Vec<String>,
) -> Vec<NamedOutput> {
    let warning = format!("inventory collection exceeded {deadline_ms}ms");
    warnings.push(warning.clone());
    collector_names
        .into_iter()
        .map(|name| {
            let now = Utc::now().to_rfc3339();
            let mut output = CollectorOutput::new(name);
            output.warn("collection_timeout", warning.clone());
            (name, now.clone(), now, deadline_ms, output)
        })
        .collect()
}

fn run_collector(
    result: NamedOutput,
    states: &mut Vec<CollectorState>,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<CollectorOutput>,
) {
    let (name, started_at, finished_at, elapsed_ms, output) = result;
    warnings.extend(output.warnings.iter().cloned());
    let status = if output
        .errors
        .iter()
        .any(|e| e.phase == "collection_timeout")
    {
        "skipped"
    } else if output.errors.iter().any(|e| e.severity == "error") {
        "failed"
    } else if output.warnings.is_empty() {
        "ok"
    } else {
        "partial"
    };
    states.push(CollectorState {
        name: name.to_string(),
        status: status.to_string(),
        started_at,
        finished_at,
        elapsed_ms,
        warnings: output.warnings.clone(),
        artifacts: output.artifacts.iter().map(|a| a.id.clone()).collect(),
    });
    outputs.push(output);
}

fn apply_section_caps(inventory: &mut HomelabInventory) {
    cap_vec(&mut inventory.nodes, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.services, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.compose_projects, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.reverse_proxies, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.networks, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.storage, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.media_services, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.projects, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.artifact_refs, MAX_SECTION_ITEMS);
    cap_vec(&mut inventory.collection_errors, MAX_SECTION_ITEMS);
}

fn inventory_has_output(inventory: &HomelabInventory) -> bool {
    !inventory.nodes.is_empty()
        || !inventory.services.is_empty()
        || !inventory.compose_projects.is_empty()
        || !inventory.reverse_proxies.is_empty()
        || !inventory.networks.is_empty()
        || !inventory.storage.is_empty()
        || !inventory.media_services.is_empty()
        || !inventory.projects.is_empty()
}

fn run_id() -> String {
    format!("inv-{}-{}", Utc::now().timestamp(), std::process::id())
}

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
