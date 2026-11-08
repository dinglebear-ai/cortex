use super::*;
use serial_test::serial;

struct EnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = crate::env::var_os(name);
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }

    fn remove(name: &'static str) -> Self {
        let previous = crate::env::var_os(name);
        crate::env::remove_test_var(name);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                crate::env::set_test_var(self.name, value);
            }
            None => {
                crate::env::remove_test_var(self.name);
            }
        }
    }
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn launchd_service_observation(home: &Path) -> SetupPhase {
    check_launchd(
        home,
        Path::new("/cortex"),
        Path::new("/env"),
        Path::new("/host-id"),
    )
    .into_iter()
    .find(|phase| phase.name == "heartbeat-agent-service")
    .unwrap()
}

#[test]
#[cfg(unix)]
#[serial]
fn launchd_check_accepts_output_larger_than_pipe_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("launchctl");
    write_executable(
        &program,
        "#!/bin/sh\ncase \"$2\" in\n*transcript-forwarder) exit 113;;\nesac\ndd if=/dev/zero bs=131072 count=1 2>/dev/null\nprintf 'state = running\\npid = 123\\n'\n",
    );
    let _program = EnvGuard::set("CORTEX_TEST_LAUNCHCTL", &program);
    // This checks pipe drainage; timeout behavior has a separate short-budget test.
    let _deadline = EnvGuard::set("CORTEX_TEST_COMMAND_DEADLINE_MS", "2000");
    let _home = EnvGuard::set("CORTEX_HOME", dir.path());
    let phase = launchd_service_observation(dir.path());
    assert!(matches!(phase.status, SetupStatus::Ok), "{}", phase.detail);
    assert_eq!(phase.detail, "canonical loaded");
}

#[test]
#[cfg(unix)]
#[serial]
fn launchd_check_reports_timeout_as_unknown_service_state() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("launchctl");
    write_executable(
        &program,
        "#!/bin/sh\ncase \"$2\" in\n*transcript-forwarder) exit 113;;\nesac\nexec sleep 30\n",
    );
    let _program = EnvGuard::set("CORTEX_TEST_LAUNCHCTL", &program);
    let _deadline = EnvGuard::set("CORTEX_TEST_COMMAND_DEADLINE_MS", "50");
    let _home = EnvGuard::set("CORTEX_HOME", dir.path());
    let phase = launchd_service_observation(dir.path());
    assert!(matches!(phase.status, SetupStatus::Error));
    assert!(
        phase
            .detail
            .contains("canonical launchd inspection failed; service state unknown")
    );
    assert!(phase.detail.contains("timed out"));
    assert!(!phase.detail.contains("no heartbeat agent loaded"));
}

#[test]
#[cfg(unix)]
#[serial]
fn launchd_check_distinguishes_missing_jobs_from_inspection_failures() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("launchctl");
    let _program = EnvGuard::set("CORTEX_TEST_LAUNCHCTL", &program);
    let _deadline = EnvGuard::set("CORTEX_TEST_COMMAND_DEADLINE_MS", "5000");
    let _home = EnvGuard::set("CORTEX_HOME", dir.path());
    write_executable(&program, "#!/bin/sh\nexit 113\n");
    let phase = launchd_service_observation(dir.path());
    assert!(
        matches!(phase.status, SetupStatus::Warn),
        "{}",
        phase.detail
    );
    assert_eq!(phase.detail, "no heartbeat agent loaded");
    for failing_label in [
        super::super::launchd::LABEL,
        super::super::launchd::LEGACY_LABEL,
    ] {
        write_executable(
            &program,
            &format!(
                "#!/bin/sh\ncase \"$2\" in\n*/{failing_label}) printf 'private-token' >&2; exit 1;;\nesac\nexit 113\n"
            ),
        );
        let phase = launchd_service_observation(dir.path());
        assert!(
            matches!(phase.status, SetupStatus::Error),
            "{}",
            phase.detail
        );
        assert!(phase.detail.contains("service state unknown"));
        assert!(!phase.detail.contains("private-token"));
    }
}

