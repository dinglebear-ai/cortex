use super::super::ShellCompletionsAction;
use super::*;
use serial_test::serial;
use std::fs;

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

#[tokio::test]
#[serial]
async fn install_writes_completion_script() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    let path = shell_completions_install_path(&home);
    let report = run_shell_completions_setup(ShellCompletionsAction::Install)
        .await
        .unwrap();
    assert!(!report.has_errors);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("#compdef cortex"));
}

#[tokio::test]
#[serial]
async fn check_reports_warn_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    let report = run_shell_completions_setup(ShellCompletionsAction::Check)
        .await
        .unwrap();
    assert!(
        report
            .phases
            .iter()
            .any(|phase| phase.name == "shell-completions-content"
                && phase.status != SetupStatus::Ok)
    );
}

#[tokio::test]
#[serial]
async fn check_reports_ok_after_install() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    run_shell_completions_setup(ShellCompletionsAction::Install)
        .await
        .unwrap();
    let report = run_shell_completions_setup(ShellCompletionsAction::Check)
        .await
        .unwrap();
    assert!(
        report
            .phases
            .iter()
            .any(|phase| phase.name == "shell-completions-content"
                && phase.status == SetupStatus::Ok)
    );
}

#[tokio::test]
#[serial]
async fn remove_deletes_completion_script() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cortex_home = home.join(".cortex");
    std::fs::create_dir_all(&cortex_home).unwrap();
    let _home = EnvGuard::set("HOME", &home);
    let _cortex_home = EnvGuard::set("CORTEX_HOME", &cortex_home);

    run_shell_completions_setup(ShellCompletionsAction::Install)
        .await
        .unwrap();
    let path = shell_completions_install_path(&home);
    assert!(path.exists());
    run_shell_completions_setup(ShellCompletionsAction::Remove)
        .await
        .unwrap();
    assert!(!path.exists());
}
