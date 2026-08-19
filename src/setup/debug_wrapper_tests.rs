use super::*;
use serial_test::serial;

struct EnvGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => crate::env::set_test_var(self.name, value),
            None => crate::env::remove_test_var(self.name),
        }
    }
}

#[test]
fn debug_wrapper_script_builds_current_repo_debug_binary_with_safe_defaults() {
    let script = debug_wrapper_script(Path::new("/home/jmagar/workspace/cortex"));

    assert!(script.starts_with("#!/usr/bin/env bash\nset -euo pipefail\n"));
    assert!(script.contains(r#"repo="${CORTEX_REPO:-/home/jmagar/workspace/cortex}""#));
    assert!(script.contains(r#"export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-.cache/cargo}""#));
    assert!(script.contains(
        r#"export CORTEX_DOCKER_INGEST_ENABLED="${CORTEX_DOCKER_INGEST_ENABLED:-false}""#
    ));
    assert!(script.contains(r#"export CORTEX_AUTH_MODE="${CORTEX_AUTH_MODE:-bearer}""#));
    assert!(script.contains(r#"exec "${CARGO_TARGET_DIR}/debug/cortex" "$@""#));
}

#[test]
fn debug_wrapper_script_keeps_serve_and_setup_env_unmodified() {
    let script = debug_wrapper_script(Path::new("/home/jmagar/workspace/cortex"));

    assert!(script.contains("case \"${1:-}\" in\n  serve|setup)\n    ;;\n  *)"));
}

#[test]
fn debug_compose_override_builds_debug_image_from_repo_context() {
    let override_yaml = debug_compose_override(Path::new("/home/jmagar/workspace/cortex"));

    assert_eq!(
        override_yaml,
        "services:\n  cortex:\n    image: cortex:local-debug\n    build:\n      context: /home/jmagar/workspace/cortex\n      dockerfile: config/Dockerfile\n      args:\n        CORTEX_BUILD_PROFILE: debug\n"
    );
}

#[test]
fn debug_wrapper_content_phase_reports_match_mismatch_and_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper_path = tmp.path().join("cortex");
    let repo_path = tmp.path().join("repo");
    std::fs::create_dir(&repo_path).unwrap();
    std::fs::write(&wrapper_path, debug_wrapper_script(&repo_path)).unwrap();

    let ok = check_debug_wrapper_content_phase(&wrapper_path, &repo_path);
    assert_eq!(ok.status, SetupStatus::Ok);
    assert!(ok.detail.contains("matches generated content"));

    std::fs::write(&wrapper_path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
    let stale = check_debug_wrapper_content_phase(&wrapper_path, &repo_path);
    assert_eq!(stale.status, SetupStatus::Error);
    assert!(
        stale
            .detail
            .contains("does not match generated debug wrapper")
    );

    let missing = check_debug_wrapper_content_phase(&tmp.path().join("missing"), &repo_path);
    assert_eq!(missing.status, SetupStatus::Error);
    assert!(missing.detail.contains("No such file") || missing.detail.contains("not found"));
}

#[test]
fn debug_compose_content_phase_reports_match_and_stale_content() {
    let tmp = tempfile::tempdir().unwrap();
    let override_path = tmp.path().join("docker-compose.override.yml");
    let repo_path = tmp.path().join("repo");
    std::fs::create_dir(&repo_path).unwrap();
    std::fs::write(&override_path, debug_compose_override(&repo_path)).unwrap();

    let ok = check_debug_compose_content_phase(&override_path, &repo_path);
    assert_eq!(ok.status, SetupStatus::Ok);
    assert!(ok.detail.contains("matches generated content"));

    std::fs::write(&override_path, "services: {}\n").unwrap();
    let stale = check_debug_compose_content_phase(&override_path, &repo_path);
    assert_eq!(stale.status, SetupStatus::Error);
    assert!(
        stale
            .detail
            .contains("does not match generated debug Compose override")
    );
}

#[tokio::test]
#[serial]
async fn debug_wrapper_setup_install_check_and_remove_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let cortex_home = tmp.path().join("cortex-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    let install = run_debug_wrapper_setup(DebugWrapperAction::Install)
        .await
        .unwrap();
    assert!(!install.has_errors);
    assert_eq!(install.mode, "debug-wrapper-install");
    assert!(home.join(".local/bin/cortex").is_file());

    let check = run_debug_wrapper_setup(DebugWrapperAction::Check)
        .await
        .unwrap();
    assert!(!check.has_errors);
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "debug-wrapper-content" && phase.status == SetupStatus::Ok)
    );

    let remove = run_debug_wrapper_setup(DebugWrapperAction::Remove)
        .await
        .unwrap();
    assert!(!remove.has_errors);
    assert!(!home.join(".local/bin/cortex").exists());

    let remove_again = run_debug_wrapper_setup(DebugWrapperAction::Remove)
        .await
        .unwrap();
    assert!(!remove_again.has_errors);
    assert!(
        remove_again
            .phases
            .iter()
            .any(|phase| phase.detail.contains("already absent"))
    );
}

#[tokio::test]
#[serial]
async fn debug_compose_setup_install_check_and_remove_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let cortex_home = tmp.path().join("cortex-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    let install = run_debug_compose_setup(DebugComposeAction::Install)
        .await
        .unwrap();
    assert!(!install.has_errors);
    assert_eq!(install.mode, "debug-compose-install");
    let override_path = cortex_home.join("compose/docker-compose.override.yml");
    assert!(override_path.is_file());

    let check = run_debug_compose_setup(DebugComposeAction::Check)
        .await
        .unwrap();
    assert!(!check.has_errors);
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "debug-compose-content" && phase.status == SetupStatus::Ok)
    );

    let remove = run_debug_compose_setup(DebugComposeAction::Remove)
        .await
        .unwrap();
    assert!(!remove.has_errors);
    assert!(!override_path.exists());
}
