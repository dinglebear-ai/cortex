use std::time::Duration;
use std::{collections::HashMap, sync::Arc};

use crate::config::{DockerHostConfig, DockerIngestConfig};
use crate::db::{DockerCheckpoint, LogBatchEntry};
use crate::docker_ingest::models::ContainerMeta;
use crate::docker_ingest::supervisor::{
    MAX_CONCURRENT_CHECKPOINT_LOADS, checkpoint_load_permits, under_checkpoint_permit,
};
use crate::observability::RuntimeObservability;

use super::{
    ActiveDockerContainerStream, ActiveDockerHostStream, DockerEventTaskPolicy,
    MIN_STREAM_DURATION_FOR_BACKOFF_RESET, StreamEnd, docker_log_since_unix,
    entry_is_at_or_before_checkpoint, event_task_policy, is_expected_disconnect,
    jittered_reconnect_delay_ms, next_reconnect_backoff_ms, prune_finished_tasks,
    should_ingest_container, should_reset_reconnect_backoff,
};

fn docker_entry(timestamp: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: timestamp.into(),
        hostname: "edge-host-a".into(),
        facility: Some("local0".into()),
        severity: "info".into(),
        app_name: Some("nginx".into()),
        process_id: Some("abcdef123456".into()),
        message: "line".into(),
        raw: format!("{timestamp} line"),
        source_ip: "docker://edge-host-a/abcdef123456/stdout".into(),
        docker_checkpoint: Some(DockerCheckpoint {
            host_name: "edge-host-a".into(),
            container_id: "abcdef123456".into(),
            timestamp: timestamp.into(),
        }),
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[test]
fn checkpoint_filter_skips_only_entries_at_or_before_precise_checkpoint() {
    let checkpoint =
        chrono::DateTime::parse_from_rfc3339("2026-05-05T01:02:03.500000000Z").unwrap();

    assert!(entry_is_at_or_before_checkpoint(
        &docker_entry("2026-05-05T01:02:03.123456789Z"),
        &checkpoint
    ));
    assert!(entry_is_at_or_before_checkpoint(
        &docker_entry("2026-05-05T01:02:03.500000000Z"),
        &checkpoint
    ));
    assert!(!entry_is_at_or_before_checkpoint(
        &docker_entry("2026-05-05T01:02:03.500000001Z"),
        &checkpoint
    ));
}

#[test]
fn checkpoint_filter_keeps_entries_without_parseable_docker_checkpoint() {
    let checkpoint =
        chrono::DateTime::parse_from_rfc3339("2026-05-05T01:02:03.500000000Z").unwrap();
    let mut entry = docker_entry("2026-05-05T01:02:03.123456789Z");
    entry.docker_checkpoint = None;

    assert!(!entry_is_at_or_before_checkpoint(&entry, &checkpoint));

    let mut invalid = docker_entry("not-a-timestamp");
    invalid.docker_checkpoint.as_mut().unwrap().timestamp = "not-a-timestamp".to_string();
    assert!(!entry_is_at_or_before_checkpoint(&invalid, &checkpoint));
}

#[test]
fn docker_log_since_uses_checkpoint_when_present() {
    let checkpoint =
        chrono::DateTime::parse_from_rfc3339("2026-05-05T01:02:03.500000000Z").unwrap();

    assert_eq!(
        docker_log_since_unix(Some(&checkpoint), 1_779_690_000),
        checkpoint.timestamp()
    );
}

#[test]
fn docker_log_since_without_checkpoint_starts_near_now() {
    assert_eq!(docker_log_since_unix(None, 1_779_690_000), 1_779_689_940);
}

#[test]
fn should_ingest_container_honors_global_exclude() {
    let config = DockerIngestConfig {
        excluded_containers: vec!["arcane-mcp".into()],
        ..Default::default()
    };
    let host = DockerHostConfig {
        name: "devhost".into(),
        base_url: "http://devhost:2375".into(),
        allow_insecure_http: true,
        excluded_containers: Vec::new(),
    };
    let container = ContainerMeta {
        id: "abcdef".into(),
        name: "arcane-mcp".into(),
        image: "arcane-mcp:latest".into(),
        compose_project: None,
        compose_service: None,
    };

    assert!(!should_ingest_container(&config, &host, &container));
}

#[test]
fn should_ingest_container_honors_host_exclude_case_insensitive() {
    let config = DockerIngestConfig::default();
    let host = DockerHostConfig {
        name: "devhost".into(),
        base_url: "http://devhost:2375".into(),
        allow_insecure_http: true,
        excluded_containers: vec!["ARCANE-MCP".into()],
    };
    let container = ContainerMeta {
        id: "abcdef".into(),
        name: "arcane-mcp".into(),
        image: "arcane-mcp:latest".into(),
        compose_project: None,
        compose_service: None,
    };

    assert!(!should_ingest_container(&config, &host, &container));
}

#[test]
fn docker_event_policy_maps_lifecycle_actions_to_task_work() {
    assert_eq!(
        event_task_policy("start"),
        DockerEventTaskPolicy::EnsureLogTask
    );
    assert_eq!(
        event_task_policy("restart"),
        DockerEventTaskPolicy::EnsureLogTask
    );
    assert_eq!(
        event_task_policy("rename"),
        DockerEventTaskPolicy::ReplaceLogTask
    );
    assert_eq!(event_task_policy("die"), DockerEventTaskPolicy::StopLogTask);
    assert_eq!(
        event_task_policy("stop"),
        DockerEventTaskPolicy::StopLogTask
    );
    assert_eq!(
        event_task_policy("destroy"),
        DockerEventTaskPolicy::StopLogTask
    );
    assert_eq!(
        event_task_policy("exec_start"),
        DockerEventTaskPolicy::Ignore
    );
}

#[test]
fn expected_disconnect_detection_matches_known_transport_errors_only() {
    for message in [
        "error reading a body from connection",
        "connection reset by peer",
        "broken pipe",
    ] {
        let error = anyhow::anyhow!(message);
        assert!(is_expected_disconnect(&error), "{message}");
    }

    let error = anyhow::anyhow!("permission denied opening docker socket");
    assert!(!is_expected_disconnect(&error));
}

#[test]
fn reconnect_backoff_resets_only_after_durable_clean_streams() {
    assert!(!should_reset_reconnect_backoff(
        StreamEnd::Clean,
        MIN_STREAM_DURATION_FOR_BACKOFF_RESET - Duration::from_millis(1)
    ));
    assert!(should_reset_reconnect_backoff(
        StreamEnd::Clean,
        MIN_STREAM_DURATION_FOR_BACKOFF_RESET
    ));
    assert!(should_reset_reconnect_backoff(
        StreamEnd::ExpectedDisconnect,
        MIN_STREAM_DURATION_FOR_BACKOFF_RESET + Duration::from_secs(1)
    ));
    assert!(!should_reset_reconnect_backoff(
        StreamEnd::Failed,
        MIN_STREAM_DURATION_FOR_BACKOFF_RESET + Duration::from_secs(1)
    ));
}

#[test]
fn reconnect_backoff_doubles_to_cap_unless_reset() {
    assert_eq!(
        next_reconnect_backoff_ms(1_000, 1_000, 30_000, false),
        2_000
    );
    assert_eq!(
        next_reconnect_backoff_ms(20_000, 1_000, 30_000, false),
        30_000
    );
    assert_eq!(
        next_reconnect_backoff_ms(u64::MAX, 1_000, 30_000, false),
        30_000
    );
    assert_eq!(next_reconnect_backoff_ms(8_000, 1_000, 30_000, true), 1_000);
}

#[test]
fn reconnect_delay_jitter_is_deterministic_and_bounded() {
    let first = jittered_reconnect_delay_ms(10_000, "edge-host-a");
    let second = jittered_reconnect_delay_ms(10_000, "edge-host-a");
    let other = jittered_reconnect_delay_ms(10_000, "edge-host-b");

    assert_eq!(first, second);
    assert_ne!(first, other);
    assert!((8_000..=12_000).contains(&first));
    assert!((8_000..=12_000).contains(&other));
}

#[test]
fn active_stream_guards_increment_and_decrement_observability_counters() {
    let observability = Arc::new(RuntimeObservability::default());

    {
        let _host = ActiveDockerHostStream::new(Arc::clone(&observability));
        let _container = ActiveDockerContainerStream::new(Arc::clone(&observability));
        let snapshot = observability.snapshot();
        assert_eq!(snapshot.docker_ingest_host_streams_active, 1);
        assert_eq!(snapshot.docker_ingest_container_streams_active, 1);
    }

    let snapshot = observability.snapshot();
    assert_eq!(snapshot.docker_ingest_host_streams_active, 0);
    assert_eq!(snapshot.docker_ingest_container_streams_active, 0);
}

#[tokio::test]
async fn prune_finished_tasks_removes_completed_tasks_and_keeps_running_tasks() {
    let mut tasks = HashMap::from([
        ("done".to_string(), tokio::spawn(async {})),
        (
            "running".to_string(),
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }),
        ),
    ]);

    tokio::task::yield_now().await;
    prune_finished_tasks(&mut tasks);

    assert!(!tasks.contains_key("done"));
    let running = tasks.remove("running").expect("running task kept");
    running.abort();
}

