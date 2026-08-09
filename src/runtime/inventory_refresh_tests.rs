use super::*;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn inventory_refresh_interval_parser_accepts_zero_disable() {
    assert_eq!(parse_inventory_refresh_interval_secs("0"), Some(0));
    assert_eq!(parse_inventory_refresh_interval_secs("300"), Some(300));
    assert_eq!(parse_inventory_refresh_interval_secs(" nope "), None);
}

#[test]
fn remote_docker_events_default_to_disabled() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS");
    }
    assert!(!remote_docker_events_enabled());
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS", "true");
    }
    assert!(remote_docker_events_enabled());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS");
    }
}

#[test]
fn inventory_graph_projection_defaults_to_disabled() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED");
    }
    assert!(!inventory_graph_projection_enabled());
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED", "true");
    }
    assert!(inventory_graph_projection_enabled());
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED", "1");
    }
    assert!(inventory_graph_projection_enabled());
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED", "false");
    }
    assert!(!inventory_graph_projection_enabled());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_GRAPH_PROJECTION_ENABLED");
    }
}

#[test]
fn inventory_watch_env_accepts_common_false_values() {
    let _guard = env_lock().lock().unwrap();
    for value in ["0", "false", "FALSE", "no", " No "] {
        unsafe {
            std::env::set_var("CORTEX_INVENTORY_WATCH_ENABLED", value);
        }
        assert!(!inventory_watch_enabled(), "{value:?} should disable watch");
    }
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_WATCH_ENABLED", "yes");
    }
    assert!(inventory_watch_enabled());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_WATCH_ENABLED");
    }
}

#[test]
fn inventory_refresh_interval_env_falls_back_on_invalid_values() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_REFRESH_INTERVAL_SECS", "not-a-number");
    }
    assert_eq!(
        inventory_refresh_interval_secs(),
        INVENTORY_REFRESH_INTERVAL_SECS
    );
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_REFRESH_INTERVAL_SECS", "42");
    }
    assert_eq!(inventory_refresh_interval_secs(), 42);
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_REFRESH_INTERVAL_SECS");
    }
}

#[test]
fn start_config_watcher_returns_none_when_watch_disabled() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_WATCH_ENABLED", "false");
    }
    let mut config = crate::inventory::InventoryConfig::from_env();
    config.compose_paths = vec!["/tmp/cortex-compose.yml".into()];

    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let watcher = start_config_watcher(&config, tx);

    assert!(watcher.is_none());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_WATCH_ENABLED");
    }
}

#[test]
fn start_config_watcher_returns_none_when_no_targets_are_configured() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_WATCH_ENABLED", "true");
    }
    let mut config = crate::inventory::InventoryConfig::from_env();
    config.compose_paths.clear();
    config.proxy_paths.clear();
    config.adguard_paths.clear();

    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let watcher = start_config_watcher(&config, tx);

    assert!(watcher.is_none());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_WATCH_ENABLED");
    }
}

#[test]
fn watched_config_targets_include_compose_proxy_and_adguard_paths_once() {
    let mut config = crate::inventory::InventoryConfig::from_env();
    config.compose_paths = vec![
        "/opt/edge/compose.yaml".into(),
        "/opt/edge/compose.yaml".into(),
    ];
    config.proxy_paths = vec!["/opt/swag/nginx/site.conf".into()];
    config.adguard_paths = vec!["/opt/adguard/AdGuardHome.yaml".into()];

    let targets = watched_config_targets(&config);

    assert_eq!(targets.len(), 3);
    assert!(targets.contains(&"/opt/edge/compose.yaml".into()));
    assert!(targets.contains(&"/opt/swag/nginx/site.conf".into()));
    assert!(targets.contains(&"/opt/adguard/AdGuardHome.yaml".into()));
}

#[test]
fn watch_directories_watch_parent_for_files() {
    let targets = vec![
        "/opt/edge/compose.yaml".into(),
        "/opt/swag/nginx/site.conf".into(),
    ];

    let dirs = watch_directories(&targets);

    assert_eq!(
        dirs,
        vec![
            std::path::PathBuf::from("/opt/edge"),
            std::path::PathBuf::from("/opt/swag/nginx")
        ]
    );
}