#[test]
#[cfg(unix)]
#[serial]
fn launchd_rollback_recovery_requires_confirmed_legacy_absence() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("launchctl");
    write_executable(
        &program,
        "#!/bin/sh\ncase \"$2\" in\n*transcript-forwarder) exit 1;;\nesac\nprintf 'state = running\\n'\n",
    );
    std::fs::write(
        dir.path().join("heartbeat-agent-migration.json"),
        "{\"step\":\"rollback\"}",
    )
    .unwrap();
    let _program = EnvGuard::set("CORTEX_TEST_LAUNCHCTL", &program);
    let _deadline = EnvGuard::set("CORTEX_TEST_COMMAND_DEADLINE_MS", "1000");
    let _home = EnvGuard::set("CORTEX_HOME", dir.path());
    let phase = check_lifecycle_state(dir.path(), ServiceBackend::Launchd);
    assert!(
        matches!(phase.status, SetupStatus::Warn),
        "{}",
        phase.detail
    );
    assert!(phase.detail.contains("migration=rollback"));
    assert!(!phase.detail.contains("recovered-canonical"));
}

#[cfg(all(unix, not(target_os = "macos")))]
fn path_with_prepended(dir: &std::path::Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = crate::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

#[test]
fn unit_runs_heartbeat_agent_with_private_host_id_path() {
    let unit = heartbeat_agent_unit(
        Path::new("/usr/local/bin/cortex"),
        Path::new("/home/me/.cortex/heartbeat-agent.env"),
        Path::new("/home/me/.cortex/heartbeat-host-id"),
    )
    .unwrap();
    assert!(unit.contains("ExecStart=/usr/local/bin/cortex heartbeat agent --env-file /home/me/.cortex/heartbeat-agent.env --host-id-path /home/me/.cortex/heartbeat-host-id"));
    assert!(!unit.contains("EnvironmentFile="));
    assert!(unit.contains("ReadWritePaths=/home/me/.cortex"));
}

#[test]
fn compose_runs_baked_image_with_host_network_and_private_id_path() {
    let compose = heartbeat_agent_compose(
        Path::new("/home/me/.local/bin/cortex"),
        Path::new("/home/me/.cortex/heartbeat-agent.env"),
        Path::new("/home/me/.cortex/heartbeat-host-id"),
    )
    .unwrap();

    assert!(compose.contains("network_mode: host"));
    assert!(compose.contains("- --env-file\n      - /home/me/.cortex/heartbeat-agent.env"));
    // Runs the published image with the binary baked in — no host binary is
    // staged or bind-mounted. Only the host-id data dir is mounted.
    assert!(compose.contains(&format!(
        "image: ghcr.io/dinglebear-ai/cortex:{}",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(compose.contains("user: \"0:0\""));
    assert!(compose.contains("disable: true"));
    assert!(!compose.contains(":/opt/cortex/bin"));
    assert!(!compose.contains("/usr/local/bin/cortex"));
    assert!(compose.contains("- cortex\n      - heartbeat"));
    assert!(compose.contains("- /home/me/.cortex:/home/me/.cortex"));
    assert!(compose.contains("- --host-id-path\n      - /home/me/.cortex/heartbeat-host-id"));
}

#[test]
fn shell_safe_value_rejects_control_characters() {
    assert_eq!(shell_safe_value("plain-token").unwrap(), "plain-token");
    assert!(shell_safe_value("bad\nvalue").is_err());
    assert!(shell_safe_value("bad\rvalue").is_err());
}

#[test]
fn remove_file_phase_treats_missing_file_as_success() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.service");

    let phase = remove_file_phase("heartbeat-agent-unit", &missing).unwrap();

    assert!(matches!(phase.status, SetupStatus::Ok));
    assert!(phase.detail.contains("already absent"));
}

#[test]
#[serial]
fn write_heartbeat_agent_env_writes_private_agent_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join("nested/heartbeat-agent.env");
    let _target = EnvGuard::remove("CORTEX_HEARTBEAT_TARGET");
    let _token = EnvGuard::remove("CORTEX_HEARTBEAT_TOKEN");
    let _setup_token = EnvGuard::remove("CORTEX_TOKEN");
    let _docker = EnvGuard::remove("CORTEX_AGENT_DOCKER");
    let _docker_url = EnvGuard::remove("CORTEX_AGENT_DOCKER_URL");
    let _journald = EnvGuard::remove("CORTEX_AGENT_JOURNALD");
    let _syslog_file = EnvGuard::remove("CORTEX_AGENT_SYSLOG_FILE");
    let _syslog_target = EnvGuard::remove("CORTEX_SYSLOG_TARGET");
    let _rust_log = EnvGuard::remove("RUST_LOG");
    let _file_tails = EnvGuard::remove("CORTEX_AGENT_FILE_TAILS");
    let _ai_transcripts = EnvGuard::remove("CORTEX_AGENT_AI_TRANSCRIPTS");
    let _ai_checkpoint = EnvGuard::remove("CORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT");
    let _command_forward = EnvGuard::remove("CORTEX_AGENT_COMMAND_FORWARD");
    let _command_spool = EnvGuard::remove("CORTEX_AGENT_COMMAND_SPOOL");
    let _shell_history = EnvGuard::remove("CORTEX_AGENT_SHELL_HISTORY_FORWARD");
    let _shell_history_checkpoint = EnvGuard::remove("CORTEX_AGENT_SHELL_HISTORY_CHECKPOINT");
    let _auto_update = EnvGuard::remove("CORTEX_AGENT_AUTO_UPDATE");
    let _preserve = EnvGuard::remove(PRESERVE_ENV);

    let phase = write_heartbeat_agent_env(&env_path).unwrap();
    let raw = std::fs::read_to_string(&env_path).unwrap();

    assert!(matches!(phase.status, SetupStatus::Ok));
    assert!(raw.contains(&format!(
        "CORTEX_HEARTBEAT_TARGET={}\n",
        heartbeat_agent::DEFAULT_TARGET
    )));
    assert!(raw.contains("CORTEX_AGENT_DOCKER=false\n"));
    assert!(raw.contains(&format!(
        "CORTEX_AGENT_DOCKER_URL={}\n",
        heartbeat_agent::DEFAULT_DOCKER_URL
    )));
    assert!(raw.contains("CORTEX_AGENT_JOURNALD=false\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&env_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
#[serial]
fn write_heartbeat_agent_env_preserves_existing_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join("heartbeat-agent.env");
    std::fs::write(&env_path, "CORTEX_HEARTBEAT_TOKEN=existing-secret\n").unwrap();
    let _preserve = EnvGuard::set(PRESERVE_ENV, "1");

    let phase = write_heartbeat_agent_env(&env_path).unwrap();

    assert!(matches!(phase.status, SetupStatus::Ok));
    assert!(phase.detail.contains("preserved existing"));
    assert_eq!(
        std::fs::read_to_string(env_path).unwrap(),
        "CORTEX_HEARTBEAT_TOKEN=existing-secret\n"
    );
}

#[test]
fn write_heartbeat_agent_unit_creates_parent_and_expected_content() {
    let dir = tempfile::tempdir().unwrap();
    let unit_path = dir
        .path()
        .join(".config/systemd/user/cortex-heartbeat-agent.service");
    let cortex_bin = Path::new("/usr/local/bin/cortex");
    let env_path = Path::new("/home/me/.cortex/heartbeat-agent.env");
    let host_id_path = Path::new("/home/me/.cortex/heartbeat-host-id");

    let phase = write_heartbeat_agent_unit(&unit_path, cortex_bin, env_path, host_id_path).unwrap();
    let raw = std::fs::read_to_string(&unit_path).unwrap();

    assert!(matches!(phase.status, SetupStatus::Ok));
    assert!(raw.contains("[Service]\nType=simple"));
    assert!(raw.contains(
        "ExecStart=/usr/local/bin/cortex heartbeat agent --env-file /home/me/.cortex/heartbeat-agent.env --host-id-path /home/me/.cortex/heartbeat-host-id"
    ));
    assert!(raw.contains("ReadWritePaths=/home/me/.cortex /usr/local/bin"));
}

#[test]
fn write_heartbeat_agent_compose_creates_compose_file() {
    let dir = tempfile::tempdir().unwrap();
    let compose_dir = dir.path().join("compose");
    std::fs::create_dir_all(&compose_dir).unwrap();

    let phase = write_heartbeat_agent_compose(
        &compose_dir,
        Path::new("/home/me/.local/bin/cortex"),
        Path::new("/home/me/.cortex/heartbeat-agent.env"),
        Path::new("/home/me/.cortex/heartbeat-host-id"),
    )
    .unwrap();
    let raw = std::fs::read_to_string(compose_dir.join("docker-compose.yml")).unwrap();

    assert!(matches!(phase.status, SetupStatus::Ok));
    assert!(raw.contains("cortex-heartbeat-agent:"));
    assert!(raw.contains("restart: unless-stopped"));
    assert!(raw.contains("network_mode: host"));
}

#[test]
fn heartbeat_agent_assets_reject_unit_breaking_paths() {
    assert!(
        heartbeat_agent_unit(
            Path::new("/usr/local/bin/cortex"),
            Path::new("/home/me/bad path/heartbeat-agent.env"),
            Path::new("/home/me/.cortex/heartbeat-host-id"),
        )
        .is_err()
    );
    assert!(
        heartbeat_agent_compose(
            Path::new("/home/me/.local/bin/cortex"),
            Path::new("/home/me/.cortex/heartbeat-agent.env"),
            Path::new("/home/me/bad path/heartbeat-host-id"),
        )
        .is_err()
    );
}

#[test]
fn docker_compose_up_phase_reports_error_when_workdir_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-compose-dir");

    let phase = docker_compose_up_phase(&missing);

    assert!(matches!(phase.status, SetupStatus::Error));
    assert_eq!(phase.name, "heartbeat-agent-docker-up");
    assert!(!phase.detail.trim().is_empty());
}

#[test]
fn content_phase_detects_matching_and_stale_units() {
    let dir = tempfile::tempdir().unwrap();
    let unit_path = dir.path().join("cortex-heartbeat-agent.service");
    let cortex_bin = Path::new("/usr/local/bin/cortex");
    let env_path = Path::new("/home/me/.cortex/heartbeat-agent.env");
    let host_id_path = Path::new("/home/me/.cortex/heartbeat-host-id");
    std::fs::write(
        &unit_path,
        heartbeat_agent_unit(cortex_bin, env_path, host_id_path).unwrap(),
    )
    .unwrap();

    let matching = check_heartbeat_agent_content(&unit_path, cortex_bin, env_path, host_id_path);
    assert!(matches!(matching.status, SetupStatus::Ok));

    std::fs::write(&unit_path, "stale unit").unwrap();
    let stale = check_heartbeat_agent_content(&unit_path, cortex_bin, env_path, host_id_path);
    assert!(matches!(stale.status, SetupStatus::Error));
    assert!(
        stale
            .detail
            .contains("does not match generated heartbeat agent unit")
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
#[serial]
async fn run_heartbeat_agent_setup_install_check_and_remove_with_systemd() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&cortex_home).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(&bin_dir.join("cortex"), "#!/bin/sh\nexit 0\n");
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-enabled*) printf 'enabled\\n' ;;\n  *is-active*) printf 'active\\n' ;;\n  *) printf 'ok\\n' ;;\nesac\nexit 0\n",
    );

    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));
    let _target = EnvGuard::set("CORTEX_HEARTBEAT_TARGET", "http://cortex.local/heartbeat");
    let _token = EnvGuard::set("CORTEX_HEARTBEAT_TOKEN", "secret");
    let _docker = EnvGuard::set("CORTEX_AGENT_DOCKER", "true");
    let _journald = EnvGuard::set("CORTEX_AGENT_JOURNALD", "true");
    let _docker_url = EnvGuard::set("CORTEX_AGENT_DOCKER_URL", "unix:///tmp/docker.sock");

    let install = run_heartbeat_agent_setup(HeartbeatAgentAction::Install)
        .await
        .unwrap();
    assert_eq!(install.mode, "heartbeat-agent-install");
    assert!(
        install.phases.iter().any(|phase| {
            phase.name == "heartbeat-agent-unit" && phase.status == SetupStatus::Ok
        })
    );
    let env_raw = std::fs::read_to_string(cortex_home.join("heartbeat-agent.env")).unwrap();
    assert!(env_raw.contains("CORTEX_HEARTBEAT_TARGET=http://cortex.local/heartbeat\n"));
    assert!(env_raw.contains("CORTEX_HEARTBEAT_TOKEN=secret\n"));
    assert!(env_raw.contains("CORTEX_AGENT_DOCKER=true\n"));

    let check = run_heartbeat_agent_setup(HeartbeatAgentAction::Check)
        .await
        .unwrap();
    assert_eq!(check.mode, "heartbeat-agent-check");
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "heartbeat-agent-active" && phase.detail == "active")
    );

    let remove = run_heartbeat_agent_setup(HeartbeatAgentAction::Remove)
        .await
        .unwrap();
    assert_eq!(remove.mode, "heartbeat-agent-remove");
    assert!(
        !home
            .join(".config/systemd/user/cortex-heartbeat-agent.service")
            .exists()
    );
}