#[tokio::test]
async fn prune_finished_tasks_removes_panicked_tasks_without_panicking() {
    let mut tasks = HashMap::from([(
        "panic".to_string(),
        tokio::spawn(async { panic!("simulated docker log task panic") }),
    )]);

    tokio::task::yield_now().await;
    prune_finished_tasks(&mut tasks);

    assert!(tasks.is_empty());
}

/// The permits are a process-wide singleton, so two tests manipulating them
/// concurrently would see each other's holds. `cargo test` runs the suite in
/// one process (unlike `cargo nextest`), so they must be serialized. Async
/// mutex because the guard is held across the awaits these tests are made of.
static PERMIT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The checkpoint-load gate is a shared, bounded singleton: every container
/// stream draws from the same permits, so a reconnect storm cannot issue more
/// than `MAX_CONCURRENT_CHECKPOINT_LOADS` simultaneous `pool.get()` calls.
///
/// Scope: this pins the cap and the sharing — that the semaphore is wired up
/// as one shared gate with the intended size. It deliberately says nothing
/// about how long a permit is held; see
/// `checkpoint_permit_is_held_for_the_whole_load` for that, which is the
/// property that actually bounds concurrency.
#[tokio::test]
async fn checkpoint_load_permits_are_shared_and_bounded() {
    let _serialize = PERMIT_TEST_LOCK.lock().await;
    let permits = checkpoint_load_permits();
    assert!(
        std::sync::Arc::ptr_eq(&permits, &checkpoint_load_permits()),
        "all streams must share one gate, not get a fresh semaphore each call"
    );

    let mut held = Vec::new();
    for _ in 0..MAX_CONCURRENT_CHECKPOINT_LOADS {
        held.push(
            checkpoint_load_permits()
                .acquire_owned()
                .await
                .expect("permit within the cap"),
        );
    }

    assert!(
        checkpoint_load_permits().try_acquire().is_err(),
        "a load beyond the cap must wait rather than pile onto the pool"
    );

    drop(held);
    assert_eq!(
        checkpoint_load_permits().available_permits(),
        MAX_CONCURRENT_CHECKPOINT_LOADS,
        "permits must be returned when loads finish"
    );
}

