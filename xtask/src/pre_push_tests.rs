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
fn docs_only_push_has_no_local_gate() {
    assert!(plan_for(&["docs/SETUP.md"], false).is_empty());
}

#[test]
fn rust_change_runs_clippy_without_full_tests() {
    let plan = plan_for(&["src/web_app.rs"], false);
    assert!(plan.contains(&"version-sync"));
    assert!(plan.contains(&"module-size"));
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
    let mut command = std::process::Command::new("bash");
    configure_shell_command(&mut command, "cargo --version");

    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(args, vec!["-c", "cargo --version"]);
}

#[test]
fn full_mode_keeps_the_old_expensive_suite_available() {
    let plan = plan_for(&["README.md"], true);
    assert!(plan.contains(&"version-sync"));
    assert!(plan.contains(&"clippy"));
    assert!(plan.contains(&"release-versions"));
    assert!(plan.contains(&"full-tests"));
}