#[test]
fn backend_selection_is_pure_and_never_falls_back_after_selection() {
    assert_eq!(
        select_backend("macos", false, true, false),
        ServiceBackend::Launchd
    );
    assert_eq!(
        select_backend("macos", false, false, false),
        ServiceBackend::Unsupported
    );
    assert_eq!(
        select_backend("linux", true, false, true),
        ServiceBackend::Systemd
    );
    assert_eq!(
        select_backend("linux", false, false, true),
        ServiceBackend::Compose
    );
    assert_eq!(
        select_backend("linux", false, false, false),
        ServiceBackend::Unsupported
    );
}

#[test]
#[serial]
fn write_heartbeat_agent_env_reads_setup_env_fallbacks_and_optional_syslog() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    std::fs::create_dir_all(&cortex_home).unwrap();
    std::fs::write(
        cortex_home.join(".env"),
        "CORTEX_TOKEN=from-setup-env\nCORTEX_HEARTBEAT_TARGET=http://from-env-file/heartbeat\n",
    )
    .unwrap();
    std::fs::write(
        cortex_home.join("agent-ai-transcript-checkpoint.json"),
        "{\"files\":{}}",
    )
    .unwrap();
    let env_path = dir.path().join("heartbeat-agent.env");

    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);
    let _target = EnvGuard::remove("CORTEX_HEARTBEAT_TARGET");
    let _token = EnvGuard::remove("CORTEX_HEARTBEAT_TOKEN");
    let _syslog_file = EnvGuard::set("CORTEX_AGENT_SYSLOG_FILE", "/var/log/syslog");
    let _syslog_target = EnvGuard::set("CORTEX_SYSLOG_TARGET", "127.0.0.1:1514");
    let _file_tails = EnvGuard::set("CORTEX_AGENT_FILE_TAILS", "/var/log/app.log:app");
    let _ai_transcripts = EnvGuard::set("CORTEX_AGENT_AI_TRANSCRIPTS", "true");
    let _command_forward = EnvGuard::set("CORTEX_AGENT_COMMAND_FORWARD", "true");
    let _shell_history = EnvGuard::set("CORTEX_AGENT_SHELL_HISTORY_FORWARD", "true");
    let _auto_update = EnvGuard::set("CORTEX_AGENT_AUTO_UPDATE", "false");
    let _transcript_checkpoint = EnvGuard::remove("CORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT");

    write_heartbeat_agent_env(&env_path).unwrap();
    let raw = std::fs::read_to_string(&env_path).unwrap();

    assert!(raw.contains("CORTEX_HEARTBEAT_TARGET=http://from-env-file/heartbeat\n"));
    assert!(raw.contains("CORTEX_HEARTBEAT_TOKEN=from-setup-env\n"));
    assert!(raw.contains("CORTEX_AGENT_SYSLOG_FILE=/var/log/syslog\n"));
    assert!(raw.contains("CORTEX_SYSLOG_TARGET=127.0.0.1:1514\n"));
    assert!(raw.contains("CORTEX_AGENT_FILE_TAILS=/var/log/app.log:app\n"));
    assert!(raw.contains("CORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n"));
    assert!(!raw.contains("CORTEX_AGENT_AI_TRANSCRIPTS="));
    assert!(raw.contains("CORTEX_AGENT_COMMAND_FORWARD=true\n"));
    assert!(raw.contains("CORTEX_AGENT_SHELL_HISTORY_FORWARD=true\n"));
    assert!(raw.contains("CORTEX_AGENT_AUTO_UPDATE=false\n"));
    assert!(raw.contains(&format!(
        "CORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT={}\n",
        cortex_home.join("agent-ai-transcript-checkpoint.json").display()
    )));
}

