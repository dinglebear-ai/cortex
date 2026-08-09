use super::*;
use crate::db::agent_observatory::{
    advance_projection_cursor, projection_cursor, projection_health,
};

fn pool() -> (tempfile::TempDir, Arc<DbPool>) {
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(directory.path().join("runtime.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    (directory, pool)
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
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if projection_cursor(&pool, "logs").unwrap() == "1" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for source in ["mcp", "hook", "skill", "llm"] {
            assert_eq!(projection_cursor(&pool, source).unwrap(), "");
        }
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
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
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repository)
        .status()
        .unwrap();
    std::process::Command::new("git")
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
        let handle = spawn_projector(token.clone(), Arc::clone(&pool), config).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let health = projection_health(&pool, "projector")
                    .unwrap()
                    .unwrap_or_default();
                if health.contains("\"status\":\"error\"") && health.contains("\"attempts\":2") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(projection_cursor(&pool, "logs").unwrap(), "invalid");
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let stopped = projection_health(&pool, "projector").unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(projection_health(&pool, "projector").unwrap(), stopped);
    });
}
