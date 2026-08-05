use super::*;
use crate::scanner::hook_events::HookStatus;
use std::io::Write;

fn write_file(dir: &std::path::Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

/// Runs `body` with `$HOME` temporarily pointed at a fresh temp dir, then
/// restores the previous value. Serialized via `#[serial]` at each call site
/// (env vars are process-global) — see the two test functions below.
///
/// Restoration happens via an RAII guard (not a post-call statement) so a
/// panic inside `body` still restores `$HOME` instead of leaking the temp
/// dir's path into the rest of the test binary.
fn with_temp_home<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let _guard = HomeGuard(std::env::var_os("HOME"));
    unsafe {
        std::env::set_var("HOME", dir.path());
    }
    body(dir.path())
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn collect_claude_settings_parses_configured_hooks() {
    with_temp_home(|home| {
        write_file(
            home,
            ".claude/settings.json",
            r#"{
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {"type": "command", "command": "cargo fmt"}
                            ]
                        }
                    ]
                }
            }"#,
        );

        let collected = collect_hook_config("devhost", "2026-06-01T00:00:00.000Z");
        assert_eq!(collected.len(), 1);
        let row = &collected[0];
        assert_eq!(row.ai_tool, "claude");
        assert_eq!(row.event.hook_event, "PostToolUse");
        assert_eq!(row.event.hook_name.as_deref(), Some("Bash"));
        assert_eq!(row.event.hook_command.as_deref(), Some("cargo fmt"));
        assert_eq!(row.event.status, HookStatus::Configured);
        assert_eq!(row.event.evidence_kind.as_str(), "config_inventory");
    });
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn collect_codex_hooks_parses_configured_groups() {
    with_temp_home(|home| {
        write_file(
            home,
            ".codex/hooks.json",
            r#"{
                "Stop": [
                    {"name": "cleanup", "command": "echo done"}
                ]
            }"#,
        );

        let collected = collect_hook_config("devhost", "2026-06-01T00:00:00.000Z");
        assert_eq!(collected.len(), 1);
        let row = &collected[0];
        assert_eq!(row.ai_tool, "codex");
        assert_eq!(row.event.hook_event, "Stop");
        assert_eq!(row.event.hook_name.as_deref(), Some("cleanup"));
        assert_eq!(row.event.hook_command.as_deref(), Some("echo done"));
        assert_eq!(row.event.evidence_kind.as_str(), "config_inventory");
    });
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn collect_codex_trust_state_parses_trusted_hashes() {
    with_temp_home(|home| {
        write_file(
            home,
            ".codex/config.toml",
            "[hooks.state]\n\"stop:cleanup\" = \"abc123\"\n",
        );

        let collected = collect_hook_config("devhost", "2026-06-01T00:00:00.000Z");
        assert_eq!(collected.len(), 1);
        let row = &collected[0];
        assert_eq!(row.ai_tool, "codex");
        assert_eq!(row.event.hook_event, "hook_trust_state");
        assert_eq!(row.event.hook_name.as_deref(), Some("stop:cleanup"));
        assert_eq!(row.event.trusted_hash.as_deref(), Some("abc123"));
        assert_eq!(row.event.evidence_kind.as_str(), "trusted_hash_state");
    });
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn missing_files_yield_empty_collection_not_an_error() {
    with_temp_home(|_home| {
        let collected = collect_hook_config("devhost", "2026-06-01T00:00:00.000Z");
        assert!(collected.is_empty());
    });
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn malformed_json_is_skipped_not_fatal() {
    with_temp_home(|home| {
        write_file(home, ".claude/settings.json", "{not valid json");
        let collected = collect_hook_config("devhost", "2026-06-01T00:00:00.000Z");
        assert!(collected.is_empty());
    });
}

#[test]
#[serial_test::serial(hook_config_home_env)]
fn collect_and_store_is_idempotent() {
    use crate::config::StorageConfig;
    use crate::db::init_pool;

    with_temp_home(|home| {
        write_file(
            home,
            ".claude/settings.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo bye"}]}]}}"#,
        );

        let dir = tempfile::tempdir().unwrap();
        let storage = StorageConfig::for_test(dir.path().join("hook-config-test.db"));
        let pool = init_pool(&storage).unwrap();

        let first = collect_and_store(&pool, "devhost", "2026-06-01T00:00:00.000Z").unwrap();
        assert_eq!(first, 1);
        let second = collect_and_store(&pool, "devhost", "2026-06-01T00:00:00.000Z").unwrap();
        assert_eq!(
            second, 0,
            "repeated collection at the same timestamp must be idempotent"
        );
    });
}