#[test]
#[serial]
fn capability_check_is_redacted_and_reports_fresh_transcript_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join("heartbeat-agent.env");
    let checkpoint = dir.path().join("checkpoint.json");
    std::fs::write(&checkpoint, "{\"files\":{}}").unwrap();
    std::fs::write(
        &env_path,
        format!(
            "CORTEX_HEARTBEAT_TOKEN=top-secret\nCORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\nCORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT={}\nCORTEX_AGENT_DOCKER=false\nCORTEX_AGENT_SHELL_HISTORY_FORWARD=false\n",
            checkpoint.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let phases = check_capabilities_and_delivery(&env_path, ServiceBackend::Launchd);
    assert_eq!(phases.len(), 3);
    assert!(phases[0].detail.contains("transcripts=enabled"));
    assert!(phases[0].detail.contains("journald=n/a"));
    assert_eq!(phases[2].status, SetupStatus::Ok);
    let rendered = serde_json::to_string(&phases).unwrap();
    assert!(!rendered.contains("top-secret"));
    assert!(!rendered.contains(&checkpoint.to_string_lossy().to_string()));
}

#[test]
fn systemd_unit_start_limit_leaves_room_for_a_self_update_rollback() {
    // A rollback needs MAX_ATTEMPTS failed starts plus the restored one inside
    // the start-limit window, or systemd gives up before the rollback runs.
    let unit = heartbeat_agent_unit(
        Path::new("/home/agent/.local/bin/cortex"),
        Path::new("/home/agent/.cortex/heartbeat-agent.env"),
        Path::new("/home/agent/.cortex/heartbeat-host-id"),
    )
    .unwrap();
    let burst: u32 = unit
        .lines()
        .find_map(|line| line.strip_prefix("StartLimitBurst="))
        .expect("unit sets StartLimitBurst")
        .parse()
        .unwrap();
    assert!(
        burst > crate::agent::self_update::MAX_ATTEMPTS,
        "StartLimitBurst={burst} must exceed MAX_ATTEMPTS={}",
        crate::agent::self_update::MAX_ATTEMPTS
    );
}

#[test]
fn windows_uses_user_task_backend() {
    assert_eq!(
        select_backend("windows", false, false, false),
        ServiceBackend::WindowsTask
    );
}

#[test]
#[serial]
fn capability_consent_preserves_auth_and_rejects_unavailable_selection() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let env = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(&env,b"CORTEX_HEARTBEAT_TOKEN=secret\nCORTEX_HEARTBEAT_TARGET=https://example.test\nCORTEX_AGENT_FILE_TAILS=/missing:test\n",0o600).unwrap();
    assert!(configure_agent_capabilities(&["file_tails".into()]).is_err());
    assert!(configure_agent_capabilities(&["unknown".into()]).is_err());
    configure_agent_capabilities(&[]).unwrap();
    let persisted = load_private_agent_env(&env).unwrap();
    assert_eq!(persisted["CORTEX_HEARTBEAT_TOKEN"], "secret");
    assert_eq!(persisted["CORTEX_HEARTBEAT_TARGET"], "https://example.test");
    assert_eq!(persisted["CORTEX_AGENT_DOCKER"], "false");
    assert!(
        discover_agent_capabilities()
            .unwrap()
            .iter()
            .all(|c| c.status == CapabilityStatus::Declined)
    );
}

