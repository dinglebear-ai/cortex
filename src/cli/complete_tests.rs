use super::*;
use std::sync::{Mutex, OnceLock};

/// Serializes tests that share the `CORTEX_DB_PATH` test-overlay key so parallel
/// execution cannot interfere semantically.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn completes_action_names_with_descriptions() {
    let out = complete(&["actions".into()]).unwrap();
    assert!(out.iter().any(|line| line.starts_with("search\t")));
    assert!(out.iter().any(|line| line.starts_with("tail\t")));
}

#[test]
fn completions_omit_clean_break_roots() {
    let out = complete(&["actions".into()]).unwrap();
    for removed in ["ai", "source-ips", "silent-hosts", "service", "deploy"] {
        assert!(
            !out.iter()
                .any(|line| line == removed || line.starts_with(&format!("{removed}\t"))),
            "removed command {removed} must not appear in completions"
        );
    }
}

#[test]
fn completion_roots_have_help_entries() {
    for line in complete(&["actions".into()]).unwrap() {
        let root = line.split('\t').next().expect("candidate root");
        assert!(
            crate::cli::help::render_command(root, false).is_some(),
            "completion root {root} must be documented in command help"
        );
    }
}

#[test]
fn completes_flags_for_action() {
    let out = complete(&["flags".into(), "search".into()]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("--host\t")));
    assert!(out.iter().any(|l| l.starts_with("--since\t")));
    // short alias is offered alongside the long flag
    assert!(out.iter().any(|l| l.starts_with("-n\t")));
}

#[test]
fn completes_every_nested_command_with_single_word_tokens() {
    for (parent, children) in COMMAND_CHILDREN {
        assert!(
            parent.split_whitespace().all(|token| !token.contains('-')),
            "hyphenated command token in catalog path {parent}"
        );
        assert!(
            children.iter().all(|token| !token.contains('-')),
            "hyphenated child command under {parent}: {children:?}"
        );
    }
    for path in ["sessions", "state", "ingest", "setup", "db"] {
        let out = complete(&["subcommands".into(), path.into()]).unwrap();
        assert!(!out.is_empty(), "missing nested completions for {path}");
        assert!(
            out.iter().all(|value| !value.contains('-')),
            "hyphenated nested completion under {path}: {out:?}"
        );
    }
    let sessions = complete(&["subcommands".into(), "sessions".into()]).unwrap();
    assert!(sessions.contains(&"skillinvestigate".to_string()));
    assert!(sessions.contains(&"mcpevents".to_string()));
}

#[test]
fn nested_paths_resolve_shared_action_flags() {
    let out = complete(&["flags".into(), "state host".into()]).unwrap();
    assert!(out.iter().any(|line| line.starts_with("--host\t")));
}

#[test]
fn flags_resolve_from_longest_leaf_after_positionals_and_options() {
    let nested = complete(&[
        "flags".into(),
        "sessions".into(),
        "hookinvestigate".into(),
        "format-on-save".into(),
        "--since".into(),
        "1h".into(),
    ])
    .unwrap();
    assert!(nested.iter().any(|line| line.starts_with("--hook-event\t")));
    assert!(nested.iter().any(|line| line.starts_with("--all\t")));

    let top_level = complete(&[
        "flags".into(),
        "search".into(),
        "disk".into(),
        "full".into(),
        "--host".into(),
        "devhost".into(),
    ])
    .unwrap();
    assert!(top_level.iter().any(|line| line.starts_with("--since\t")));
}

