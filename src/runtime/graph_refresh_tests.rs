use super::*;

/// Env parsing for the refresh interval. Serialized via a process-wide guard
/// because it mutates a shared environment variable.
#[test]
fn refresh_interval_secs_parses_env_with_default_fallback() {
    use std::sync::Mutex;
    static ENV_GUARD: Mutex<()> = Mutex::new(());
    let _guard = ENV_GUARD.lock().unwrap();

    let key = "CORTEX_GRAPH_REFRESH_INTERVAL_SECS";
    let previous = crate::env::var(key).ok();

    crate::env::remove_test_var(key);
    assert_eq!(refresh_interval_secs(), 0);

    crate::env::set_test_var(key, "45");
    assert_eq!(refresh_interval_secs(), 45);

    crate::env::set_test_var(key, "  90 ");
    assert_eq!(refresh_interval_secs(), 90);

    // 0 disables the scheduler.
    crate::env::set_test_var(key, "0");
    assert_eq!(refresh_interval_secs(), 0);

    // Garbage falls back to the disabled-by-default scheduler.
    crate::env::set_test_var(key, "not-a-number");
    assert_eq!(refresh_interval_secs(), 0);

    match previous {
        Some(value) => crate::env::set_test_var(key, value),
        None => crate::env::remove_test_var(key),
    }
}

/// interval=0 must yield no spawned task.
#[test]
fn spawn_returns_none_when_disabled() {
    use std::sync::Mutex;
    static ENV_GUARD: Mutex<()> = Mutex::new(());
    let _guard = ENV_GUARD.lock().unwrap();

    let key = "CORTEX_GRAPH_REFRESH_INTERVAL_SECS";
    let previous = crate::env::var(key).ok();
    crate::env::set_test_var(key, "0");

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let handle = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::StorageConfig::for_test(dir.path().join("graph-sched.db"));
        let pool = std::sync::Arc::new(crate::db::init_pool(&config).unwrap());
        spawn(
            CancellationToken::new(),
            pool,
            std::sync::Arc::new(Semaphore::new(1)),
            std::sync::Arc::new(RuntimeObservability::default()),
        )
    });
    assert!(handle.is_none());

    match previous {
        Some(value) => crate::env::set_test_var(key, value),
        None => crate::env::remove_test_var(key),
    }
}
