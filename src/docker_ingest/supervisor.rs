use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::{FutureExt, StreamExt};
use tokio::task::JoinHandle;

use crate::config::{DockerHostConfig, DockerIngestConfig};
use crate::db::DbPool;
use crate::ingest::IngestTx;
use crate::observability::RuntimeObservability;

use super::checkpoint::load_checkpoint;
use super::client::DockerHostClient;
use super::models::ContainerMeta;
use super::parser::{docker_event_to_entry, log_output_to_entry};

const MIN_STREAM_DURATION_FOR_BACKOFF_RESET: Duration = Duration::from_secs(30);
const RECONNECT_JITTER_MIN_PCT: u64 = 80;
const RECONNECT_JITTER_SPREAD_PCT: u64 = 41;

/// Concurrent checkpoint loads allowed across every container stream.
///
/// Each load takes a pooled connection directly, bypassing the service layer's
/// read semaphore, so it competes with the ingest writers for the one
/// connection reserved outside that semaphore (see
/// `app::services::read_permits_for_pool`). One task is spawned per container,
/// so a docker-daemon restart reconnects every stream at once and, uncapped,
/// issues that many simultaneous `pool.get()` calls against an 8-connection
/// pool — enough on its own to exhaust it and time out unrelated writers.
/// Deliberately conservative: loads are short, and a small cap only staggers
/// reconnects rather than slowing steady-state ingest.
pub(super) const MAX_CONCURRENT_CHECKPOINT_LOADS: usize = 2;

/// Process-wide gate for [`MAX_CONCURRENT_CHECKPOINT_LOADS`].
pub(super) fn checkpoint_load_permits() -> Arc<tokio::sync::Semaphore> {
    static PERMITS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    Arc::clone(
        PERMITS
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CHECKPOINT_LOADS))),
    )
}

pub(crate) fn spawn_all(
    config: DockerIngestConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
) -> Vec<JoinHandle<()>> {
    if !config.enabled {
        return Vec::new();
    }

    config
        .hosts
        .clone()
        .into_iter()
        .map(|host| {
            let config = config.clone();
            let pool = Arc::clone(&pool);
            let ingest = ingest.clone();
            let observability = ingest.observability();
            tokio::spawn(async move {
                run_host_forever(config, host, pool, ingest, observability).await;
            })
        })
        .collect()
}

async fn run_host_forever(
    config: DockerIngestConfig,
    host: DockerHostConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
    observability: Arc<RuntimeObservability>,
) {
    let mut delay_ms = config.reconnect_initial_ms;
    loop {
        let stream_started = Instant::now();
        let outcome = {
            let _active = ActiveDockerHostStream::new(Arc::clone(&observability));
            match run_host_once(
                &config,
                &host,
                Arc::clone(&pool),
                ingest.clone(),
                Arc::clone(&observability),
            )
            .await
            {
                Ok(()) => {
                    tracing::warn!(host = %host.name, "Docker ingest host stream ended; reconnecting");
                    StreamEnd::Clean
                }
                Err(ref e) if is_expected_disconnect(e) => {
                    tracing::debug!(
                        host = %host.name,
                        "Docker ingest host stream closed by daemon; reconnecting"
                    );
                    StreamEnd::ExpectedDisconnect
                }
                Err(e) => {
                    observability.record_docker_ingest_stream_failure();
                    tracing::warn!(
                        host = %host.name,
                        error = %e,
                        delay_ms,
                        "Docker ingest host failed; retrying"
                    );
                    StreamEnd::Failed
                }
            }
        };
        let reset_backoff = should_reset_reconnect_backoff(outcome, stream_started.elapsed());
        observability.record_docker_ingest_stream_reconnect();
        tokio::time::sleep(Duration::from_millis(jittered_reconnect_delay_ms(
            delay_ms, &host.name,
        )))
        .await;
        delay_ms = next_reconnect_backoff_ms(
            delay_ms,
            config.reconnect_initial_ms,
            config.reconnect_max_ms,
            reset_backoff,
        );
    }
}