#[test]
fn resolved_leaf_does_not_offer_subcommands_after_arguments() {
    let out = complete(&[
        "subcommands".into(),
        "sessions".into(),
        "mcpinvestigate".into(),
        "labby".into(),
        "--since".into(),
        "1h".into(),
    ])
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn refreshed_event_leaves_expose_exact_parser_flags() {
    use std::collections::BTreeSet;

    let cases: &[(&str, &[&str])] = &[
        (
            "sessions skills",
            &[
                "--skill",
                "--plugin",
                "--tool",
                "--project",
                "--session-id",
                "--host",
                "--since",
                "--until",
                "--limit",
                "--json",
            ],
        ),
        (
            "sessions skillincidents",
            &[
                "--skill",
                "--plugin",
                "--tool",
                "--project",
                "--session-id",
                "--hostname",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--signal",
                "--min-score",
                "--json",
            ],
        ),
        (
            "sessions skillinvestigate",
            &[
                "--incident-id",
                "--plugin",
                "--tool",
                "--project",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--correlation-window-minutes",
                "--all",
                "--json",
            ],
        ),
        (
            "sessions mcpevents",
            &[
                "--tool-name",
                "--mcp-server",
                "--mcp-tool",
                "--tool",
                "--project",
                "--session-id",
                "--host",
                "--error-only",
                "--since",
                "--until",
                "--limit",
                "--json",
            ],
        ),
        (
            "sessions mcpincidents",
            &[
                "--mcp-server",
                "--mcp-tool",
                "--tool-name",
                "--tool",
                "--project",
                "--session-id",
                "--hostname",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--signal",
                "--min-score",
                "--json",
            ],
        ),
        (
            "sessions mcpinvestigate",
            &[
                "--incident-id",
                "--mcp-server",
                "--mcp-tool",
                "--tool-name",
                "--tool",
                "--project",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--correlation-window-minutes",
                "--all",
                "--json",
            ],
        ),
        (
            "sessions hookevents",
            &[
                "--hook-event",
                "--hook",
                "--hook-source",
                "--status",
                "--evidence-kind",
                "--tool",
                "--project",
                "--session-id",
                "--host",
                "--since",
                "--until",
                "--limit",
                "--json",
            ],
        ),
        (
            "sessions hookincidents",
            &[
                "--hook-event",
                "--hook",
                "--hook-source",
                "--tool",
                "--project",
                "--session-id",
                "--hostname",
                "--evidence-kind",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--signal",
                "--min-score",
                "--json",
            ],
        ),
        (
            "sessions hookinvestigate",
            &[
                "--incident-id",
                "--hook-event",
                "--hook",
                "--hook-source",
                "--tool",
                "--project",
                "--since",
                "--until",
                "--limit",
                "--window-minutes",
                "--correlation-window-minutes",
                "--all",
                "--json",
            ],
        ),
    ];

    for (path, expected) in cases {
        let actual: BTreeSet<_> = complete(&["flags".into(), (*path).into()])
            .unwrap()
            .into_iter()
            .map(|line| line.split('\t').next().unwrap().to_string())
            .collect();
        let expected: BTreeSet<_> = expected.iter().map(|flag| (*flag).to_string()).collect();
        assert_eq!(actual, expected, "flag metadata drift for {path}");
    }
}

#[test]
fn completion_does_not_advertise_deferred_syslog_test() {
    let out = complete(&["subcommands".into(), "ingest syslog".into()]).unwrap();
    assert_eq!(out, vec!["status"]);
}

#[test]
fn completes_static_enum_values_for_severity() {
    let out = complete(&["value".into(), "--severity".into()]).unwrap();
    assert!(out.iter().any(|l| l == "err"));
    assert!(out.iter().any(|l| l == "warning"));
}

#[test]
fn completes_time_hints() {
    let out = complete(&["value".into(), "--since".into()]).unwrap();
    assert!(out.iter().any(|l| l == "1h"));
    assert!(out.iter().any(|l| l == "yesterday"));
}

#[test]
fn dynamic_value_degrades_to_ok_without_db() {
    // Point at a nonexistent DB; host completion must return Ok (empty), never
    // panic or error — completion degrades silently to static candidates.
    let _guard = env_lock().lock().expect("env lock poisoned");
    // Restore CORTEX_DB_PATH on scope exit (including panic) so other tests that
    // read it are unaffected.
    struct RestoreDbPath(Option<std::ffi::OsString>);
    impl Drop for RestoreDbPath {
        fn drop(&mut self) {
            {
                match self.0.take() {
                    Some(v) => crate::env::set_test_var("CORTEX_DB_PATH", v),
                    None => crate::env::remove_test_var("CORTEX_DB_PATH"),
                }
            }
        }
    }
    let _restore = RestoreDbPath(crate::env::var_os("CORTEX_DB_PATH"));
    crate::env::set_test_var("CORTEX_DB_PATH", "/nonexistent/cortex-complete-test.db");
    let out = complete(&["value".into(), "--host".into()]);
    assert!(out.is_ok(), "dynamic completion must not error: {out:?}");
}

#[test]
fn unknown_context_errors() {
    assert!(complete(&["bogus".into()]).is_err());
}