#[test]
#[serial]
fn capability_discovery_matches_actual_history_collectors_and_preserves_legacy_consent() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let env = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(&env,b"CORTEX_AGENT_AI_TRANSCRIPTS=true\nCORTEX_AGENT_DOCKER=true\nCORTEX_AGENT_DOCKER_URL=http://docker.internal:2375\n",0o600).unwrap();
    std::fs::write(dir.path().join(".bash_history"), "command\n").unwrap();
    let capabilities = discover_agent_capabilities().unwrap();
    assert!(
        !capabilities
            .iter()
            .find(|c| c.id == "shell_history")
            .unwrap()
            .available
    );
    assert_eq!(
        capabilities
            .iter()
            .find(|c| c.id == "transcripts")
            .unwrap()
            .status,
        CapabilityStatus::NeedsConfiguration
    );
    assert_eq!(
        capabilities
            .iter()
            .find(|c| c.id == "docker")
            .unwrap()
            .detected_value
            .as_deref(),
        Some("http://docker.internal:2375")
    );
    std::fs::create_dir_all(dir.path().join(".local/share/atuin")).unwrap();
    std::fs::write(dir.path().join(".local/share/atuin/history.db"), "").unwrap();
    assert!(
        discover_agent_capabilities()
            .unwrap()
            .iter()
            .find(|c| c.id == "shell_history")
            .unwrap()
            .available
    );
}

