use super::*;
use serial_test::serial;

struct EnvGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }

    fn remove(name: &'static str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::remove_test_var(name);
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

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn global_flags_extracts_bare_and_value_forms_without_touching_command_args() {
    let mut args = strings(&[
        "--http",
        "search",
        "--server=https://cortex.example",
        "disk",
        "--token",
        "secret",
        "--json",
    ]);

    let flags = GlobalFlags::extract(&mut args).unwrap();

    assert_eq!(
        flags,
        GlobalFlags {
            force_http: true,
            server: Some("https://cortex.example".into()),
            token: Some("secret".into()),
        }
    );
    assert_eq!(args, strings(&["search", "disk", "--json"]));
    assert_eq!(flags.http_flag_trigger(), Some("--http"));
}

#[test]
fn global_flags_http_equals_url_sets_server_and_transport() {
    // `--http=<url>` is the curl-style shortcut: enables HTTP transport AND
    // sets the server, so a URL no longer has to go through `--server`.
    let mut args = strings(&[
        "--http=http://localhost:40110",
        "correlate",
        "topic",
        "axon",
    ]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(
        flags,
        GlobalFlags {
            force_http: true,
            server: Some("http://localhost:40110".into()),
            token: None,
        }
    );
    assert_eq!(args, strings(&["correlate", "topic", "axon"]));
}

#[test]
fn global_flags_http_equals_empty_is_rejected() {
    let mut args = strings(&["--http=", "stats"]);
    let err = GlobalFlags::extract(&mut args).unwrap_err().to_string();
    assert!(err.contains("--http=<url> requires a value"), "got: {err}");
}

#[test]
fn global_flags_stop_at_double_dash_for_wrapped_commands() {
    let mut args = strings(&[
        "ingest",
        "agent-command",
        "wrap",
        "--token=outer",
        "--",
        "echo",
        "--http",
        "--token=inner",
    ]);

    let flags = GlobalFlags::extract(&mut args).unwrap();

    assert_eq!(flags.token.as_deref(), Some("outer"));
    assert_eq!(
        args,
        strings(&[
            "ingest",
            "agent-command",
            "wrap",
            "--",
            "echo",
            "--http",
            "--token=inner"
        ])
    );
}

#[test]
fn global_flags_report_actionable_value_errors() {
    for raw in [
        vec!["--server"],
        vec!["--server="],
        vec!["--server", ""],
        vec!["--token"],
        vec!["--token="],
        vec!["--token", "  "],
    ] {
        let mut args = strings(&raw);
        let error = GlobalFlags::extract(&mut args).unwrap_err().to_string();
        assert!(
            error.contains("requires"),
            "unexpected error for {raw:?}: {error}"
        );
    }
}

#[test]
#[serial]
fn http_trigger_prefers_flags_then_env_opt_in() {
    let _env = EnvGuard::set(ENV_USE_HTTP, "true");

    assert_eq!(
        GlobalFlags {
            force_http: false,
            server: Some("http://localhost:3100".into()),
            token: None,
        }
        .http_trigger(),
        Some("--server")
    );
    assert_eq!(
        GlobalFlags::default().http_trigger(),
        Some("CORTEX_USE_HTTP=1")
    );
}

#[test]
#[serial]
fn env_http_opt_in_accepts_only_trueish_values() {
    let _env = EnvGuard::remove(ENV_USE_HTTP);
    assert!(!env_opts_into_http());

    for value in ["1", "true", " TRUE "] {
        let _env = EnvGuard::set(ENV_USE_HTTP, value);
        assert!(env_opts_into_http(), "{value}");
    }
    for value in ["", "0", "false", "falze"] {
        let _env = EnvGuard::set(ENV_USE_HTTP, value);
        assert!(!env_opts_into_http(), "{value}");
    }
}

#[test]
fn strip_eq_prefix_matches_only_exact_flag_equals_forms() {
    assert_eq!(
        strip_eq_prefix("--server=http://x", "--server"),
        Some("http://x")
    );
    assert_eq!(strip_eq_prefix("--server=", "--server"), Some(""));
    assert_eq!(strip_eq_prefix("--server", "--server"), None);
    assert_eq!(strip_eq_prefix("--serverish=http://x", "--server"), None);
}
