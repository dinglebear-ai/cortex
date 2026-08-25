use super::*;
use crate::db::agent_observatory::{
    advance_projection_cursor, projection_cursor, projection_health,
};
use crate::db::{LogBatchEntry, insert_logs_batch};
use tokio::sync::watch;

fn pool() -> (tempfile::TempDir, Arc<DbPool>) {
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(directory.path().join("runtime.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    (directory, pool)
}

/// Block until the projector's own progress signal satisfies `ready`.
///
/// A projector cycle is not wall-clock bounded — it takes the process-wide
/// SQLite write lock several times, and under full-suite parallelism that lock
/// can hold one cycle for tens of seconds — so these tests wait on a real
/// signal instead of polling against a deadline. Predicates must be monotonic
/// over [`ProjectorProgress`]'s cumulative counters: the projector only yields
/// when all of its select branches are pending, so it can run several cycles
/// between two polls of the waiter and a "counter equals N exactly" predicate
/// would be missable. `Err` means the task ended first, which is a real failure
/// rather than a slow machine.
async fn await_progress(
    progress: &mut watch::Receiver<ProjectorProgress>,
    expectation: &str,
    ready: impl Fn(ProjectorProgress) -> bool,
) -> ProjectorProgress {
    loop {
        let seen = *progress.borrow_and_update();
        if ready(seen) {
            return seen;
        }
        progress
            .changed()
            .await
            .unwrap_or_else(|_| panic!("projector exited before {expectation}"));
    }
}

/// Fallback poll interval high enough that the projector's own timer never
/// fires during a test, so it runs its first cycle and then idles. Progress
/// observed after that therefore means something *woke* it rather than that the
/// timer came round. Tests that need repeated cycles set a small interval
/// instead.
const IDLE_POLL_MS: u64 = 600_000;

fn health_attempts(health: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(health)
        .unwrap_or_else(|error| panic!("health is not JSON: {error}: {health}"))["attempts"]
        .as_u64()
        .unwrap_or_else(|| panic!("health has no numeric attempts: {health}"))
}

#[test]
fn projector_sqlite_retry_backoff_reuses_ingest_policy_and_caps() {
    assert_eq!(projector_retry_delay_ms(1), 25);
    assert_eq!(projector_retry_delay_ms(2), 100);
    assert_eq!(projector_retry_delay_ms(3), 250);
    assert_eq!(projector_retry_delay_ms(4), 250);
    assert_eq!(projector_retry_delay_ms(100), 250);
}

#[test]
fn workers_stay_dormant_when_agent_observatory_is_disabled() {
    let (_directory, pool) = pool();
    let config = AgentObservatoryConfig::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert!(
            spawn_projector(CancellationToken::new(), Arc::clone(&pool), config.clone()).is_none()
        );
        assert!(spawn_git_reconcile(CancellationToken::new(), pool, config).is_none());
    });
}

#[test]
fn enabled_projector_advances_durable_log_cursor_and_shuts_down() {
    let (_directory, pool) = pool();
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO logs
            (timestamp, hostname, facility, severity, app_name, message, raw, received_at, source_ip)
         VALUES (?1, 'test-host', 1, 6, 'unrelated', 'bounded test row', 'bounded test row', ?1, '127.0.0.1')",
            ["2026-08-09T12:00:00.000Z"],
        )
        .unwrap();
    let config = AgentObservatoryConfig {
        enabled: true,
        projector_poll_ms: IDLE_POLL_MS,
        projector_page_bytes: 1,
        ..AgentObservatoryConfig::default()
    };
    let token = CancellationToken::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handle, mut progress) =
            spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        let seen = await_progress(&mut progress, "the log row was projected", |p| {
            p.projected >= 1
        })
        .await;
        // The single row is over `projector_page_bytes` but is the page's first
        // row, so the guard counts it and projects it anyway. Both counters are
        // cumulative, and no later cycle has a row to project, so these hold no
        // matter how many cycles ran before this task was polled.
        assert_eq!(seen.projected, 1);
        assert_eq!(seen.oversized_first_rows, 1);
        assert_eq!(projection_cursor(&pool, "logs").unwrap(), "1");
        for source in [
            "mcp_events",
            "hook_events",
            "skill_events",
            "llm_invocations",
        ] {
            assert_eq!(projection_cursor(&pool, source).unwrap(), "");
        }
        let health = projection_health(&pool, "projector").unwrap().unwrap();
        assert!(health.contains("\"status\":\"ok\""), "{health}");
        token.cancel();
        handle.await.unwrap();
    });
}