/// The cap only bounds anything if the permit is held for the **duration** of
/// the load. Releasing it as soon as the load starts would let every
/// reconnecting stream pile onto `pool.get()` simultaneously while the
/// semaphore reported itself satisfied — the exact exhaustion the cap exists
/// to prevent, with a green test suite over it.
///
/// So: occupy every permit but one, start a load that parks mid-flight, and
/// assert the gate is empty *while the load is still running*. Nothing here
/// sleeps or races a clock; the load parks on a channel and is released
/// explicitly.
#[tokio::test]
async fn checkpoint_permit_is_held_for_the_whole_load() {
    let _serialize = PERMIT_TEST_LOCK.lock().await;
    let gate = checkpoint_load_permits();
    assert_eq!(
        gate.available_permits(),
        MAX_CONCURRENT_CHECKPOINT_LOADS,
        "gate must start idle"
    );

    // Leave exactly one permit for the load under test.
    let mut held = Vec::new();
    for _ in 0..MAX_CONCURRENT_CHECKPOINT_LOADS - 1 {
        held.push(gate.clone().acquire_owned().await.unwrap());
    }

    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let load = tokio::spawn(under_checkpoint_permit(async move {
        started_tx.send(()).expect("test still listening");
        finish_rx.await.expect("release signal");
    }));

    started_rx.await.expect("load entered the guarded section");
    assert_eq!(
        gate.available_permits(),
        0,
        "the permit must stay held while the load runs, not be released at its start"
    );
    assert!(
        gate.try_acquire().is_err(),
        "a further load must wait rather than pile onto the pool"
    );

    finish_tx.send(()).expect("load still running");
    load.await.expect("load task joins").expect("load succeeds");

    assert_eq!(
        gate.available_permits(),
        1,
        "the permit must be released once the load completes"
    );
    drop(held);
    assert_eq!(gate.available_permits(), MAX_CONCURRENT_CHECKPOINT_LOADS);
}

/// A load that fails still returns its permit — otherwise a run of failing
/// reconnects would drain the gate permanently and wedge checkpoint loading
/// for the life of the process.
#[tokio::test]
async fn checkpoint_permit_is_released_when_the_load_fails() {
    let _serialize = PERMIT_TEST_LOCK.lock().await;
    let gate = checkpoint_load_permits();

    let result: anyhow::Result<anyhow::Result<()>> =
        under_checkpoint_permit(async { Err(anyhow::anyhow!("checkpoint load failed")) }).await;
    assert!(result.expect("gate acquired").is_err());

    assert_eq!(
        gate.available_permits(),
        MAX_CONCURRENT_CHECKPOINT_LOADS,
        "a failed load must not leak its permit"
    );
}
