use super::*;
use crate::db::agent_observatory::{
    advance_projection_cursor, projection_cursor, projection_health,
};
use crate::db::{LogBatchEntry, insert_logs_batch};

fn pool() -> (tempfile::TempDir, Arc<DbPool>) {
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(directory.path().join("runtime.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    (directory, pool)
}

/// Deadline for waits on a background worker making progress.
///
/// Every `DbPool` in this test binary shares one process-wide SQLite write lock
/// (`crate::db::pool::write_lock`), and each test pool's `init_pool` runs the
/// full migration set — VACUUM, CREATE INDEX, ANALYZE — while holding it. Under
/// full-suite parallelism a worker's cursor or health write therefore queues
/// behind however many other tests are migrating, which is seconds, not
/// milliseconds. `cargo nextest` gives each test its own process and never sees
/// this contention; plain `cargo test` does, and the old 2s/5s deadlines flaked
/// there.
///
/// These are liveness backstops for a genuinely stuck worker, not latency
/// assertions — a passing run exits as soon as the condition holds, so the
/// headroom is free. Anything that needs to assert *timing* must do it against a
/// deliberately separated interval, the way the wake test uses `IDLE_POLL_MS`.
const PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

/// Deadline for a cancelled worker to unwind. The same contention applies: an
/// in-flight write may still be queued behind the shared write lock.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval high enough that the projector's fallback timer never fires
/// during a test, so it runs its first cycle and then idles. Observing progress
/// within `PROGRESS_TIMEOUT` therefore means something woke it, not that the
/// timer came round.
const IDLE_POLL_MS: u64 = 600_000;

/// Reads `attempts` out of a projector health row.
///
/// `record_projection_health` bumps this on every health write, so it only ever
/// climbs — which is the whole point. The projector's other health fields
/// (`projected`, `oversized_first_rows`) are per-cycle snapshots, and
/// `notify_projection_work` broadcasts on a **process-global** channel that every
/// `insert_logs_batch` in this test binary rings. Any test can therefore drive
/// any other test's projector through another cycle at any moment, overwriting a
/// per-cycle value microseconds after it appears. Assert on monotone facts —
/// this counter, and the durable cursors — never on a per-cycle snapshot.
fn projection_attempts(health: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(health)
        .ok()
        .and_then(|value| value.get("attempts").and_then(serde_json::Value::as_u64))
        .unwrap_or_default()
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
        let handle = spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        tokio::time::timeout(PROGRESS_TIMEOUT, async {
            loop {
                if projection_cursor(&pool, "logs").unwrap() == "1" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for source in [
            "mcp_events",
            "hook_events",
            "skill_events",
            "llm_invocations",
        ] {
            assert_eq!(projection_cursor(&pool, source).unwrap(), "");
        }
        // The cursor reaching "1" above is what proves the page-bytes guard does
        // not stall on a first row bigger than the whole budget — with
        // projector_page_bytes = 1 and a 16-byte row, a stalling guard never
        // advances it. Health is only checked for *reporting* the counter;
        // asserting the count itself would race the global wake broadcast (see
        // `projection_attempts`).
        // Wait for the health row to exist rather than reading straight after the
        // cursor wait: the cursor is written during projection, health only at
        // the end of the cycle. Existence is monotone, so this cannot flap.
        let health = tokio::time::timeout(PROGRESS_TIMEOUT, async {
            loop {
                if let Some(health) = projection_health(&pool, "projector").unwrap() {
                    return health;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a completed cycle records health");
        assert!(
            health.contains("oversized_first_rows="),
            "projector health must report the oversized-first-row counter: {health}"
        );
        token.cancel();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle)
            .await
            .unwrap()
            .unwrap();
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
        let handle = spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        tokio::time::timeout(PROGRESS_TIMEOUT, async {
            loop {
                if projection_health(&pool, "projector").unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

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

        // Still proves the wake came from the committed ingest rather than the
        // fallback poll: PROGRESS_TIMEOUT is an order of magnitude below
        // IDLE_POLL_MS.
        tokio::time::timeout(PROGRESS_TIMEOUT, async {
            loop {
                if projection_cursor(&pool, "logs").unwrap() == "1" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("committed ingest should wake projector without waiting for fallback poll");

        token.cancel();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle)
            .await
            .unwrap()
            .unwrap();
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
        tokio::time::timeout(PROGRESS_TIMEOUT, async {
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
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle)
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
        let handle = spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        tokio::time::timeout(PROGRESS_TIMEOUT, async {
            loop {
                let health = projection_health(&pool, "projector")
                    .unwrap()
                    .unwrap_or_default();
                // `>= 2`, not `== 2`: attempts only climbs, so this latches once
                // true. Pinning it to an exact value made the assertion true for
                // one ~10ms retry window and false forever after.
                if health.contains("\"status\":\"error\"") && projection_attempts(&health) >= 2 {
                    assert!(health.contains("retry_safe=false"));
                    assert!(health.contains("retry_delay_ms=0"));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(projection_cursor(&pool, "logs").unwrap(), "invalid");
        token.cancel();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle)
            .await
            .unwrap()
            .unwrap();
        let stopped = projection_health(&pool, "projector").unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(projection_health(&pool, "projector").unwrap(), stopped);
    });
}