#[test]
#[serial]
fn syslog_file_consent_clears_collection_source_but_preserves_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let log = dir.path().join("syslog");
    std::fs::write(&log, "event\n").unwrap();
    let path = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(&path,format!("CORTEX_AGENT_SYSLOG_FILE={}\nCORTEX_AGENT_AUTO_UPDATE=true\nCORTEX_SYSLOG_TARGET=localhost:1514\n",log.display()).as_bytes(),0o600).unwrap();
    assert_eq!(
        discover_agent_capabilities()
            .unwrap()
            .iter()
            .find(|c| c.id == "syslog_file")
            .unwrap()
            .status,
        CapabilityStatus::Enabled
    );
    configure_agent_capabilities(&[]).unwrap();
    let values = load_private_agent_env(&path).unwrap();
    assert_eq!(values["CORTEX_AGENT_SYSLOG_FILE"], "");
    assert_eq!(values["CORTEX_AGENT_AUTO_UPDATE"], "true");
    assert_eq!(values["CORTEX_SYSLOG_TARGET"], "localhost:1514");
}

#[test]
#[serial]
fn container_agent_mounts_selected_collection_sources_and_keeps_spool_writable() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let path = dir.path().join(".cortex/heartbeat-agent.env");
    let spool = dir.path().join(".local/state/cortex/agent-command.jsonl");
    std::fs::create_dir_all(spool.parent().unwrap()).unwrap();
    std::fs::write(&spool, "").unwrap();
    std::fs::write(dir.path().join(".zsh_history"), "command\n").unwrap();
    atomic_private_write(&path,format!("CORTEX_AGENT_SHELL_HISTORY_FORWARD=true\nCORTEX_AGENT_COMMAND_FORWARD=true\nCORTEX_AGENT_COMMAND_SPOOL={}\nCORTEX_AGENT_DOCKER=false\n",spool.display()).as_bytes(),0o600).unwrap();
    let raw = heartbeat_agent_compose(
        Path::new("/bin/cortex"),
        &path,
        &dir.path().join(".cortex/host-id"),
    )
    .unwrap();
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw).unwrap();
    let service = &yaml["services"]["cortex-heartbeat-agent"];
    assert_eq!(
        service["environment"]["HOME"].as_str(),
        Some(dir.path().to_str().unwrap())
    );
    let mounts = service["volumes"].as_sequence().unwrap();
    assert!(
        mounts
            .iter()
            .any(|m| m["source"].as_str() == spool.parent().unwrap().to_str()
                && m["read_only"].as_bool() == Some(false))
    );
    assert!(
        mounts
            .iter()
            .any(|m| m["source"].as_str() == dir.path().to_str()
                && m["read_only"].as_bool() == Some(true))
    );
    assert!(!raw.contains("docker.sock"));
}

