use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn exercise(failure: &str, existing: bool) -> (tempfile::TempDir, std::process::Output) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let env = dir.path().join("agent.env");
    let staged = dir.path().join("staged.env");
    if existing {
        std::fs::write(state.join("agent"), "true\n").unwrap();
        std::fs::write(&env, "OLD=preserved\n").unwrap();
    }
    std::fs::write(&staged, "NEW=candidate\n").unwrap();
    executable(
        &dir.path().join("timeout"),
        "#!/bin/sh\nshift\nexec \"$@\"\n",
    );
    executable(&dir.path().join("sleep"), "#!/bin/sh\nexit 0\n");
    executable(
        &dir.path().join("docker"),
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$STATE/commands"
case "$1" in
  container)
    name=${5#name=^/}
    name=${name%\$}
    if [ -f "$STATE/$name" ]; then echo "$name"; fi ;;
  inspect)
    if [ "$2" = --format ]; then
      if [ "$4" = agent ] && [ "$FAIL" = exits ]; then
        echo false
      else
        cat "$STATE/$4"
      fi
    else
      test -f "$STATE/$2"
    fi ;;
  rename)
    case "$2" in agent.previous-*) [ "$FAIL" != rollback ] || exit 1 ;; esac
    test ! -e "$STATE/$3"
    mv "$STATE/$2" "$STATE/$3"
    if [ "$2" = agent ] && [ "$FAIL" = rename_uncertain ]; then exit 1; fi ;;
  stop) echo false > "$STATE/$2" ;;
  run)
    [ "$FAIL" != create ] && [ "$FAIL" != rollback ] || exit 1
    echo true > "$STATE/agent" ;;
  exec)
    if [ "$FAIL" = version ] || [ "$FAIL" = remove ]; then echo 'cortex incorrect'; else echo 'cortex 9.9.9'; fi ;;
  rm) [ "$FAIL" != remove ] || exit 1; rm -f "$STATE/$3" ;;
  start) test -f "$STATE/$2"; echo true > "$STATE/$2" ;;
  *) exit 7 ;;
esac
"#,
    );
    let script = replacement_script(
        "agent",
        env.to_str().unwrap(),
        staged.to_str().unwrap(),
        "docker run -d --name agent candidate",
        "9.9.9",
    );
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("STATE", &state)
        .env("FAIL", failure)
        .output()
        .unwrap();
    (dir, output)
}

#[test]
fn failed_create_restores_previous_container_and_environment() {
    let (dir, output) = exercise("create", true);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("agent.env")).unwrap(),
        "OLD=preserved\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state/agent")).unwrap(),
        "true\n"
    );
    assert!(!dir.path().join("agent.env.deploy-lock").exists());
}

#[test]
fn wrong_version_and_early_exit_restore_previous_agent() {
    for failure in ["version", "exits"] {
        let (dir, output) = exercise(failure, true);
        assert!(!output.status.success(), "{failure}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("agent.env")).unwrap(),
            "OLD=preserved\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("state/agent")).unwrap(),
            "true\n"
        );
    }
}

#[test]
fn failed_first_install_removes_candidate_and_environment() {
    let (dir, output) = exercise("version", false);
    assert!(!output.status.success());
    assert!(!dir.path().join("state/agent").exists());
    assert!(!dir.path().join("agent.env").exists());
}

#[test]
fn validated_replacement_commits_new_environment_and_removes_old_container() {
    let (dir, output) = exercise("", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("agent.env")).unwrap(),
        "NEW=candidate\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state/agent")).unwrap(),
        "true\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("delivery unverified"));
    assert!(
        std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("previous"))
    );
}

#[test]
fn rollback_failure_retains_configuration_backup_and_recovery_lock() {
    for failure in ["rollback", "remove"] {
        let (dir, output) = exercise(failure, true);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("rollback failed; retained"));
        assert!(dir.path().join("agent.env.deploy-lock").exists());
        let backup = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("rollback-")
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "OLD=preserved\n");
    }
}

#[test]
fn uncertain_rename_failure_reconciles_names_and_restores_original_agent() {
    let (dir, output) = exercise("rename_uncertain", true);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("agent.env")).unwrap(),
        "OLD=preserved\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state/agent")).unwrap(),
        "true\n"
    );
    assert!(!dir.path().join("agent.env.deploy-lock").exists());
}
