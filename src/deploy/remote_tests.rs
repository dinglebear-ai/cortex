use super::super::remote_support::{RemoteOutput, RemoteRunner};
use super::*;

#[derive(Default)]
struct FakeRemoteRunner {
    commands: Vec<String>,
    fail_contains: Option<&'static str>,
    error_contains: Option<&'static str>,
    existing_env: Option<String>,
}

impl FakeRemoteRunner {
    fn ok() -> Self {
        Self::default()
    }

    fn fail_on(needle: &'static str) -> Self {
        Self {
            commands: Vec::new(),
            fail_contains: Some(needle),
            error_contains: None,
            existing_env: None,
        }
    }

    fn error_on(needle: &'static str) -> Self {
        Self {
            commands: Vec::new(),
            fail_contains: None,
            error_contains: Some(needle),
            existing_env: None,
        }
    }

    fn with_existing_env(existing_env: impl Into<String>) -> Self {
        Self {
            commands: Vec::new(),
            fail_contains: None,
            error_contains: None,
            existing_env: Some(existing_env.into()),
        }
    }
}

impl RemoteRunner for FakeRemoteRunner {
    fn latest_stable_version(&mut self) -> io::Result<String> {
        self.commands
            .push("resolve latest stable release".to_string());
        Ok("9.8.7".to_string())
    }

    fn run(&mut self, host: &str, script: &str, _stdin: Option<&str>) -> io::Result<RemoteOutput> {
        self.commands.push(format!("{host}: {script}"));
        if self
            .error_contains
            .is_some_and(|needle| script.contains(needle))
        {
            return Err(io::Error::new(io::ErrorKind::NotFound, "ssh not found"));
        }
        if self
            .fail_contains
            .is_some_and(|needle| script.contains(needle))
        {
            return Ok(RemoteOutput {
                status_success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: "forced failure\n".to_string(),
            });
        }
        let stdout = if script.contains("__CORTEX_VERIFY__") {
            r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"cortex"}}}
__CORTEX_REPLY__
{"jsonrpc":"2.0","id":2,"result":{"content":[]}}
__CORTEX_REPLY__
{}
__CORTEX_REPLY__
{}"#
            .to_string()
        } else if script.contains("cat ")
            && (script.contains("/.env'") || script.contains("/compose/.env'"))
        {
            self.existing_env.clone().unwrap_or_default()
        } else if script.contains("id -u") {
            "/home/syslog\n1001\n1002\n".to_string()
        } else {
            "ok\n".to_string()
        };
        Ok(RemoteOutput {
            status_success: true,
            exit_code: Some(0),
            stdout,
            stderr: String::new(),
        })
    }
}

#[test]
fn remote_dry_run_only_checks_ssh_and_docker() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", true, &mut runner).unwrap();

    assert_eq!(report.host, "host-a");
    assert!(
        runner
            .commands
            .iter()
            .any(|cmd| cmd.contains("docker --version"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|cmd| cmd.contains("docker compose") && cmd.ends_with("compose up -d cortex"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|cmd| cmd.contains("cat > ~/.cortex/.env"))
    );
}

#[test]
fn remote_repair_writes_assets_before_compose_up() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    assert!(!report.has_errors);
    let index = |needle: &str| {
        runner
            .commands
            .iter()
            .position(|cmd| cmd.contains(needle))
            .unwrap_or_else(|| panic!("missing command containing {needle}"))
    };
    let mkdir = index(
        "mkdir -p '/home/syslog/.cortex/compose/config' '/home/syslog/.cortex/data' '/home/syslog/.cortex/backups' && chmod 700 '/home/syslog/.cortex/backups'",
    );
    let docker_check = index("docker --version && docker compose version");
    let env_write = index("cat > '/home/syslog/.cortex/.env.tmp'");
    let assets_write = index("docker-compose.yml.tmp");
    let compose_pull = index("pull --ignore-buildable");
    let compose_up = index("compose up -d cortex");
    assert!(mkdir < docker_check);
    assert!(docker_check < env_write);
    assert!(env_write < assets_write);
    assert!(assets_write < compose_pull);
    assert!(compose_pull < compose_up);
}

#[test]
fn remote_repair_health_check_retries_after_compose_up() {
    let mut runner = FakeRemoteRunner::ok();
    run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    let health = runner
        .commands
        .iter()
        .find(|cmd| cmd.contains("http://127.0.0.1:3100/health"))
        .expect("remote health command should run");
    assert!(health.contains("deadline=$(( $(date +%s) + 300 ))"));
    assert!(health.contains("sleep 1"));
}