struct ActiveDockerHostStream {
    observability: Arc<RuntimeObservability>,
}

impl ActiveDockerHostStream {
    fn new(observability: Arc<RuntimeObservability>) -> Self {
        observability.record_docker_ingest_host_stream_started();
        Self { observability }
    }
}

impl Drop for ActiveDockerHostStream {
    fn drop(&mut self) {
        self.observability.record_docker_ingest_host_stream_ended();
    }
}

struct ActiveDockerContainerStream {
    observability: Arc<RuntimeObservability>,
}

impl ActiveDockerContainerStream {
    fn new(observability: Arc<RuntimeObservability>) -> Self {
        observability.record_docker_ingest_container_stream_started();
        Self { observability }
    }
}

impl Drop for ActiveDockerContainerStream {
    fn drop(&mut self) {
        self.observability
            .record_docker_ingest_container_stream_ended();
    }
}

async fn run_host_once(
    config: &DockerIngestConfig,
    host: &DockerHostConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
    observability: Arc<RuntimeObservability>,
) -> Result<()> {
    let event_since_unix = chrono::Utc::now().timestamp().saturating_sub(60);
    let client = DockerHostClient::connect(&host.base_url)?;
    let containers = client.list_containers().await?;
    tracing::info!(
        host = %host.name,
        container_count = containers.len(),
        "Docker ingest discovered containers"
    );

    let mut log_tasks: HashMap<String, JoinHandle<()>> = HashMap::new();
    let runtime = HostRuntime {
        config,
        host,
        client: &client,
        pool,
        ingest,
        observability,
    };
    for container in containers {
        if !should_ingest_container(config, host, &container) {
            tracing::info!(
                host = %host.name,
                container = %container.name,
                "Docker ingest skipping excluded container"
            );
            continue;
        }
        spawn_log_task_if_absent(&runtime, &mut log_tasks, container);
    }

    let result = follow_container_events(&runtime, &mut log_tasks, event_since_unix).await;
    for handle in log_tasks.into_values() {
        handle.abort();
    }
    result
}

struct HostRuntime<'a> {
    config: &'a DockerIngestConfig,
    host: &'a DockerHostConfig,
    client: &'a DockerHostClient,
    pool: Arc<DbPool>,
    ingest: IngestTx,
    observability: Arc<RuntimeObservability>,
}

async fn follow_container_events(
    runtime: &HostRuntime<'_>,
    log_tasks: &mut HashMap<String, JoinHandle<()>>,
    event_since_unix: i64,
) -> Result<()> {
    let docker = runtime.client.docker();
    let mut events = docker.events(Some(DockerHostClient::container_events_options(
        event_since_unix,
    )));
    while let Some(event) = events.next().await {
        let event = event?;
        runtime.observability.record_docker_ingest_event();
        if let Some(entry) = docker_event_to_entry(&runtime.host.name, &event)?
            && runtime.ingest.send(entry).await.is_err()
        {
            anyhow::bail!("Docker ingest channel closed");
        }
        let action = event.action.unwrap_or_default();
        let Some(actor) = event.actor else {
            continue;
        };
        let Some(id) = actor.id else {
            continue;
        };

        let policy = event_task_policy(&action);
        match policy {
            DockerEventTaskPolicy::EnsureLogTask | DockerEventTaskPolicy::ReplaceLogTask => {
                prune_finished_tasks(log_tasks);
                if policy == DockerEventTaskPolicy::ReplaceLogTask
                    && let Some(handle) = log_tasks.remove(&id)
                {
                    handle.abort();
                }
                let containers = runtime.client.list_containers().await?;
                for container in containers.into_iter().filter(|c| {
                    c.id == id && should_ingest_container(runtime.config, runtime.host, c)
                }) {
                    spawn_log_task_if_absent(runtime, log_tasks, container);
                }
            }
            DockerEventTaskPolicy::StopLogTask => {
                if let Some(handle) = log_tasks.remove(&id) {
                    handle.abort();
                }
            }
            DockerEventTaskPolicy::Ignore => {}
        }
    }
    Ok(())
}

