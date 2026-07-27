use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn classify(event: &str, files: &[&str]) -> HashMap<String, String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "cortex-ci-paths-{}-{}-{}",
        std::process::id(),
        files.len(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let changed = temp_dir.join("changed.txt");
    let output = temp_dir.join("github_output.txt");
    fs::write(&changed, files.join("\n")).expect("write changed file list");

    let status = Command::new("python3")
        .arg("scripts/ci/changed_paths.py")
        .arg("--event")
        .arg(event)
        .arg("--changed-files")
        .arg(&changed)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run changed_paths.py");
    assert!(status.success(), "changed_paths.py exited with {status}");

    let raw = fs::read_to_string(&output).expect("read github output");
    raw.lines()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key=value output");
            (key.to_string(), value.to_string())
        })
        .collect()
}

#[test]
fn docs_only_changes_skip_runtime_categories() {
    let out = classify("pull_request", &["docs/SETUP.md", "README.md"]);
    assert_eq!(out["docs"], "true");
    assert_eq!(out["rust"], "false");
    assert_eq!(out["web"], "false");
    assert_eq!(out["docker"], "false");
    assert_eq!(out["release"], "false");
    assert_eq!(out["mcp"], "false");
    assert_eq!(out["security"], "false");
}

#[test]
fn rust_changes_enable_runtime_security_release_and_mcp_smoke() {
    let out = classify("pull_request", &["src/mcp/tools.rs"]);
    assert_eq!(out["rust"], "true");
    assert_eq!(out["mcp"], "true");
    assert_eq!(out["security"], "true");
    assert_eq!(out["release"], "true");
}

#[test]
fn web_changes_enable_web_docker_and_release_without_rust_tests() {
    let out = classify("pull_request", &["web/app/app.js"]);
    assert_eq!(out["web"], "true");
    assert_eq!(out["docker"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["rust"], "false");
}

#[test]
fn plugin_skill_changes_enable_skill_and_release_gates() {
    let out = classify("pull_request", &["plugins/cortex/skills/cortex/SKILL.md"]);
    assert_eq!(out["skills"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["rust"], "false");
}

#[test]
fn launcher_and_installer_changes_enable_release_checks() {
    for file in [
        "packages/cortex-rmcp/package.json",
        "install.sh",
        "install.ps1",
    ] {
        let out = classify("pull_request", &[file]);
        assert_eq!(
            out["release"], "true",
            "{file} should enable release checks"
        );
        assert_eq!(out["rust"], "false", "{file} should not force Rust tests");
    }
}

#[test]
fn workflow_router_changes_force_full_ci() {
    for file in [
        ".github/workflows/ci.yml",
        "scripts/ci/changed_paths.py",
        "tests/ci_changed_paths.rs",
    ] {
        let out = classify("pull_request", &[file]);
        for key in [
            "all", "docs", "workflow", "rust", "web", "docker", "release", "skills", "security",
            "mcp",
        ] {
            assert_eq!(out[key], "true", "{file} should enable {key}");
        }
    }
}

#[test]
fn schedule_and_manual_runs_enable_everything() {
    for event in ["schedule", "workflow_dispatch"] {
        let out = classify(event, &[]);
        for key in [
            "all", "docs", "workflow", "rust", "web", "docker", "release", "skills", "security",
            "mcp",
        ] {
            assert_eq!(out[key], "true", "{event} should enable {key}");
        }
    }
}