#[test]
fn remote_repair_home_override_targets_existing_home_and_preserves_remote_env() {
    let mut runner = FakeRemoteRunner::with_existing_env(
        "CORTEX_AUTH_MODE=oauth\nCORTEX_VERSION=dev\nCORTEX_TOKEN=keep-token\n",
    );
    let report = run_remote_deploy_with_runner(
        "nashost",
        RemoteDeployOptions {
            dry_run: false,
            home: Some("/mnt/cache/appdata/cortex".to_string()),
            update_latest: false,
        },
        &mut runner,
    )
    .unwrap();

    assert!(!report.has_errors);
    assert_eq!(report.home, "/mnt/cache/appdata/cortex");
    assert_eq!(report.env_path, "/mnt/cache/appdata/cortex/.env");
    assert_eq!(report.compose_dir, "/mnt/cache/appdata/cortex/compose");
    assert_eq!(report.data_dir, "/mnt/cache/appdata/cortex/data");
    assert!(
        runner
            .commands
            .iter()
            .any(|cmd| cmd.contains("cat '/mnt/cache/appdata/cortex/.env'")
                && cmd.contains("cat '/mnt/cache/appdata/cortex/compose/.env'"))
    );
    assert!(runner.commands.iter().any(|cmd| cmd.contains(
        "mkdir -p '/mnt/cache/appdata/cortex/compose/config' '/mnt/cache/appdata/cortex/data' '/mnt/cache/appdata/cortex/backups' && chmod 700 '/mnt/cache/appdata/cortex/backups'"
    )));
    let env_write = runner
        .commands
        .iter()
        .find(|cmd| cmd.contains("cat > '/mnt/cache/appdata/cortex/.env.tmp'"))
        .expect("env write command should target the override home");
    assert!(env_write.contains("CORTEX_AUTH_MODE=oauth"));
    assert!(env_write.contains("CORTEX_TOKEN=keep-token"));
    assert!(env_write.contains("CORTEX_VERSION=dev"));
    assert!(env_write.contains("CORTEX_DATA_VOLUME=/mnt/cache/appdata/cortex/data"));
    assert!(env_write.contains("CORTEX_BACKUP_DIR=/mnt/cache/appdata/cortex/backups"));
    assert!(env_write.contains(
        "mv '/mnt/cache/appdata/cortex/compose/.env' '/mnt/cache/appdata/cortex/compose/.env.legacy'"
    ));
    assert!(env_write.contains("chmod 600 '/mnt/cache/appdata/cortex/compose/.env.legacy'"));
    assert!(!runner.commands.iter().any(|cmd| cmd.contains("~/.cortex")));
}

#[test]
fn remote_deploy_preserves_and_provisions_custom_backup_bind_before_up() {
    let mut runner =
        FakeRemoteRunner::with_existing_env("CORTEX_BACKUP_DIR=/srv/cortex-recovery\n");
    run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    let index = |needle: &str| {
        runner
            .commands
            .iter()
            .position(|cmd| cmd.contains(needle))
            .unwrap_or_else(|| panic!("missing command containing {needle}"))
    };
    let provision = index(
        "mkdir -p '/home/syslog/.cortex/compose/config' '/home/syslog/.cortex/data' '/srv/cortex-recovery' && chmod 700 '/srv/cortex-recovery'",
    );
    let compose_up = index("compose up -d cortex");
    assert!(provision < compose_up);
}

#[test]
fn remote_dry_run_does_not_provision_backup_bind() {
    let mut runner = FakeRemoteRunner::ok();
    run_remote_deploy_with_runner("host-a", true, &mut runner).unwrap();
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| { command.contains("backups") || command.contains("chmod 700") })
    );
}

#[test]
fn remote_env_uses_remote_uid_and_gid() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    let env_write = runner
        .commands
        .iter()
        .find(|cmd| cmd.contains(&format!("cat > '{}/.env.tmp'", report.home)))
        .expect("env write command should run");
    assert!(env_write.contains("CORTEX_UID=1001"));
    assert!(env_write.contains("CORTEX_GID=1002"));
}

#[test]
fn remote_deploy_skips_mutations_after_identity_failure() {
    let mut runner = FakeRemoteRunner::fail_on("id -u");
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    assert!(report.has_errors);
    assert!(
        !runner
            .commands
            .iter()
            .any(|cmd| cmd.ends_with("compose up -d cortex"))
    );
    assert!(report.phases.iter().any(|phase| {
        phase.name == "remote-compose-up" && matches!(phase.status, SetupStatus::Skipped)
    }));
}