#[test]
fn projector_wakes_on_committed_log_ingest_before_fallback_poll() {
    let (_directory, pool) = pool();
    let config = AgentObservatoryConfig {
        enabled: true,
        projector_poll_ms: IDLE_POLL_MS,
        ..AgentObservatoryConfig::default()
    };
    let token = CancellationToken::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handle, mut progress) =
            spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        await_progress(&mut progress, "the first cycle completed", |p| {
            p.cycles >= 1
        })
        .await;

        insert_logs_batch(
            &pool,
            &[LogBatchEntry {
                timestamp: "2026-08-09T12:00:00.000Z".to_string(),
                hostname: "test-host".to_string(),
                facility: None,
                severity: "info".to_string(),
                app_name: Some("wake-test".to_string()),
                process_id: None,
                message: "wake test row".to_string(),
                raw: "wake test row".to_string(),
                source_ip: "test://wake".to_string(),
                docker_checkpoint: None,
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
            }],
        )
        .unwrap();

        // This deadline *is* the assertion: it has to discriminate "woken by the
        // commit notification" from "woken by the fallback poll", so a
        // wall-clock bound is the correct mechanism here. It is sized for the
        // gap between those two outcomes — 20s against a 600s fallback — not for
        // how fast a cycle usually is.
        tokio::time::timeout(
            Duration::from_secs(20),
            await_progress(&mut progress, "the committed row was projected", |p| {
                p.projected >= 1
            }),
        )
        .await
        .expect("committed ingest should wake projector without waiting for fallback poll");
        assert_eq!(projection_cursor(&pool, "logs").unwrap(), "1");

        token.cancel();
        handle.await.unwrap();
    });
}

#[test]
fn enabled_git_worker_records_progress_and_shuts_down() {
    let (directory, pool) = pool();
    let repository = directory.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    crate::env::command("git")
        .args(["init", "-q"])
        .current_dir(&repository)
        .status()
        .unwrap();
    crate::env::command("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ])
        .current_dir(&repository)
        .status()
        .unwrap();
    let config = AgentObservatoryConfig {
        enabled: true,
        git: crate::config::AgentObservatoryGitConfig {
            roots: vec![repository.display().to_string()],
            reconcile_interval_secs: 60,
            ..crate::config::AgentObservatoryGitConfig::default()
        },
        ..AgentObservatoryConfig::default()
    };
    let token = CancellationToken::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let handle = spawn_git_reconcile(token.clone(), Arc::clone(&pool), config).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !projection_cursor(&pool, "git").unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn projector_failure_is_queryable_retries_without_advancing_and_stops_after_cancel() {
    let (_directory, pool) = pool();
    projection_cursor(&pool, "logs").unwrap();
    advance_projection_cursor(&pool, "logs", "invalid").unwrap();
    let config = AgentObservatoryConfig {
        enabled: true,
        projector_poll_ms: 10,
        ..AgentObservatoryConfig::default()
    };
    let token = CancellationToken::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handle, mut progress) =
            spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        // Two persisted health rows are two completed cycles, i.e. the failure
        // was retried. Awaiting the projector's signal keeps that independent of
        // how long a cycle takes under suite load.
        await_progress(&mut progress, "two failing cycles were recorded", |p| {
            p.health_records >= 2
        })
        .await;

        let health = projection_health(&pool, "projector").unwrap().unwrap();
        assert!(health.contains("\"status\":\"error\""), "{health}");
        assert!(health.contains("retry_safe=false"), "{health}");
        assert!(health.contains("retry_delay_ms=0"), "{health}");
        assert!(health_attempts(&health) >= 2, "{health}");
        assert_eq!(projection_cursor(&pool, "logs").unwrap(), "invalid");

        token.cancel();
        handle.await.unwrap();
        // The task has been joined, so its progress sender is dropped. A closed
        // channel with no unseen update proves no further cycle ran after
        // cancellation — the signal replaces a "sleep and re-read" check.
        let after_cancel = *progress.borrow_and_update();
        assert!(
            progress.changed().await.is_err(),
            "projector must not run another cycle after cancellation"
        );
        assert_eq!(*progress.borrow(), after_cancel);
        assert_eq!(
            health_attempts(&projection_health(&pool, "projector").unwrap().unwrap()),
            after_cancel.health_records
        );
    });
}
