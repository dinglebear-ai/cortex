//! End-to-end: `cortex reflect` indexes Claude transcript lines from a
//! temporary HOME into the default reflect DB and reports a skill incident.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn now_minus(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn write_fixture(home: &Path) {
    let dir = home.join(".claude/projects/-tmp-reflect-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        serde_json::json!({
            "sessionId": "sess-e2e",
            "timestamp": now_minus(30),
            "attributionSkill": "cortex-troubleshoot",
            "attributionPlugin": "cortex",
            "content": "ran troubleshoot"
        }),
        serde_json::json!({
            "sessionId": "sess-e2e",
            "timestamp": now_minus(28),
            "content": "That's not what I asked for, please redo it."
        }),
    ];
    let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(dir.join("sess-e2e.jsonl"), body).unwrap();
}

fn cortex(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cortex"));
    command
        .args(args)
        .current_dir(home) // keep the repo's config.toml out of the run
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env_remove("CORTEX_DB_PATH")
        .env_remove("CORTEX_USE_HTTP")
        .env_remove("CORTEX_LLM")
        .env_remove("CORTEX_LLM_ENABLED")
        .env_remove("CORTEX_CODEX_CMD")
        .env_remove("CORTEX_API_TOKEN")
        .env_remove("CORTEX_URL");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn reflect_json(home: &Path, extra: &[&str], env: &[(&str, &str)]) -> serde_json::Value {
    let mut args = vec!["reflect", "--json", "--kinds", "skill"];
    args.extend_from_slice(extra);
    let output = cortex(home, &args, env);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn false_program() -> PathBuf {
    let paths = cortex::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&paths)
        .map(|dir| dir.join("false"))
        .find(|candidate| candidate.is_file())
        .expect("a `false` program on PATH")
}

#[test]
fn reflect_indexes_detects_and_reports_incrementally() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());

    let report = reflect_json(home.path(), &["--no-llm"], &[]);
    assert_eq!(report["mode"], "report_only");
    assert!(
        report["db_path"]
            .as_str()
            .unwrap()
            .ends_with(".cortex/reflect.db")
    );
    assert!(report["index"]["ingested"].as_u64().unwrap() >= 2);
    let top = &report["assessed"][0]["incident"];
    assert_eq!(top["kind"], "skill");
    assert_eq!(top["target"], "cortex:cortex-troubleshoot");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(home.path().join(".cortex"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let db_mode = std::fs::metadata(home.path().join(".cortex/reflect.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(db_mode & 0o777, 0o600);
    }

    let again = reflect_json(home.path(), &["--no-llm"], &[]);
    assert_eq!(
        again["index"]["ingested"], 0,
        "second run must be incremental"
    );
    assert_eq!(
        again["assessed"][0]["incident"]["target"],
        "cortex:cortex-troubleshoot"
    );
}

#[test]
fn a_failing_llm_program_keeps_the_report() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let program = false_program();
    let report = reflect_json(
        home.path(),
        &["--max-assess", "1"],
        &[
            ("CORTEX_LLM", "codex"),
            ("CORTEX_CODEX_CMD", program.to_str().unwrap()),
        ],
    );
    let entry = &report["assessed"][0];
    assert!(entry["assessment"].is_null());
    assert!(
        entry["failure"].is_string(),
        "expected a failure reason: {entry}"
    );
    assert!(entry["findings"].is_object());
    assert!(report["llm_fallback_reason"].is_null());
}

#[test]
fn a_missing_llm_program_downgrades_the_run() {
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let report = reflect_json(
        home.path(),
        &["--max-assess", "1"],
        &[
            ("CORTEX_LLM", "codex"),
            ("CORTEX_CODEX_CMD", "/nonexistent/cortex-reflect/codex"),
        ],
    );
    assert_eq!(report["mode"], "report_only");
    assert!(
        report["llm_fallback_reason"]
            .as_str()
            .unwrap()
            .contains("was not found")
    );
    assert!(report["assessed"][0]["findings"].is_object());
}

#[test]
fn reflect_server_mode_needs_a_token() {
    // --http reads incidents from a Cortex server; without a token the
    // client discovery fails closed before any local indexing.
    let home = tempfile::tempdir().unwrap();
    write_fixture(home.path());
    let output = cortex(home.path(), &["--http", "reflect", "--no-llm"], &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("CORTEX_API_TOKEN"));
    assert!(
        !home.path().join(".cortex/reflect.db").exists(),
        "server mode must fail before creating or indexing the local DB"
    );
}