#[test]
fn remote_deploy_reports_identity_spawn_error_as_phase_failure() {
    let mut runner = FakeRemoteRunner::error_on("id -u");
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    assert!(report.has_errors);
    let identity = report
        .phases
        .iter()
        .find(|phase| phase.name == "remote-identity")
        .expect("identity phase should be present");
    assert!(matches!(identity.status, SetupStatus::Error));
    assert!(identity.detail.contains("ssh failed"));
    assert!(
        !runner
            .commands
            .iter()
            .any(|cmd| cmd.ends_with("compose up -d cortex"))
    );
}

#[test]
fn remote_deploy_does_not_source_env_as_shell() {
    let mut runner = FakeRemoteRunner::ok();
    run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();

    assert!(
        !runner
            .commands
            .iter()
            .any(|cmd| cmd.contains(". ~/.cortex/.env"))
    );
}

#[test]
fn remote_deploy_rejects_option_like_hosts_before_running_ssh() {
    let mut runner = FakeRemoteRunner::ok();

    let error = run_remote_deploy_with_runner("-oProxyCommand=touch /tmp/pwned", true, &mut runner)
        .unwrap_err();

    assert!(error.to_string().contains("unsafe ssh host"));
    assert!(runner.commands.is_empty());
}

#[test]
fn remote_deploy_rejects_relative_home_before_running_ssh() {
    let mut runner = FakeRemoteRunner::ok();

    let error = run_remote_deploy_with_runner(
        "nashost",
        RemoteDeployOptions {
            dry_run: true,
            home: Some("relative/path".to_string()),
            update_latest: false,
        },
        &mut runner,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("--home must be an absolute path")
    );
    assert!(runner.commands.is_empty());
}

#[test]
fn remote_deploy_accepts_safe_hosts() {
    let mut runner = FakeRemoteRunner::ok();

    let report = run_remote_deploy_with_runner("nashost", true, &mut runner).unwrap();

    assert_eq!(report.host, "nashost");
    assert!(
        runner
            .commands
            .iter()
            .all(|command| command.starts_with("nashost:"))
    );
}

#[test]
fn existing_environment_read_failure_aborts_before_writes() {
    let mut runner = FakeRemoteRunner::fail_on("then cat");
    assert!(run_remote_deploy_with_runner("host-a", false, &mut runner).is_err());
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.contains("cat >"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.ends_with("compose up -d cortex"))
    );
}

#[test]
fn remote_staging_scripts_fail_fast() {
    let mut runner = FakeRemoteRunner::ok();
    run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    let staging: Vec<_> = runner
        .commands
        .iter()
        .filter(|command| command.contains("cat >"))
        .collect();
    assert_eq!(staging.len(), 2);
    for script in staging {
        assert!(script.contains("set -eu\n"));
    }
}

#[cfg(unix)]
#[test]
fn staging_failures_preserve_existing_remote_files() {
    let mut runner = FakeRemoteRunner::ok();
    run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    for script in runner
        .commands
        .iter()
        .filter(|command| command.contains("cat >"))
    {
        for failing in ["cat", "chmod", "mv"] {
            // Asset staging has no chmod operation.
            if failing == "chmod" && !script.contains("chmod ") {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("compose/config")).unwrap();
            let files = [
                ".env",
                "compose/docker-compose.yml",
                "compose/config/Dockerfile",
            ];
            for file in files {
                std::fs::write(dir.path().join(file), "LAST_GOOD").unwrap();
            }
            let script = script
                .strip_prefix("host-a: ")
                .unwrap()
                .replace("/home/syslog/.cortex", dir.path().to_str().unwrap());
            let injection = if failing == "cat" {
                "cat() { command cat >/dev/null; printf PARTIAL; return 1; }\n".to_string()
            } else {
                format!("{failing}() {{ return 1; }}\n")
            };
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{injection}{script}"))
                .status()
                .unwrap();
            assert!(!status.success(), "{failing} must fail staging");
            for file in files {
                assert_eq!(
                    std::fs::read_to_string(dir.path().join(file)).unwrap(),
                    "LAST_GOOD",
                    "{file}, failed {failing}"
                );
            }
        }
    }
}

#[test]
fn recovery_failure_blocks_configuration_changes_and_upgrades() {
    let mut runner = FakeRemoteRunner::fail_on("# Managed Compose recovery");
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    assert!(report.has_errors);
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.contains("cat >"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.ends_with("compose up -d cortex"))
    );
}