#[test]
#[cfg(unix)]
#[serial]
fn loaded_launchd_job_without_running_process_is_not_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("launchctl");
    write_executable(
        &program,
        "#!/bin/sh\ncase \"$2\" in *transcript-forwarder) exit 113;; esac\nprintf 'state = waiting\\n'\n",
    );
    let _program = EnvGuard::set("CORTEX_TEST_LAUNCHCTL", &program);
    let _home = EnvGuard::set("CORTEX_HOME", dir.path());
    let phase = launchd_service_observation(dir.path());
    assert_eq!(phase.status, SetupStatus::Error);
    assert!(phase.detail.contains("no running process"));
}

#[test]
#[serial]
fn systemd_only_requires_selected_spool_and_supports_custom_location() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let env = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(&env, b"CORTEX_AGENT_COMMAND_FORWARD=false\n", 0o600).unwrap();
    let unit = heartbeat_agent_unit(
        Path::new("/bin/cortex"),
        &env,
        &dir.path().join(".cortex/host-id"),
    )
    .unwrap();
    assert!(!unit.contains(".local/state/cortex"));
    let spool = dir.path().join("custom-spool/events.jsonl");
    atomic_private_write(
        &env,
        format!(
            "CORTEX_AGENT_COMMAND_FORWARD=true\nCORTEX_AGENT_COMMAND_SPOOL={}\n",
            spool.display()
        )
        .as_bytes(),
        0o600,
    )
    .unwrap();
    let unit = heartbeat_agent_unit(
        Path::new("/bin/cortex"),
        &env,
        &dir.path().join(".cortex/host-id"),
    )
    .unwrap();
    assert!(unit.contains(spool.parent().unwrap().to_str().unwrap()));
}

#[test]
fn agent_compose_never_reuses_a_server_or_shared_compose_directory() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("compose");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("docker-compose.yml"),
        "services:\n  cortex:\n    image: server\n",
    )
    .unwrap();
    assert_eq!(
        compose_sources::compose_dir(dir.path()),
        dir.path().join("heartbeat-agent-compose")
    );
    std::fs::write(
        legacy.join("docker-compose.yml"),
        "services:\n  cortex-heartbeat-agent:\n    image: agent\n",
    )
    .unwrap();
    assert_eq!(compose_sources::compose_dir(dir.path()), legacy);
    std::fs::write(legacy.join("docker-compose.yml"),"services:\n  cortex-heartbeat-agent:\n    image: agent\n  another-service:\n    image: retained\n").unwrap();
    assert_eq!(
        compose_sources::compose_dir(dir.path()),
        dir.path().join("heartbeat-agent-compose")
    );
}