#[test]
fn watch_directories_dedupes_parent_and_directory_targets() {
    let dir = tempfile::tempdir().unwrap();
    let stack_dir = dir.path().join("stack");
    std::fs::create_dir(&stack_dir).unwrap();
    let targets = vec![
        stack_dir.clone(),
        stack_dir.join("compose.yaml"),
        stack_dir.join("override.yaml"),
    ];

    let dirs = watch_directories(&targets);

    assert_eq!(dirs, vec![stack_dir]);
}

#[test]
fn should_refresh_for_relevant_config_events_only() {
    let targets = vec!["/opt/edge/compose.yaml".into()];
    let changed = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Content,
    )))
    .add_path("/opt/edge/compose.yaml".into());
    let access = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Open(
        notify::event::AccessMode::Read,
    )))
    .add_path("/opt/edge/compose.yaml".into());
    let unrelated = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Content,
    )))
    .add_path("/opt/edge/notes.txt".into());

    assert!(should_refresh_for_event(&Ok(changed), &targets));
    assert!(!should_refresh_for_event(&Ok(access), &targets));
    assert!(!should_refresh_for_event(&Ok(unrelated), &targets));
}

#[test]
fn should_refresh_for_event_handles_errors_and_create_remove_directory_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let target_dir = tmp.path().join("stack");
    std::fs::create_dir(&target_dir).unwrap();
    let targets = vec![target_dir.clone()];
    let created = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(target_dir.join("docker-compose.yml"));
    let removed = notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
        .add_path(target_dir.join("docker-compose.yml"));
    let any =
        notify::Event::new(notify::EventKind::Any).add_path(target_dir.join("docker-compose.yml"));

    assert!(!should_refresh_for_event(
        &Err(notify::Error::generic("watch failed")),
        &targets
    ));
    assert!(should_refresh_for_event(&Ok(created), &targets));
    assert!(should_refresh_for_event(&Ok(removed), &targets));
    assert!(should_refresh_for_event(&Ok(any), &targets));
}

#[test]
fn path_matches_target_accepts_directory_targets_but_not_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let target_dir = tmp.path().join("stack");
    std::fs::create_dir(&target_dir).unwrap();
    let sibling_dir = tmp.path().join("stack-old");
    std::fs::create_dir(&sibling_dir).unwrap();

    assert!(path_matches_target(
        &target_dir.join("docker-compose.yml"),
        &target_dir
    ));
    assert!(path_matches_target(&target_dir, &target_dir));
    assert!(!path_matches_target(
        &sibling_dir.join("docker-compose.yml"),
        &target_dir
    ));
}

#[test]
fn remote_docker_events_ssh_args_include_safe_options_and_remote_command() {
    let context = crate::inventory::ssh::SshContext::new(
        crate::inventory::ssh::SshOptions::for_config(Some(std::path::Path::new(
            "/tmp/ssh_config",
        )))
        .with_event_stream_defaults(),
    );
    let args = remote_docker_events_ssh_args(&context, "edgehost").unwrap();

    assert_eq!(args[0], "-o");
    assert_eq!(args[1], "IgnoreUnknown=WarnWeakCrypto");
    assert_eq!(args[2], "-F");
    assert!(args.contains(&"/tmp/ssh_config".to_string()));
    assert!(args.contains(&"BatchMode=yes".to_string()));
    assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
    assert!(args.contains(&"ServerAliveInterval=15".to_string()));
    assert!(args.contains(&"--".to_string()));
    assert_eq!(args[args.len() - 2], "edgehost");
    let command = &args[args.len() - 1];
    assert!(command.contains("command -v docker"));
    assert!(command.contains(REMOTE_DOCKER_EVENTS_UNSUPPORTED_MARKER));
    assert!(command.contains("exec docker events --filter type=container"));
    assert!(command.contains("--format '{{json .}}'"));
}

#[test]
fn remote_docker_events_unsupported_detects_preflight_marker() {
    assert!(remote_docker_events_unsupported(
        "ssh docker events exited with status=exit status: 78; stderr_sample=cortex: remote Docker events unsupported: docker command not found"
    ));
    assert!(!remote_docker_events_unsupported(
        "ssh docker events exited with status=exit status: 255; stderr_sample=connection timed out"
    ));
}