#[test]
fn dry_run_reads_saved_pin_but_never_stops_or_snapshots_service() {
    let mut runner =
        FakeRemoteRunner::with_existing_env("CORTEX_VERSION=3.12.0\nCORTEX_PORT=3200\n");
    let report = run_remote_deploy_with_runner("host-a", true, &mut runner).unwrap();
    assert_eq!(report.health_url, "http://127.0.0.1:3200/health");
    assert!(
        runner
            .commands
            .iter()
            .any(|command| command.contains("cat '/home/syslog/.cortex/.env'"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.contains("# Managed Compose recovery")
                || command.contains("compose stop"))
    );
}

#[test]
fn readiness_failure_retains_recovery_and_skips_authentication_probes() {
    let mut runner = FakeRemoteRunner::fail_on("deadline=$((");
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    assert!(report.has_errors);
    assert!(
        runner
            .commands
            .iter()
            .any(|command| command.contains("# Managed Compose recovery"))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.contains("__CORTEX_VERIFY__"))
    );
}

#[test]
fn update_compose_commands_keep_custom_override_and_recovery_pin() {
    let script = compose_command("/srv/cortex");
    assert!(script.contains("docker-compose.override.yml"));
    assert!(script.contains("docker-compose.recovery.yml"));
    assert!(script.contains("--project-directory"));
}

#[test]
fn unpinned_update_resolves_latest_without_creating_a_permanent_env_pin() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner(
        "host-a",
        RemoteDeployOptions {
            update_latest: true,
            ..Default::default()
        },
        &mut runner,
    )
    .unwrap();
    assert!(!report.has_errors);
    assert!(
        runner
            .commands
            .iter()
            .any(|command| command == "resolve latest stable release")
    );
    let env_write = runner
        .commands
        .iter()
        .find(|command| command.contains("/.env.tmp'"))
        .unwrap();
    assert!(!env_write.contains("CORTEX_VERSION=9.8.7"));
    let asset_write = runner
        .commands
        .iter()
        .find(|command| command.contains("__CORTEX_COMPOSE__"))
        .unwrap();
    assert!(asset_write.contains("${CORTEX_VERSION:-9.8.7}"));
}

#[test]
fn pinned_update_keeps_exact_version_and_skips_latest_discovery() {
    let mut runner = FakeRemoteRunner::with_existing_env("CORTEX_VERSION=session-explicit\n");
    run_remote_deploy_with_runner(
        "host-a",
        RemoteDeployOptions {
            update_latest: true,
            ..Default::default()
        },
        &mut runner,
    )
    .unwrap();
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command == "resolve latest stable release")
    );
    assert!(
        runner
            .commands
            .iter()
            .any(|command| command.contains("CORTEX_VERSION=session-explicit"))
    );
}

#[test]
fn deployment_lease_spans_snapshot_writes_and_authenticated_verification() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    assert!(!report.has_errors);
    let acquired = runner
        .commands
        .iter()
        .position(|s| s.contains("external-deployment"))
        .unwrap();
    let snapshot = runner
        .commands
        .iter()
        .position(|s| s.contains("set -- 'prepare'"))
        .unwrap();
    let verified = runner
        .commands
        .iter()
        .position(|s| s.contains("__CORTEX_VERIFY__"))
        .unwrap();
    assert!(acquired < snapshot && snapshot < verified);
    assert!(
        runner.commands[snapshot..=verified]
            .iter()
            .all(|s| s.contains("export CORTEX_LIFECYCLE_TOKEN="))
    );
    assert!(
        runner
            .commands
            .last()
            .unwrap()
            .contains("rmdir \"$home/.lifecycle-lock\"; fi")
    );
}

#[test]
fn deployment_lease_is_released_when_mutating_phase_returns_io_error() {
    let mut runner = FakeRemoteRunner::error_on("cat > '/home/syslog/.cortex/.env.tmp'");
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    assert!(report.has_errors);
    assert!(
        report
            .phases
            .iter()
            .any(|phase| phase.name == "remote-env" && matches!(phase.status, SetupStatus::Error))
    );
    assert!(
        !runner
            .commands
            .iter()
            .any(|command| command.ends_with("compose up -d cortex"))
    );
    assert!(
        runner
            .commands
            .last()
            .unwrap()
            .contains("rmdir \"$home/.lifecycle-lock\"; fi")
    );
}
