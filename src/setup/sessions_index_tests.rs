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

#[test]
fn install_ai_index_timer_files_writes_script_service_and_timer() {
    let dir = tempfile::tempdir().unwrap();
    let bin_path = dir.path().join("bin/cortex-sessions-index");
    let systemd_dir = dir.path().join("systemd");
    let service_path = systemd_dir.join("cortex-sessions-index.service");
    let timer_path = systemd_dir.join("cortex-sessions-index.timer");

    let phase =
        install_ai_index_timer_files(&bin_path, &systemd_dir, &service_path, &timer_path).unwrap();

    assert_eq!(phase.status, SetupStatus::Ok);
    let script = std::fs::read_to_string(&bin_path).unwrap();
    assert!(script.contains("cortex sessions index --json"));
    assert!(script.contains("CORTEX_DOCKER_INGEST_ENABLED"));
    let service = std::fs::read_to_string(&service_path).unwrap();
    assert!(service.contains(&format!("ExecStart={}", bin_path.display())));
    let timer = std::fs::read_to_string(&timer_path).unwrap();
    assert!(timer.contains("OnUnitActiveSec=30min"));
}

#[test]
fn remove_ai_index_timer_files_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let bin_path = dir.path().join("bin/cortex-sessions-index");
    let service_path = dir.path().join("systemd/cortex-sessions-index.service");
    let timer_path = dir.path().join("systemd/cortex-sessions-index.timer");
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(service_path.parent().unwrap()).unwrap();
    std::fs::write(&bin_path, "script").unwrap();
    std::fs::write(&service_path, "service").unwrap();
    std::fs::write(&timer_path, "timer").unwrap();

    let first = remove_ai_index_timer_files(&bin_path, &service_path, &timer_path).unwrap();
    let second = remove_ai_index_timer_files(&bin_path, &service_path, &timer_path).unwrap();

    assert_eq!(first.status, SetupStatus::Ok);
    assert_eq!(second.status, SetupStatus::Ok);
    assert!(!bin_path.exists());
    assert!(!service_path.exists());
    assert!(!timer_path.exists());
}

#[test]
fn service_and_timer_units_keep_host_user_timer_contract() {
    let service = ai_index_service_unit(Path::new("/home/me/.local/bin/cortex-sessions-index"));
    let timer = ai_index_timer_unit();

    assert!(service.contains("Type=oneshot"));
    assert!(service.contains("ExecStart=/home/me/.local/bin/cortex-sessions-index"));
    assert!(timer.contains("OnBootSec=5min"));
    assert!(timer.contains("Persistent=true"));
    assert!(timer.contains("WantedBy=timers.target"));
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn run_ai_index_timer_setup_install_check_and_remove_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&cortex_home).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-enabled*) printf 'enabled\\n' ;;\n  *) printf 'ok\\n' ;;\nesac\nexit 0\n",
    );

    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let install = run_sessions_index_timer_setup(SessionsIndexTimerAction::Install)
        .await
        .unwrap();
    assert_eq!(install.mode, "sessions-index-timer-install");
    assert!(
        install
            .phases
            .iter()
            .any(|phase| phase.name == "sessions-index-timer-files"
                && phase.status == SetupStatus::Ok)
    );
    assert!(home.join(".local/bin/cortex-sessions-index").is_file());
    assert!(
        home.join(".config/systemd/user/cortex-sessions-index.service")
            .is_file()
    );
    assert!(
        home.join(".config/systemd/user/cortex-sessions-index.timer")
            .is_file()
    );

    let check = run_sessions_index_timer_setup(SessionsIndexTimerAction::Check)
        .await
        .unwrap();
    assert_eq!(check.mode, "sessions-index-timer-check");
    assert!(
        check
            .phases
            .iter()
            .any(|phase| phase.name == "systemctl-user" && phase.status == SetupStatus::Ok)
    );

    let remove = run_sessions_index_timer_setup(SessionsIndexTimerAction::Remove)
        .await
        .unwrap();
    assert_eq!(remove.mode, "sessions-index-timer-remove");
    assert!(!home.join(".local/bin/cortex-sessions-index").exists());
    assert!(
        !home
            .join(".config/systemd/user/cortex-sessions-index.service")
            .exists()
    );
    assert!(
        !home
            .join(".config/systemd/user/cortex-sessions-index.timer")
            .exists()
    );
}