#[test]
fn remote_docker_events_output_sample_is_bounded() {
    let mut sample = OutputSample::default();
    sample.push_line(&"x".repeat(5000));

    let rendered = sample.as_str();
    assert!(rendered.ends_with("...<truncated>"));
    assert!(rendered.len() <= 4110);
}

#[test]
fn output_sample_preserves_line_boundaries_and_empty_state() {
    let mut sample = OutputSample::default();
    assert_eq!(sample.as_str(), "");

    sample.push_line("first");
    sample.push_line("second");

    assert_eq!(sample.as_str(), "first\nsecond");
}

#[test]
fn output_sample_marks_truncated_after_capacity_is_reached() {
    let mut sample = OutputSample::default();
    sample.push_line(&"x".repeat(4096));
    sample.push_line("extra");

    let rendered = sample.as_str();

    assert!(rendered.ends_with("...<truncated>"));
    assert!(rendered.starts_with(&"x".repeat(4096)));
}

#[test]
fn output_sample_truncates_on_utf8_boundary() {
    let mut sample = OutputSample::default();
    sample.push_line(&"é".repeat(3000));

    let rendered = sample.as_str();
    assert!(rendered.ends_with("...<truncated>"));
    let sample_body = rendered.trim_end_matches("...<truncated>");
    assert!(sample_body.is_char_boundary(sample_body.len()));
}

#[tokio::test]
async fn read_stream_sample_truncates_large_stderr_payloads() {
    let payload = vec![b'x'; 5000];
    let rendered = read_stream_sample(std::io::Cursor::new(payload)).await;

    assert!(rendered.ends_with("...<truncated>"));
    assert!(rendered.len() <= 4110);
}

#[tokio::test]
async fn read_stream_sample_preserves_lossy_utf8_without_truncation_marker() {
    let rendered = read_stream_sample(std::io::Cursor::new(vec![0xff, b'a'])).await;

    assert_eq!(rendered, "\u{fffd}a");
}

#[tokio::test]
async fn debounce_watch_events_drains_queued_events_after_delay() {
    tokio::time::pause();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    tx.send(()).await.unwrap();
    tx.send(()).await.unwrap();
    {
        let debounce = debounce_watch_events(&mut rx);
        tokio::pin!(debounce);

        tokio::select! {
            _ = &mut debounce => panic!("debounce should wait before draining"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
        }
        tokio::time::advance(std::time::Duration::from_secs(
            INVENTORY_WATCH_DEBOUNCE_SECS,
        ))
        .await;
        debounce.await;
    }

    assert!(rx.try_recv().is_err());
}

#[test]
fn remote_docker_event_tasks_return_empty_when_disabled_even_with_hosts() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS", "false");
    }
    let mut config = crate::inventory::InventoryConfig::from_env();
    config.ssh_hosts = vec!["nashost".into()];
    let (tx, _rx) = tokio::sync::mpsc::channel(1);

    let tasks = spawn_remote_docker_event_tasks(
        &config,
        tx,
        tokio_util::sync::CancellationToken::new(),
        Arc::new(RuntimeObservability::default()),
    );

    assert!(tasks.is_empty());
    unsafe {
        std::env::remove_var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS");
    }
}

#[test]
fn event_stream_failure_log_counts_saturating_failures() {
    let mut log = EventStreamFailureLog { failures: u64::MAX };

    log.record("nashost", "ssh failed");

    assert_eq!(log.failures, u64::MAX);
}

#[tokio::test]
async fn remote_docker_event_stream_cancels_while_waiting_for_ssh_limiter() {
    let context = crate::inventory::ssh::SshContext::new(
        crate::inventory::ssh::SshOptions::default()
            .with_max_concurrent(1)
            .unwrap(),
    );
    let _held = context.acquire_owned().await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let token = tokio_util::sync::CancellationToken::new();
    let child = token.child_token();

    let task = tokio::spawn({
        let context = context.clone();
        async move { run_remote_docker_events_once("nashost", &context, tx, child).await }
    });
    token.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_millis(100), task)
        .await
        .expect("event stream task should return promptly after cancellation")
        .expect("join");
    assert!(result.is_ok());
}
