use super::*;

#[test]
fn retained_environment_alone_is_not_an_installation() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".cortex")).unwrap();
    std::fs::write(home.path().join(".cortex/heartbeat-agent.env"), "retained").unwrap();
    assert!(!native_installation_exists(home.path()));
}

#[test]
fn native_artifacts_still_require_explicit_removal() {
    let home = tempfile::tempdir().unwrap();
    let unit = home
        .path()
        .join(".config/systemd/user/cortex-heartbeat-agent.service");
    std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
    std::fs::write(unit, "installed").unwrap();
    assert!(native_installation_exists(home.path()));
}

#[test]
fn compose_detection_distinguishes_dedicated_legacy_and_server_only() {
    let home = tempfile::tempdir().unwrap();
    assert!(!managed_compose_exists(home.path()).unwrap());
    let legacy = home.path().join("compose/docker-compose.yml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "services:\n  cortex:\n    image: fixture\n").unwrap();
    assert!(!managed_compose_exists(home.path()).unwrap());
    std::fs::write(
        &legacy,
        "services:\n  cortex-heartbeat-agent:\n    image: fixture\n",
    )
    .unwrap();
    assert!(managed_compose_exists(home.path()).unwrap());
    std::fs::remove_file(legacy).unwrap();
    let dedicated = home
        .path()
        .join("heartbeat-agent-compose/docker-compose.yml");
    std::fs::create_dir_all(dedicated.parent().unwrap()).unwrap();
    std::fs::write(dedicated, "services: {}\n").unwrap();
    assert!(managed_compose_exists(home.path()).unwrap());
}

#[test]
fn container_query_includes_stopped_services_and_project_directory_identity() {
    let command = docker_inspection_command();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert_eq!(args[0], "ps");
    assert!(args.contains(&"--all"));
    assert!(args.contains(&"label=com.docker.compose.service=cortex-heartbeat-agent"));
    assert!(args.contains(&"{{.Label \"com.docker.compose.project.working_dir\"}}"));
}

#[test]
fn task_query_is_scoped_to_registered_task_and_current_user_identity() {
    let command = windows_task_command();
    let script = command.get_args().last().unwrap().to_str().unwrap();
    assert!(script.contains("WindowsIdentity]::GetCurrent()"));
    assert!(script.contains("$_.TaskPath -eq '\\'"));
    assert!(script.contains("$owner -eq $identity.Name -or $owner -eq $identity.User.Value"));
    assert!(script.contains("$owner -notmatch '^S-1-'"));
    assert!(!script.contains("Unregister-ScheduledTask"));
}

#[test]
fn malformed_server_only_compose_does_not_claim_a_collector_installation() {
    let home = tempfile::tempdir().unwrap();
    let legacy = home.path().join("compose/docker-compose.yml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "services: [not valid yaml").unwrap();
    assert!(!managed_compose_exists(home.path()).unwrap());
    std::fs::write(legacy, "services: [cortex-heartbeat-agent").unwrap();
    assert!(managed_compose_exists(home.path()).is_err());
}

#[cfg(unix)]
fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[cfg(unix)]
#[tokio::test]
async fn dedicated_and_legacy_container_labels_block_even_when_stopped() {
    let home = tempfile::tempdir().unwrap();
    for directory in ["heartbeat-agent-compose", "compose"] {
        let label = home.path().join(directory);
        let command = shell("printf '%s\\n' \"$1\"");
        let mut command = command;
        command.arg("fixture").arg(&label);
        assert!(
            check_docker(home.path(), false, command, Duration::from_secs(1))
                .await
                .is_err()
        );
    }
    // The query includes --all and deliberately makes no state filter; stopped
    // collectors are installed and must also be explicitly removed.
}

#[cfg(unix)]
#[tokio::test]
async fn unrelated_agent_containers_do_not_block_current_managed_home() {
    let home = tempfile::tempdir().unwrap();
    let command = shell("printf '%s\\n' /unrelated/heartbeat-agent-compose /unrelated/compose");
    assert!(
        check_docker(home.path(), true, command, Duration::from_secs(1))
            .await
            .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stale_configuration_after_removal_is_allowed_with_successful_query() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        check_docker(home.path(), true, shell("exit 0"), Duration::from_secs(1))
            .await
            .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn configured_agent_fails_closed_when_daemon_query_fails() {
    let home = tempfile::tempdir().unwrap();
    let error = check_docker(home.path(), true, shell("exit 1"), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot inspect Docker"));
    assert!(error.to_string().contains("heartbeatagent remove"));
}

#[tokio::test]
async fn missing_docker_cli_only_blocks_a_configured_agent() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("no-docker-cli");
    assert!(
        check_docker(
            home.path(),
            false,
            Command::new(&missing),
            Duration::from_secs(1)
        )
        .await
        .is_ok()
    );
    assert!(
        check_docker(
            home.path(),
            true,
            Command::new(&missing),
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn inspection_deadline_fails_closed_for_configured_agent() {
    let home = tempfile::tempdir().unwrap();
    let start = std::time::Instant::now();
    let result = check_docker(
        home.path(),
        true,
        shell("exec sleep 30"),
        Duration::from_millis(50),
    )
    .await;
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[tokio::test]
async fn registered_current_user_task_blocks_and_absent_or_other_user_does_not() {
    assert!(
        check_windows_task(
            shell("printf '%s' '{\"owned\":true}'"),
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
    assert!(
        check_windows_task(
            shell("printf '%s' '{\"owned\":false}'"),
            Duration::from_secs(1)
        )
        .await
        .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn windows_task_inspection_errors_and_timeouts_fail_closed() {
    for script in [
        "exit 1",
        "printf '%s' malformed",
        "printf '%s' '{}'",
        "exec sleep 30",
    ] {
        assert!(
            check_windows_task(shell(script), Duration::from_millis(50))
                .await
                .is_err()
        );
    }
}
