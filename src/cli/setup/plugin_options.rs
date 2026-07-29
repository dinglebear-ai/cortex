//! Plugin-hook environment preparation, ported from
//! `plugins/cortex/scripts/plugin-setup.sh`.
//!
//! Operators invoke `cortex setup pluginhook` directly (no bash wrapper, and no
//! Claude Code lifecycle hook). Before the check/repair phases run, we adapt the
//! Claude Code plugin `CLAUDE_PLUGIN_OPTION_*` settings into the `CORTEX_*`
//! environment the binary reads, mirroring the bash script exactly:
//!
//! 1. Map ~26 plugin options to `CORTEX_*` env vars (rejecting newline/CR
//!    values with a hard error, like the script's `exit 2`).
//! 2. Prepare OAuth env (public URL derivation + redirect URI assembly).
//! 3. For client installs (`IS_SERVER != "true"`), validate connectivity to the
//!    remote server's `/health` and return early — no local server setup.

use anyhow::{Result, bail};
use std::time::Duration;

/// Outcome of the plugin-option preparation step.
pub(super) enum HookPrep {
    /// This install is a server: continue to the local check/repair phases.
    Server,
    /// This install is a client: connectivity was validated; return early.
    Client,
}

/// Apply the env mappings + OAuth prep + is_server branch.
///
/// Returns `HookPrep::Client` when the caller should print/return without
/// running the local server check+repair (the client validation path), or
/// `HookPrep::Server` when the caller should proceed with setup.
pub(super) fn prepare_plugin_hook_env() -> Result<HookPrep> {
    // is_server defaults to "true" — only an explicit non-"true" value takes the
    // client path.
    let is_server = std::env::var("CLAUDE_PLUGIN_OPTION_IS_SERVER")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "true".to_string());

    // The script rejects an unsafe API token up-front (before mapping), then the
    // export_if_set mapping rejects each value it touches.
    reject_unsafe_value(
        "CLAUDE_PLUGIN_OPTION_API_TOKEN",
        &option_value("CLAUDE_PLUGIN_OPTION_API_TOKEN"),
    )?;

    apply_plugin_options()?;
    prepare_oauth_env()?;

    if is_server != "true" {
        validate_client();
        return Ok(HookPrep::Client);
    }

    Ok(HookPrep::Server)
}

/// `CLAUDE_PLUGIN_OPTION_* -> CORTEX_*` env mapping, mirroring the script's
/// `export_if_set` calls one-for-one and in order.
fn apply_plugin_options() -> Result<()> {
    // (target CORTEX env var, source CLAUDE_PLUGIN_OPTION_* var)
    const MAPPINGS: &[(&str, &str)] = &[
        ("CORTEX_TOKEN", "CLAUDE_PLUGIN_OPTION_API_TOKEN"),
        ("CORTEX_SERVER_URL", "CLAUDE_PLUGIN_OPTION_SERVER_URL"),
        ("CORTEX_AUTH_MODE", "CLAUDE_PLUGIN_OPTION_AUTH_MODE"),
        ("CORTEX_PUBLIC_URL", "CLAUDE_PLUGIN_OPTION_PUBLIC_URL"),
        (
            "CORTEX_GOOGLE_CLIENT_ID",
            "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_ID",
        ),
        (
            "CORTEX_GOOGLE_CLIENT_SECRET",
            "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_SECRET",
        ),
        (
            "CORTEX_AUTH_ADMIN_EMAIL",
            "CLAUDE_PLUGIN_OPTION_AUTH_ADMIN_EMAIL",
        ),
        (
            "CORTEX_AUTH_ALLOWED_REDIRECT_URIS",
            "CLAUDE_PLUGIN_OPTION_AUTH_ALLOWED_REDIRECT_URIS",
        ),
        (
            "CORTEX_RECEIVER_HOST",
            "CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_HOST",
        ),
        (
            "CORTEX_RECEIVER_PORT",
            "CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_PORT",
        ),
        (
            "CORTEX_RECEIVER_HOST_PORT",
            "CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_HOST_PORT",
        ),
        ("CORTEX_HOST", "CLAUDE_PLUGIN_OPTION_MCP_HOST"),
        ("CORTEX_PORT", "CLAUDE_PLUGIN_OPTION_MCP_PORT"),
        (
            "CORTEX_MAX_DB_SIZE_MB",
            "CLAUDE_PLUGIN_OPTION_MAX_DB_SIZE_MB",
        ),
        ("CORTEX_DATA_VOLUME", "CLAUDE_PLUGIN_OPTION_DATA_DIR"),
        (
            "CORTEX_RETENTION_DAYS",
            "CLAUDE_PLUGIN_OPTION_RETENTION_DAYS",
        ),
        ("CORTEX_BATCH_SIZE", "CLAUDE_PLUGIN_OPTION_BATCH_SIZE"),
        (
            "CORTEX_WRITE_CHANNEL_CAPACITY",
            "CLAUDE_PLUGIN_OPTION_WRITE_CHANNEL_CAPACITY",
        ),
        (
            "CORTEX_DOCKER_INGEST_ENABLED",
            "CLAUDE_PLUGIN_OPTION_DOCKER_INGEST_ENABLED",
        ),
        ("CORTEX_DOCKER_HOSTS", "CLAUDE_PLUGIN_OPTION_FLEET_HOSTS"),
        ("NO_AUTH", "CLAUDE_PLUGIN_OPTION_NO_AUTH"),
    ];

    for (env_name, option_name) in MAPPINGS {
        export_if_set(env_name, option_name)?;
    }
    Ok(())
}

