use super::*;
use serial_test::serial;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    values: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new(names: &[&'static str]) -> Self {
        let values = names
            .iter()
            .map(|name| (*name, crate::env::var(name).ok()))
            .collect();
        for name in names {
            crate::env::remove_test_var(name);
        }
        Self { values }
    }

    fn set(&self, name: &'static str, value: &str) {
        crate::env::set_test_var(name, value);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.values.drain(..) {
            match value {
                Some(value) => crate::env::set_test_var(name, value),
                None => crate::env::remove_test_var(name),
            }
        }
    }
}

#[test]
fn strip_trailing_mcp_path_drops_slash_and_mcp() {
    assert_eq!(
        strip_trailing_mcp_path("https://cortex.example/mcp"),
        "https://cortex.example"
    );
    assert_eq!(
        strip_trailing_mcp_path("https://cortex.example/mcp/"),
        "https://cortex.example"
    );
    assert_eq!(
        strip_trailing_mcp_path("https://cortex.example/"),
        "https://cortex.example"
    );
    assert_eq!(
        strip_trailing_mcp_path("https://cortex.example"),
        "https://cortex.example"
    );
}

#[test]
fn append_csv_unique_appends_and_dedupes() {
    assert_eq!(append_csv_unique("", "a"), "a");
    assert_eq!(append_csv_unique("a", "b"), "a,b");
    assert_eq!(append_csv_unique("a,b", "b"), "a,b");
    // empty value is a no-op
    assert_eq!(append_csv_unique("a,b", ""), "a,b");
    // whitespace-trimmed comparison
    assert_eq!(append_csv_unique("a, b", "b"), "a, b");
}

#[test]
fn reject_unsafe_value_errors_on_newline_and_cr() {
    assert!(reject_unsafe_value("X", "ok").is_ok());
    assert!(reject_unsafe_value("X", "bad\nvalue").is_err());
    assert!(reject_unsafe_value("X", "bad\rvalue").is_err());
}

#[test]
#[serial]
fn prepare_plugin_hook_env_maps_server_options_without_client_probe() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = EnvGuard::new(&[
        "CLAUDE_PLUGIN_OPTION_IS_SERVER",
        "CLAUDE_PLUGIN_OPTION_API_TOKEN",
        "CLAUDE_PLUGIN_OPTION_SERVER_URL",
        "CLAUDE_PLUGIN_OPTION_MCP_PORT",
        "CLAUDE_PLUGIN_OPTION_DOCKER_INGEST_ENABLED",
        "CLAUDE_PLUGIN_OPTION_NO_AUTH",
        "CORTEX_TOKEN",
        "CORTEX_SERVER_URL",
        "CORTEX_PORT",
        "CORTEX_DOCKER_INGEST_ENABLED",
        "NO_AUTH",
        "CORTEX_AUTH_ALLOWED_REDIRECT_URIS",
        "CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH",
    ]);
    env.set("CLAUDE_PLUGIN_OPTION_API_TOKEN", "token-123");
    env.set(
        "CLAUDE_PLUGIN_OPTION_SERVER_URL",
        "https://cortex.example/mcp",
    );
    env.set("CLAUDE_PLUGIN_OPTION_MCP_PORT", "43100");
    env.set("CLAUDE_PLUGIN_OPTION_DOCKER_INGEST_ENABLED", "false");
    env.set("CLAUDE_PLUGIN_OPTION_NO_AUTH", "1");

    let prep = prepare_plugin_hook_env().unwrap();
    let _guard = match prep {
        HookPrep::Server(guard) => guard,
        HookPrep::Client => panic!("expected server plugin preparation"),
    };

    assert_eq!(
        cortex::config::config_env_var("CORTEX_TOKEN").as_deref(),
        Some("token-123")
    );
    assert_eq!(
        cortex::config::config_env_var("CORTEX_SERVER_URL").as_deref(),
        Some("https://cortex.example/mcp")
    );
    assert_eq!(
        cortex::config::config_env_var("CORTEX_PORT").as_deref(),
        Some("43100")
    );
    assert_eq!(
        cortex::config::config_env_var("CORTEX_DOCKER_INGEST_ENABLED").as_deref(),
        Some("false")
    );
    assert_eq!(
        cortex::config::config_env_var("NO_AUTH").as_deref(),
        Some("1")
    );
}

#[test]
#[serial]
fn prepare_oauth_env_derives_public_url_and_redirects_once() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = EnvGuard::new(&[
        "HOME",
        "CORTEX_AUTH_MODE",
        "CLAUDE_PLUGIN_OPTION_SERVER_URL",
        "CORTEX_PUBLIC_URL",
        "CORTEX_AUTH_ALLOWED_REDIRECT_URIS",
        "CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH",
    ]);
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::create_dir(&codex).unwrap();
    std::fs::write(
        codex.join("config.toml"),
        "ignored = \"nope\"\nmcp_oauth_callback_url = \"http://127.0.0.1:1455/callback\"\n",
    )
    .unwrap();
    env.set("HOME", temp.path().to_str().unwrap());
    env.set("CORTEX_AUTH_MODE", "oauth");
    env.set(
        "CLAUDE_PLUGIN_OPTION_SERVER_URL",
        "https://cortex.example/mcp/",
    );
    env.set(
        "CORTEX_AUTH_ALLOWED_REDIRECT_URIS",
        "https://claude.ai/api/mcp/auth_callback",
    );

    let mut mapped = HashMap::new();
    prepare_oauth_env(&mut mapped).unwrap();
    prepare_oauth_env(&mut mapped).unwrap();

    assert_eq!(
        mapped.get("CORTEX_PUBLIC_URL").map(String::as_str),
        Some("https://cortex.example")
    );
    let redirects = mapped.get("CORTEX_AUTH_ALLOWED_REDIRECT_URIS").unwrap();
    assert_eq!(
        redirects
            .matches("https://claude.ai/api/mcp/auth_callback")
            .count(),
        1
    );
    assert!(redirects.contains("https://claudeai.ai/api/mcp/auth_callback"));
    assert!(redirects.contains("http://127.0.0.1:1455/callback"));
    assert_eq!(
        mapped
            .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
            .map(String::as_str),
        Some("false")
    );
}

#[test]
#[serial]
fn prepare_plugin_hook_env_rejects_unsafe_api_token_before_mapping() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = EnvGuard::new(&[
        "CLAUDE_PLUGIN_OPTION_API_TOKEN",
        "CORTEX_TOKEN",
        "CORTEX_AUTH_ALLOWED_REDIRECT_URIS",
    ]);
    env.set("CLAUDE_PLUGIN_OPTION_API_TOKEN", "token\nwith-newline");

    let err = match prepare_plugin_hook_env() {
        Ok(_) => panic!("expected unsafe token to fail"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("CLAUDE_PLUGIN_OPTION_API_TOKEN"));
    assert!(crate::env::var("CORTEX_TOKEN").is_err());
}