#[test]
#[serial]
fn custom_journald_image_is_preserved_and_default_image_fails_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let env = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(&env, b"CORTEX_AGENT_JOURNALD=true\n", 0o600).unwrap();
    assert!(
        compose_sources::collection_mounts(&env)
            .unwrap_err()
            .to_string()
            .contains("journalctl")
    );
    let compose = dir.path().join(".cortex/heartbeat-agent-compose");
    std::fs::create_dir_all(&compose).unwrap();
    std::fs::write(
        compose.join("docker-compose.override.yml"),
        "services:\n  cortex-heartbeat-agent:\n    image: local/agent-with-journalctl\n",
    )
    .unwrap();
    assert!(compose_sources::collection_mounts(&env).is_ok());
    assert!(compose_sources::custom_journal_image(&compose));
    assert_eq!(
        load_private_agent_env(&env).unwrap()["CORTEX_AGENT_JOURNALD"],
        "true"
    );
}

#[test]
#[serial]
fn unavailable_explicit_docker_endpoint_does_not_switch_collection_target() {
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", dir.path());
    let _cortex = EnvGuard::set("CORTEX_HOME", dir.path().join(".cortex"));
    let env = dir.path().join(".cortex/heartbeat-agent.env");
    atomic_private_write(
        &env,
        b"CORTEX_AGENT_DOCKER=true\nCORTEX_AGENT_DOCKER_URL=unix:///missing/selected-daemon.sock\n",
        0o600,
    )
    .unwrap();
    let capabilities = discover_agent_capabilities().unwrap();
    let docker = capabilities.iter().find(|c| c.id == "docker").unwrap();
    assert_eq!(docker.status, CapabilityStatus::NeedsConfiguration);
    assert!(!docker.available);
    assert_eq!(
        docker.detected_value.as_deref(),
        Some("unix:///missing/selected-daemon.sock")
    );
    assert!(configure_agent_capabilities(&["docker".into()]).is_err());
    assert_eq!(
        load_private_agent_env(&env).unwrap()["CORTEX_AGENT_DOCKER_URL"],
        "unix:///missing/selected-daemon.sock"
    );
}

#[test]
#[serial]
fn windows_upgrade_stages_env_without_touching_previous_state_and_cleans_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.exe");
    let managed = temp.path().join("managed/cortex.exe");
    let env = temp.path().join("state/heartbeat-agent.env");
    std::fs::write(&source, b"new candidate").unwrap();
    atomic_private_write(&managed, b"previous binary", 0o755).unwrap();
    let original = b"CORTEX_HEARTBEAT_TOKEN='previous-secret'\nCORTEX_AGENT_AUTO_UPDATE=true\n";
    atomic_private_write(&env, original, 0o600).unwrap();
    let _preserve = EnvGuard::set(PRESERVE_ENV, "1");
    let (staged, _) = stage_windows_upgrade(&source, &managed, &env).unwrap();
    assert_eq!(std::fs::read(&staged.env).unwrap(), original);
    assert_eq!(std::fs::read(&env).unwrap(), original);
    assert_eq!(std::fs::read(&managed).unwrap(), b"previous binary");
    let staged_env = staged.env.clone();
    let staged_binary = staged.binary.clone();
    drop(staged); // Candidate validation/task-preflight failures take this path.
    assert!(!staged_env.exists());
    assert!(!staged_binary.exists());
    assert_eq!(std::fs::read(&env).unwrap(), original);
    assert_eq!(std::fs::read(&managed).unwrap(), b"previous binary");
}

#[test]
#[serial]
fn windows_upgrade_env_preflight_failure_preserves_old_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.exe");
    let managed = temp.path().join("managed/cortex.exe");
    let env = temp.path().join("state/heartbeat-agent.env");
    std::fs::write(&source, b"candidate").unwrap();
    atomic_private_write(&managed, b"old binary", 0o755).unwrap();
    let original = b"CORTEX_HEARTBEAT_TOKEN=old-token\n";
    atomic_private_write(&env, original, 0o600).unwrap();
    let _preserve = EnvGuard::remove(PRESERVE_ENV);
    let _token = EnvGuard::set("CORTEX_HEARTBEAT_TOKEN", "invalid\nsecret");
    assert!(stage_windows_upgrade(&source, &managed, &env).is_err());
    assert_eq!(std::fs::read(&env).unwrap(), original);
    assert_eq!(std::fs::read(&managed).unwrap(), b"old binary");
    assert!(!env.with_extension("candidate.env").exists());
    assert!(!managed.with_file_name("cortex.candidate.exe").exists());
}