/// Mirror of bash `export_if_set`: read the source option, reject unsafe values
/// (hard error), and set the target env var only when the value is non-empty.
fn export_if_set(env_name: &str, option_name: &str) -> Result<()> {
    let value = option_value(option_name);
    reject_unsafe_value(option_name, &value)?;
    if value.is_empty() {
        return Ok(());
    }
    // SAFETY (edition 2021): set_var is safe here; this runs early in the
    // plugin-hook path before any threads read these vars.
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var(env_name, value) };
    Ok(())
}

/// Mirror of bash `reject_unsafe_value`: abort (the script `exit 2`s) if the
/// value contains a newline or carriage return.
fn reject_unsafe_value(name: &str, value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        bail!("cortex plugin setup: {name} must not contain newlines");
    }
    Ok(())
}

/// Read an env var, returning "" when unset (matches bash `printenv ... || true`
/// followed by emptiness checks).
fn option_value(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// Mirror of bash `strip_trailing_mcp_path`: drop a single trailing `/`, then a
/// trailing `/mcp` segment.
fn strip_trailing_mcp_path(url: &str) -> String {
    let url = url.strip_suffix('/').unwrap_or(url);
    url.strip_suffix("/mcp").unwrap_or(url).to_string()
}

/// Mirror of bash `append_csv_unique`: append `value` to a comma-separated list
/// unless empty or already present (whitespace-trimmed comparison per item).
fn append_csv_unique(csv: &str, value: &str) -> String {
    if value.is_empty() {
        return csv.to_string();
    }
    for item in csv.split(',') {
        if item.trim() == value {
            return csv.to_string();
        }
    }
    if csv.is_empty() {
        value.to_string()
    } else {
        format!("{csv},{value}")
    }
}

/// Mirror of bash `prepare_oauth_env`. Runs only when `CORTEX_AUTH_MODE` (the
/// already-mapped var) is `oauth`. Reads `server_url` from the RAW option
/// (`CLAUDE_PLUGIN_OPTION_SERVER_URL`), defaulting to `http://localhost:3100`.
fn prepare_oauth_env() -> Result<()> {
    let auth_mode = std::env::var("CORTEX_AUTH_MODE").unwrap_or_else(|_| "bearer".to_string());
    if auth_mode != "oauth" {
        return Ok(());
    }

    let server_url = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "http://localhost:3100".to_string());

    // Derive CORTEX_PUBLIC_URL when unset and the server is https.
    let public_unset = std::env::var("CORTEX_PUBLIC_URL")
        .map(|v| v.is_empty())
        .unwrap_or(true);
    if public_unset && server_url.starts_with("https://") {
        // SAFETY (edition 2021): early single-threaded plugin-hook path.
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("CORTEX_PUBLIC_URL", strip_trailing_mcp_path(&server_url)) };
    }

    let mut redirects = std::env::var("CORTEX_AUTH_ALLOWED_REDIRECT_URIS").unwrap_or_default();
    redirects = append_csv_unique(&redirects, "https://claude.ai/api/mcp/auth_callback");
    redirects = append_csv_unique(&redirects, "https://claudeai.ai/api/mcp/auth_callback");
    if let Some(codex_callback) = codex_oauth_callback_url()
        && !codex_callback.is_empty()
    {
        redirects = append_csv_unique(&redirects, &codex_callback);
    }
    // SAFETY (edition 2021): early single-threaded plugin-hook path.
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("CORTEX_AUTH_ALLOWED_REDIRECT_URIS", redirects) };

    let disable_static = std::env::var("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "false".to_string());
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe {
        std::env::set_var(
            "CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH",
            disable_static,
        )
    };
    Ok(())
}

/// Mirror of bash `codex_oauth_callback_url`: flat line-scan of
/// `~/.codex/config.toml` for the first line whose trimmed key is
/// `mcp_oauth_callback_url`, returning its quote-stripped value.
fn codex_oauth_callback_url() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let config = std::path::PathBuf::from(home)
        .join(".codex")
        .join("config.toml");
    let contents = std::fs::read_to_string(&config).ok()?;
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "mcp_oauth_callback_url" {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(value);
        return Some(value.to_string());
    }
    None
}

/// Mirror of bash `validate_client`: GET `${server_url%/mcp}/health` with a 2s
/// connect / 5s total timeout and print a connected/warning line. Reads the RAW
/// option `CLAUDE_PLUGIN_OPTION_SERVER_URL`, defaulting to
/// `http://localhost:3100`.
///
/// `run_plugin_hook` is a sync fn called from within `#[tokio::main]`, so we
/// cannot `block_on` the ambient multi-thread runtime (it would panic), and the
/// `reqwest` blocking feature isn't enabled. Run the async GET on a dedicated
/// thread with its own current-thread runtime.
fn validate_client() {
    let server_url = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "http://localhost:3100".to_string());
    let base = strip_trailing_mcp_path(&server_url);
    let health_url = format!("{base}/health");

    let reachable = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => return false,
                };
                runtime.block_on(async {
                    let client = match reqwest::Client::builder()
                        .connect_timeout(Duration::from_secs(2))
                        .timeout(Duration::from_secs(5))
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return false,
                    };
                    match client.get(&health_url).send().await {
                        Ok(resp) => resp.status().is_success(),
                        Err(_) => false,
                    }
                })
            })
            .join()
            .unwrap_or(false)
    });

    if reachable {
        println!("cortex: connected to {base}");
    } else {
        eprintln!("WARNING: cortex server at {base} is not reachable");
    }
}

#[cfg(test)]
#[path = "plugin_options_tests.rs"]
mod tests;
