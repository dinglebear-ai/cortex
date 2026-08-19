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

fn path_with_prepended(dir: &std::path::Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = crate::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

#[cfg(unix)]
#[test]
fn install_agent_command_files_hardens_existing_spool_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    let spool = state_dir.join("agent-command.jsonl");
    std::fs::write(&spool, "keep\n").unwrap();
    std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o644)).unwrap();
    let wrapper = dir.path().join("bin/wrapper");

    let phase = install_agent_command_files(&wrapper, &spool, &state_dir, Path::new("/bin/sh"))
        .expect("install files");

    assert_eq!(phase.status, SetupStatus::Ok);
    assert_eq!(std::fs::read_to_string(&spool).unwrap(), "keep\n");
    let mode = std::fs::metadata(&spool).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn agent_command_state_phase_rejects_spool_symlink() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = dir.path().join("target.jsonl");
    std::fs::write(&target, "").unwrap();
    let spool = state_dir.join("agent-command.jsonl");
    std::os::unix::fs::symlink(&target, &spool).unwrap();

    let phase = agent_command_state_phase(&state_dir, &spool);

    assert_eq!(phase.status, SetupStatus::Error);
    assert!(phase.detail.contains("regular file"));
}

#[test]
fn agent_command_state_phase_rejects_spool_directory() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    let spool = state_dir.join("agent-command.jsonl");
    std::fs::create_dir(&spool).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let phase = agent_command_state_phase(&state_dir, &spool);

    assert_eq!(phase.status, SetupStatus::Error);
    assert!(phase.detail.contains("regular file"));
}

#[test]
fn agent_command_env_phase_accepts_claude_settings_json() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir(&claude_dir).unwrap();
    let wrapper = home.path().join(".local/bin/cortex-agent-command-wrapper");
    std::fs::write(
        claude_dir.join("settings.json"),
        format!(
            r#"{{"env":{{"CLAUDE_CODE_SHELL_PREFIX":"{}"}}}}"#,
            wrapper.display()
        ),
    )
    .unwrap();

    let phase = agent_command_env_phase(&wrapper, home.path());

    assert_eq!(phase.status, SetupStatus::Ok);
}

#[test]
fn agent_command_wrapper_script_execs_cortex_with_spool_and_passthrough_args() {
    let script = agent_command_wrapper_script(
        Path::new("/home/me/.local/bin/cortex"),
        Path::new("/home/me/.local/state/cortex/agent-command.jsonl"),
    );

    assert!(script.starts_with("#!/usr/bin/env sh\n"));
    // Fail-open: probe the subcommand first, only delegate when it is runnable.
    assert!(script.contains("ingest shell agent wrap --probe >/dev/null 2>&1"));
    assert!(script.contains("exec /home/me/.local/bin/cortex ingest shell agent wrap"));
    assert!(script.contains("--spool /home/me/.local/state/cortex/agent-command.jsonl -- \"$@\""));
    // ...and run the command directly if the probe fails, so logging can never
    // brick the shell.
    assert!(script.contains("fi\nexec \"$@\"\n"));
}

#[test]
fn agent_command_content_phase_accepts_matching_wrapper_and_rejects_stale_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("wrapper");
    let spool = dir.path().join("agent-command.jsonl");
    let cortex_bin = Path::new("/usr/local/bin/cortex");
    std::fs::write(&wrapper, agent_command_wrapper_script(cortex_bin, &spool)).unwrap();

    let matching = agent_command_content_phase(&wrapper, &spool, cortex_bin);
    assert_eq!(matching.status, SetupStatus::Ok);

    std::fs::write(&wrapper, "#!/bin/sh\nexec old\n").unwrap();
    let stale = agent_command_content_phase(&wrapper, &spool, cortex_bin);
    assert_eq!(stale.status, SetupStatus::Error);
    assert!(
        stale
            .detail
            .contains("does not match generated agent command wrapper")
    );
}

#[test]
fn agent_command_state_phase_reports_missing_state_and_ready_state() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let spool = state_dir.join("agent-command.jsonl");

    let missing = agent_command_state_phase(&state_dir, &spool);
    assert_eq!(missing.status, SetupStatus::Warn);
    assert!(missing.detail.contains("missing"));

    std::fs::create_dir(&state_dir).unwrap();
    std::fs::write(&spool, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let ready = agent_command_state_phase(&state_dir, &spool);
    assert_eq!(ready.status, SetupStatus::Ok);
    assert!(ready.detail.contains("state ready"));
}

#[test]
fn agent_command_env_phase_warns_on_mismatched_or_malformed_settings() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir(&claude_dir).unwrap();
    let wrapper = home.path().join(".local/bin/cortex-agent-command-wrapper");
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"env":{"CLAUDE_CODE_SHELL_PREFIX":"/tmp/old-wrapper"}}"#,
    )
    .unwrap();

    let mismatched = agent_command_env_phase(&wrapper, home.path());
    assert_eq!(mismatched.status, SetupStatus::Warn);
    assert!(mismatched.detail.contains("expected"));

    std::fs::write(claude_dir.join("settings.json"), "{not-json").unwrap();
    let malformed = agent_command_env_phase(&wrapper, home.path());
    assert_eq!(malformed.status, SetupStatus::Warn);
    assert!(malformed.detail.contains("parse"));
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn run_shell_agent_setup_install_check_and_remove_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&cortex_home).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_cortex = bin_dir.join("cortex");
    write_executable(
        &fake_cortex,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'cortex {}\\n'; exit 0; fi\nexit 0\n",
            env!("CARGO_PKG_VERSION")
        ),
    );

    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));
    let wrapper = home.join(".local/bin/cortex-agent-command-wrapper");
    let _prefix = EnvGuard::set("CLAUDE_CODE_SHELL_PREFIX", &wrapper);

    let install = run_shell_agent_setup(ShellAgentAction::Install)
        .await
        .unwrap();
    assert_eq!(install.mode, "shell-agent-install");
    assert!(
        install
            .phases
            .iter()
            .any(|phase| phase.name == "agent-command-files" && phase.status == SetupStatus::Ok)
    );
    assert!(wrapper.is_file());
    assert!(
        home.join(".local/state/cortex/agent-command.jsonl")
            .is_file()
    );

    let check = run_shell_agent_setup(ShellAgentAction::Check)
        .await
        .unwrap();
    assert_eq!(check.mode, "shell-agent-check");
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "agent-command-content" && phase.status == SetupStatus::Ok)
    );
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "agent-command-state" && phase.status == SetupStatus::Ok)
    );

    let remove = run_shell_agent_setup(ShellAgentAction::Remove)
        .await
        .unwrap();
    assert_eq!(remove.mode, "shell-agent-remove");
    assert!(!wrapper.exists());
    assert!(
        remove
            .phases
            .iter()
            .any(|phase| phase.name == "agent-command-wrapper" && phase.status == SetupStatus::Ok)
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn run_shell_agent_setup_rejects_stale_cortex_binary_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&cortex_home).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_cortex = bin_dir.join("cortex");
    write_executable(
        &fake_cortex,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'cortex 0.0.0\\n'; exit 0; fi\nexit 0\n",
    );

    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let error = run_shell_agent_setup(ShellAgentAction::Install)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not the current cortex binary"));
    assert!(
        !home
            .join(".local/bin/cortex-agent-command-wrapper")
            .exists()
    );
}
