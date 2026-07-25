use std::process::Command;

#[test]
fn help_lists_sessions_command() {
    // Explicit `--help` prints the grouped top-level banner to stdout (exit 0),
    // listing each command by name + summary. Per-command flags live behind
    // `cortex <command> --help`.
    let output = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .arg("--help")
        .output()
        .expect("run cortex --help");

    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("help output should be valid UTF-8");
    assert!(
        stdout.contains("sessions"),
        "top-level help should list the sessions command, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Commands"),
        "top-level help should have a Commands section, got:\n{stdout}"
    );
}

#[test]
fn per_command_help_shows_detailed_flags() {
    // `cortex sessions --help` prints that command's detailed flag reference.
    let output = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .args(["sessions", "--help"])
        .output()
        .expect("run cortex sessions --help");

    assert!(output.status.success(), "per-command --help should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    assert!(
        stdout.contains("cortex sessions") && stdout.contains("--project"),
        "per-command help should show detailed flags, got:\n{stdout}"
    );
}

#[test]
fn update_help_shows_server_clients_and_config_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .args(["update", "--help"])
        .output()
        .expect("run cortex update --help");

    assert!(output.status.success(), "update --help should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    assert!(stdout.contains("cortex update [all|server|clients|agents]"));
    assert!(stdout.contains("cortex update config server --host HOST --home PATH"));
    assert!(stdout.contains("cortex update config clients --hosts h1,h2"));
}

#[test]
fn sessions_cli_add_and_query_commands_emit_json() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cli-ai.db");
    let transcript = dir.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        "{\"sessionId\":\"cli-1\",\"content\":\"hello cli transcript\"}\n",
    )
    .unwrap();
    let transcript_path = transcript
        .to_str()
        .expect("transcript path should be UTF-8");

    let add = run_command(
        &db_path,
        &["sessions", "add", "--file", transcript_path, "--json"],
    );
    assert!(add.status.success(), "sessions add failed: {add:?}");
    let add_json: serde_json::Value = serde_json::from_slice(&add.stdout).unwrap();
    assert_eq!(add_json["ingested"], 1);

    let index = run_command(
        &db_path,
        &["sessions", "index", "--path", transcript_path, "--json"],
    );
    assert!(index.status.success(), "sessions index failed: {index:?}");
    let index_json: serde_json::Value = serde_json::from_slice(&index.stdout).unwrap();
    assert_eq!(index_json["skipped_dupes"], 1);

    let search = run_command(&db_path, &["sessions", "search", "hello", "--json"]);
    assert!(
        search.status.success(),
        "sessions search failed: {search:?}"
    );
    let search_json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(search_json["sessions"].as_array().unwrap().len(), 1);

    for args in [
        &["sessions", "blocks", "--json"][..],
        &["sessions", "tools", "--json"][..],
        &["sessions", "projects", "--json"][..],
        &["sessions", "--json"][..],
    ] {
        let output = run_command(&db_path, args);
        assert!(output.status.success(), "{args:?} failed: {output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    }

    let cwd = std::env::current_dir().unwrap();
    let context = run_command(
        &db_path,
        &[
            "sessions",
            "context",
            "--project",
            cwd.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        context.status.success(),
        "sessions context failed: {context:?}"
    );
    serde_json::from_slice::<serde_json::Value>(&context.stdout).unwrap();
}

#[test]
fn sessions_transcript_tail_uses_human_transcript_layout() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cli-ai-human.db");
    let transcript = dir.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        "{\"sessionId\":\"cli-human\",\"cwd\":\"/tmp/pretty-project\",\"content\":\"first line\\nsecond line\"}\n",
    )
    .unwrap();
    let transcript_path = transcript
        .to_str()
        .expect("transcript path should be UTF-8");

    let add = run_command(&db_path, &["sessions", "add", "--file", transcript_path]);
    assert!(add.status.success(), "sessions add failed: {add:?}");

    let tail = run_command(&db_path, &["tail", "-n", "1"]);
    assert!(tail.status.success(), "tail failed: {tail:?}");
    let stdout = String::from_utf8(tail.stdout).unwrap();
    assert!(stdout.contains("claude"), "missing tool label:\n{stdout}");
    assert!(
        stdout.contains("/tmp/pretty-project"),
        "missing project:\n{stdout}"
    );
    assert!(
        stdout.contains("session=cli-human"),
        "missing session:\n{stdout}"
    );
    assert!(
        stdout.contains("    first line\n    second line"),
        "message was not indented:\n{stdout}"
    );
    assert!(
        !stdout.contains(" localhost "),
        "transcript output leaked synthetic localhost:\n{stdout}"
    );
}

#[test]
fn sessions_cli_add_reports_parse_errors_and_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cli-ai-bad.db");
    let transcript = dir.path().join("bad.jsonl");
    std::fs::write(
        &transcript,
        "{\"sessionId\":\"cli-1\",\"content\":\"good\"}\nnot-json\n",
    )
    .unwrap();
    let transcript_path = transcript
        .to_str()
        .expect("transcript path should be UTF-8");

    let output = run_command(
        &db_path,
        &["sessions", "add", "--file", transcript_path, "--json"],
    );
    assert!(!output.status.success(), "sessions add unexpectedly passed");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ingested"], 1);
    assert_eq!(json["parse_errors"], 1);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("transcript record(s) failed to parse"));
}

fn run_command(db_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cortex"));
    command
        .env("CORTEX_DB_PATH", db_path)
        // CI wrappers may force colored subprocess output. These assertions
        // exercise the default plain-text layout, so isolate them from the
        // runner's presentation environment.
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE");
    command.args(args);
    command.output().expect("run syslog command")
}
