use super::*;

fn names(plan: &[PlanStep]) -> Vec<&'static str> {
    plan.iter().map(|step| step.name).collect()
}

fn plan_for(paths: &[&str], full: bool) -> Vec<&'static str> {
    let owned = paths
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();
    let categories = classify(&owned, full);
    names(&command_plan(&owned, &categories, full))
}

#[test]
fn docs_only_push_runs_instruction_and_generator_gates() {
    for path in [
        "docs/SETUP.md",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "CONTRIBUTING.md",
        "plugins/cortex/AGENTS.md",
        "contracts/rendered-session-page.schema.json",
        "packages/cortex-rmcp/README.md",
        "Justfile",
        "LICENSE",
    ] {
        assert_eq!(
            plan_for(&[path], false),
            vec![
                "agent-instructions",
                "agent-instruction-tests",
                "repository-contract-tests",
                "generated-docs"
            ],
            "{path} should validate instructions and generated docs without the full Rust suite"
        );
    }
}

#[test]
fn rust_change_runs_clippy_without_full_tests() {
    let plan = plan_for(&["src/web_app.rs"], false);
    assert!(plan.contains(&"version-sync"));
    assert!(plan.contains(&"module-size"));
    assert!(plan.contains(&"generated-docs"));
    assert!(plan.contains(&"clippy"));
    assert!(!plan.contains(&"full-tests"));
}

#[test]
fn web_change_runs_focused_web_tests() {
    let plan = plan_for(&["web/app/app.js"], false);
    assert!(plan.contains(&"web-app-tests"));
    assert!(!plan.contains(&"full-tests"));
}

#[test]
fn hook_change_tests_router() {
    let plan = plan_for(&["lefthook.yml", "xtask/src/pre_push.rs"], false);
    assert!(plan.contains(&"pre-push-router-tests"));
    assert!(!plan.contains(&"full-tests"));
}

#[test]
fn pre_push_steps_preserve_the_callers_toolchain_environment() {
    let mut command = Command::new("bash");
    configure_shell_command(&mut command, "cargo --version");

    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(args, vec!["-c", "cargo --version"]);
}

#[test]
fn validation_fixtures_cannot_modify_the_hook_repository() {
    let temp = tempfile::tempdir().unwrap();
    let protected = temp.path().join("protected");
    let fixture = temp.path().join("fixture");
    std::fs::create_dir(&protected).unwrap();
    std::fs::create_dir(&fixture).unwrap();
    let mut init = Command::new("git");
    init.arg("-C").arg(&protected).arg("init");
    remove_repository_git_environment(Path::new("."), &mut init).unwrap();
    assert!(init.output().unwrap().status.success());
    let config_path = protected.join(".git/config");
    let original_config = std::fs::read(&config_path).unwrap();

    let mut command = Command::new("bash");
    configure_shell_command(
        &mut command,
        "git -C \"$FIXTURE_PATH\" init && git -C \"$FIXTURE_PATH\" config core.bare true",
    );
    command
        .env("FIXTURE_PATH", &fixture)
        .env("GIT_DIR", protected.join(".git"))
        .env("GIT_WORK_TREE", &protected)
        .env("GIT_INDEX_FILE", protected.join(".git/index"));
    remove_repository_git_environment(&protected, &mut command).unwrap();
    assert!(command.output().unwrap().status.success());
    assert_eq!(std::fs::read(config_path).unwrap(), original_config);
    assert!(fixture.join(".git/config").is_file());
    assert!(!protected.join(".git/index").exists());
}

#[test]
fn full_mode_keeps_the_old_expensive_suite_available() {
    let plan = plan_for(&["README.md"], true);
    assert!(plan.contains(&"version-sync"));
    assert!(plan.contains(&"clippy"));
    assert!(plan.contains(&"release-versions"));
    assert!(plan.contains(&"full-tests"));
}