fn prune_finished_tasks(tasks: &mut HashMap<String, JoinHandle<()>>) {
    tasks.retain(|container_id, handle| {
        if !handle.is_finished() {
            return true;
        }
        if let Some(Err(ref e)) = handle.now_or_never()
            && e.is_panic()
        {
            tracing::error!(container_id = %container_id, "container log task panicked");
        }
        false
    });
}

fn is_expected_disconnect(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("error reading a body from connection")
        || msg.contains("connection reset by peer")
        || msg.contains("broken pipe")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockerEventTaskPolicy {
    EnsureLogTask,
    ReplaceLogTask,
    StopLogTask,
    Ignore,
}

fn event_task_policy(action: &str) -> DockerEventTaskPolicy {
    match action {
        "start" | "restart" => DockerEventTaskPolicy::EnsureLogTask,
        "rename" => DockerEventTaskPolicy::ReplaceLogTask,
        "die" | "destroy" | "stop" => DockerEventTaskPolicy::StopLogTask,
        _ => DockerEventTaskPolicy::Ignore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamEnd {
    Clean,
    ExpectedDisconnect,
    Failed,
}

fn should_reset_reconnect_backoff(outcome: StreamEnd, stream_duration: Duration) -> bool {
    matches!(outcome, StreamEnd::Clean | StreamEnd::ExpectedDisconnect)
        && stream_duration >= MIN_STREAM_DURATION_FOR_BACKOFF_RESET
}

fn next_reconnect_backoff_ms(current_ms: u64, initial_ms: u64, max_ms: u64, reset: bool) -> u64 {
    if reset {
        initial_ms
    } else {
        current_ms.saturating_mul(2).min(max_ms)
    }
}

fn jittered_reconnect_delay_ms(base_ms: u64, key: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let pct = RECONNECT_JITTER_MIN_PCT + (hash % RECONNECT_JITTER_SPREAD_PCT);
    ((u128::from(base_ms) * u128::from(pct)) / 100).max(1) as u64
}

fn spawn_log_task_if_absent(
    runtime: &HostRuntime<'_>,
    tasks: &mut HashMap<String, JoinHandle<()>>,
    container: ContainerMeta,
) {
    if tasks.contains_key(&container.id) {
        return;
    }
    let docker = runtime.client.docker();
    let host_name = runtime.host.name.clone();
    let reconnect_initial_ms = runtime.config.reconnect_initial_ms;
    let reconnect_max_ms = runtime.config.reconnect_max_ms;
    let pool = Arc::clone(&runtime.pool);
    let ingest = runtime.ingest.clone();
    let observability = Arc::clone(&runtime.observability);
    let container_id = container.id.clone();
    let task_container_id = container_id.clone();
    observability.record_docker_ingest_task_spawned();
    let handle = tokio::spawn(async move {
        let mut delay_ms = reconnect_initial_ms;
        loop {
            let stream_started = Instant::now();
            let outcome = {
                let _active = ActiveDockerContainerStream::new(Arc::clone(&observability));
                match follow_container_logs_once(
                    &docker,
                    &pool,
                    &ingest,
                    &observability,
                    &host_name,
                    &task_container_id,
                    &container,
                )
                .await
                {
                    Ok(()) => {
                        tracing::debug!(
                            host = %host_name,
                            container_id = %task_container_id,
                            "Docker log stream ended; reconnecting"
                        );
                        StreamEnd::Clean
                    }
                    Err(ref e) if is_expected_disconnect(e) => {
                        tracing::debug!(
                            host = %host_name,
                            container_id = %task_container_id,
                            "Docker log stream closed by daemon; reconnecting"
                        );
                        StreamEnd::ExpectedDisconnect
                    }
                    Err(e) => {
                        observability.record_docker_ingest_stream_failure();
                        tracing::warn!(
                            host = %host_name,
                            container_id = %task_container_id,
                            error = %e,
                            delay_ms,
                            "Docker log stream failed; retrying"
                        );
                        StreamEnd::Failed
                    }
                }
            };
            let reset_backoff = should_reset_reconnect_backoff(outcome, stream_started.elapsed());
            observability.record_docker_ingest_stream_reconnect();
            tokio::time::sleep(Duration::from_millis(jittered_reconnect_delay_ms(
                delay_ms,
                &task_container_id,
            )))
            .await;
            delay_ms = next_reconnect_backoff_ms(
                delay_ms,
                reconnect_initial_ms,
                reconnect_max_ms,
                reset_backoff,
            );
        }
    });
    tasks.insert(container_id, handle);
}

async fn follow_container_logs_once(
    docker: &bollard::Docker,
    pool: &Arc<DbPool>,
    ingest: &IngestTx,
    observability: &RuntimeObservability,
    host_name: &str,
    container_id: &str,
    container: &ContainerMeta,
) -> Result<()> {
    // spawn_blocking: load_checkpoint does `pool.get()` (blocks up to the 6s
    // pool timeout under contention) plus a synchronous query. Dozens of
    // container streams reconnecting after an outage previously parked that
    // wait on executor threads, stalling unrelated async work (full-review
    // PM8).
    let checkpoint = {
        let pool = Arc::clone(pool);
        let host = host_name.to_string();
        let container = container_id.to_string();
        // Bound how many streams can be loading a checkpoint at once; see
        // MAX_CONCURRENT_CHECKPOINT_LOADS. Held across the blocking call so the
        // cap covers the `pool.get()` wait, which is the contended part.
        let _permit = checkpoint_load_permits()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("checkpoint load permits closed: {e}"))?;
        tokio::task::spawn_blocking(move || load_checkpoint(&pool, &host, &container))
            .await
            .map_err(|e| anyhow::anyhow!("checkpoint load task join error: {e}"))??
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
    };
    let since_unix = docker_log_since_unix(checkpoint.as_ref(), chrono::Utc::now().timestamp());
    let mut logs = docker.logs(
        container_id,
        Some(DockerHostClient::logs_options(since_unix)),
    );

    while let Some(output) = logs.next().await {
        match log_output_to_entry(host_name, container, output?) {
            Ok(Some(entry)) => {
                observability.record_docker_ingest_log_entry();
                if checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| entry_is_at_or_before_checkpoint(&entry, checkpoint))
                {
                    continue;
                }
                if ingest.send(entry).await.is_err() {
                    anyhow::bail!("Docker ingest channel closed");
                }
            }
            Ok(None) => {}
            Err(e) => {
                observability.record_docker_ingest_parse_error();
                tracing::warn!(
                    host = %host_name,
                    container_id = %container_id,
                    error = %e,
                    "Failed to parse Docker log frame"
                );
            }
        }
    }
    Ok(())
}

fn entry_is_at_or_before_checkpoint(
    entry: &crate::db::LogBatchEntry,
    checkpoint: &chrono::DateTime<chrono::FixedOffset>,
) -> bool {
    entry
        .docker_checkpoint
        .as_ref()
        .and_then(|docker_checkpoint| {
            chrono::DateTime::parse_from_rfc3339(&docker_checkpoint.timestamp).ok()
        })
        .is_some_and(|entry_ts| entry_ts <= *checkpoint)
}

fn should_ingest_container(
    config: &DockerIngestConfig,
    host: &DockerHostConfig,
    container: &ContainerMeta,
) -> bool {
    !container_name_is_excluded(&config.excluded_containers, &container.name)
        && !container_name_is_excluded(&host.excluded_containers, &container.name)
}

fn container_name_is_excluded(excluded_containers: &[String], container_name: &str) -> bool {
    excluded_containers
        .iter()
        .any(|excluded| excluded.eq_ignore_ascii_case(container_name))
}

fn docker_log_since_unix(
    checkpoint: Option<&chrono::DateTime<chrono::FixedOffset>>,
    now_unix: i64,
) -> i64 {
    checkpoint
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|| now_unix.saturating_sub(60))
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
