//! Bounded readiness and authenticated, read-only connection verification.
use super::{PhaseTimer, SetupPhase, SetupStatus};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub fn normalize_server_url(raw: &str) -> Result<String, String> {
    let base = raw
        .trim()
        .trim_end_matches('/')
        .strip_suffix("/mcp")
        .unwrap_or(raw.trim().trim_end_matches('/'));
    let url = url::Url::parse(base).map_err(|_| "server must be an absolute HTTP(S) URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("server URL must use HTTP(S) without credentials, query, or fragment".into());
    }
    Ok(base.to_string())
}

pub async fn wait_ready(
    client: &reqwest::Client,
    base: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("readiness deadline exceeded; inspect Compose logs for startup or migration progress".into());
        }
        let ready = client
            .get(format!("{base}/health"))
            .timeout(remaining.min(Duration::from_secs(5)))
            .send()
            .await;
        if ready.is_ok_and(|r| r.status().is_success()) {
            return Ok(());
        }
        tokio::time::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(1)),
        )
        .await;
    }
}

async fn rpc(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    body: Value,
) -> Result<Value, String> {
    let mut request = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "MCP request failed; check address/TLS/network")?;
    if !response.status().is_success() {
        return Err(format!(
            "MCP returned HTTP {}; check credentials and auth mode",
            response.status().as_u16()
        ));
    }
    let text = response
        .text()
        .await
        .map_err(|_| "failed reading MCP response")?;
    let value: Value = serde_json::from_str(&text)
        .or_else(|_| {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
                .find(|v| v.get("id") == body.get("id"))
                .ok_or_else(|| serde_json::from_str::<Value>("").unwrap_err())
        })
        .map_err(|_| "MCP response was not valid JSON-RPC")?;
    if value.get("id") != body.get("id") || value.get("error").is_some() {
        return Err("MCP returned an invalid or failed JSON-RPC result".into());
    }
    Ok(value)
}

/// Verify protocol initialization and a real authenticated Cortex action.
pub async fn verify_connection(
    base: &str,
    mcp_token: Option<&str>,
    api_token: Option<&str>,
    oauth: bool,
    timeout: Duration,
) -> Vec<SetupPhase> {
    verify_requested_connection(base, mcp_token, api_token, oauth, timeout, true).await
}

/// Check the requested surfaces; REST-only clients do not require an MCP token.
pub(crate) async fn verify_requested_connection(
    base: &str,
    mcp_token: Option<&str>,
    api_token: Option<&str>,
    oauth: bool,
    timeout: Duration,
    verify_mcp: bool,
) -> Vec<SetupPhase> {
    let mut phases = Vec::new();
    let base = match normalize_server_url(base) {
        Ok(v) => v,
        Err(e) => return vec![PhaseTimer::start("connection-url").finish(SetupStatus::Error, e)],
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return vec![
                PhaseTimer::start("connection-client")
                    .finish(SetupStatus::Error, "could not construct HTTP client"),
            ];
        }
    };
    let timer = PhaseTimer::start("health");
    match wait_ready(&client, &base, timeout).await {
        Ok(()) => phases.push(timer.finish(SetupStatus::Ok, "HTTP and started listeners ready")),
        Err(e) => {
            phases.push(timer.finish(SetupStatus::Error, e));
            return phases;
        }
    }
    if (mcp_token.is_some() || api_token.is_some())
        && let Err(e) = validate_credential_transport(
            &base,
            super::auth_policy::enabled(
                crate::config::config_env_var("CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP").as_deref(),
            ),
        )
    {
        phases.push(PhaseTimer::start("credential-transport").finish(SetupStatus::Error, e));
        return phases;
    }
    if verify_mcp {
        let timer = PhaseTimer::start("mcp-auth");
        if oauth && mcp_token.is_none() {
            phases.push(timer.finish(SetupStatus::Warn, "needs_configuration: complete OAuth login in the selected MCP client, then verify status from a fresh session"));
        } else {
            let result = async {
            let initialized = rpc(&client, &base, mcp_token, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cortex-setup","version":env!("CARGO_PKG_VERSION")}}})).await?;
            if initialized.pointer("/result/serverInfo").is_none() { return Err("MCP initialization did not return server information".into()); }
            let status = rpc(&client, &base, mcp_token, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cortex","arguments":{"action":"status"}}})).await?;
            if status.pointer("/result/isError").and_then(Value::as_bool) == Some(true) || status.pointer("/result/content").is_none() { return Err("Cortex status action failed or returned no content".into()); }
            Ok::<_, String>(())
        }.await;
            phases.push(match result {
                Ok(()) => timer.finish(
                    SetupStatus::Ok,
                    "MCP initialization and read-only status succeeded",
                ),
                Err(e) => timer.finish(SetupStatus::Error, e),
            });
        }
    }
    if let Some(token) = api_token {
        let timer = PhaseTimer::start("rest-auth");
        let result = client
            .get(format!("{base}/api/stats"))
            .bearer_auth(token)
            .send()
            .await;
        phases.push(match result {
            Ok(r)
                if r.status().is_success()
                    && r.headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .is_some_and(|v| v.to_str().unwrap_or("").contains("application/json")) =>
            {
                if r.json::<Value>().await.is_ok_and(|value| value.is_object()) {
                    timer.finish(SetupStatus::Ok, "authenticated REST stats succeeded")
                } else {
                    timer.finish(SetupStatus::Error, "REST stats returned malformed JSON")
                }
            }
            Ok(r) => timer.finish(
                SetupStatus::Error,
                format!(
                    "REST returned HTTP {} or a non-JSON response; check REST credentials/proxy",
                    r.status().as_u16()
                ),
            ),
            Err(_) => timer.finish(SetupStatus::Error, "REST request failed; check TLS/network"),
        });
    }
    phases
}

pub async fn verify_managed_server(env: &BTreeMap<String, String>) -> Vec<SetupPhase> {
    let port = env.get("CORTEX_PORT").map(String::as_str).unwrap_or("3100");
    let oauth = env.get("CORTEX_AUTH_MODE").is_some_and(|v| v == "oauth");
    let disable_static = env
        .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
        .is_none_or(|v| v != "false");
    let mcp_token = if oauth && disable_static {
        None
    } else {
        env.get("CORTEX_TOKEN").map(String::as_str)
    };
    let timeout = env
        .get("CORTEX_SETUP_READY_TIMEOUT_SECS")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300)
        .clamp(1, 3600);
    verify_connection(
        &format!("http://127.0.0.1:{port}"),
        mcp_token,
        env.get("CORTEX_API_TOKEN").map(String::as_str),
        oauth,
        Duration::from_secs(timeout),
    )
    .await
}

/// Never transmit a bearer over non-loopback HTTP without explicit overlay consent.
pub fn validate_credential_transport(base: &str, allow_overlay_http: bool) -> Result<(), String> {
    let url = url::Url::parse(base).map_err(|_| "invalid server URL")?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if url.scheme() == "http" && !loopback && !allow_overlay_http {
        return Err("non-loopback plaintext credentials require explicit CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP=true for a trusted private overlay; prefer HTTPS".into());
    }
    Ok(())
}

/// Read-only ingest credential preflight, before writing an agent configuration.
pub async fn verify_agent_auth(
    base: &str,
    token: Option<&str>,
    allow_overlay_http: bool,
) -> SetupPhase {
    let timer = PhaseTimer::start("agent-auth");
    if let Err(e) = validate_credential_transport(base, allow_overlay_http) {
        return timer.finish(SetupStatus::Error, e);
    }
    let result = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "HTTP client initialization failed")?;
        let mut request = client.get(format!("{base}/v1/agent/release?version=setup-auth-probe"));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "ingest authentication preflight could not reach server")?;
        if response.status() != reqwest::StatusCode::BAD_REQUEST {
            return Err(
                "ingest authentication preflight failed; check ingest token and proxy routing",
            );
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| "ingest preflight returned invalid JSON")?;
        if body["error"] != "version_mismatch" || !body["server_version"].is_string() {
            return Err("ingest preflight response is not a Cortex version response");
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => timer.finish(SetupStatus::Ok, "ingest credential accepted by Cortex"),
        Err(e) => timer.finish(SetupStatus::Error, e),
    }
}

/// Send one real heartbeat and require a durable server acknowledgement. This
/// never starts optional forwarders or auto-update, and retains the stable host ID.
pub async fn verify_agent_delivery(
    base: &str,
    token: Option<&str>,
    host_id_path: &std::path::Path,
    allow_overlay_http: bool,
) -> SetupPhase {
    let timer = PhaseTimer::start("agent-delivery");
    if let Err(e) = validate_credential_transport(base, allow_overlay_http) {
        return timer.finish(SetupStatus::Error, e);
    }
    let result = async {
        let host_id = crate::heartbeat_agent::load_or_create_host_id(host_id_path).map_err(|_| "cannot persist stable host identity")?;
        let sampled_at = chrono::Utc::now().timestamp_millis();
        let payload = crate::heartbeat_agent::HeartbeatCollector::for_platform(std::env::consts::OS)
            .collect(host_id, sampled_at, Duration::from_secs(30), 0, Duration::from_secs(1), Duration::from_secs(3)).await;
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "HTTP client initialization failed")?;
        let mut request = client.post(format!("{base}/v1/heartbeats")).json(&payload);
        if let Some(token) = token { request = request.bearer_auth(token); }
        let response = request.send().await.map_err(|_| "heartbeat delivery could not reach server")?;
        if response.status() != reqwest::StatusCode::ACCEPTED { return Err("heartbeat was not durably acknowledged; check ingest token, storage and proxy routing"); }
        let ack: Value = response.json().await.map_err(|_| "heartbeat acknowledgement was not JSON")?;
        if !fresh_heartbeat_ack(&ack) { return Err("heartbeat acknowledgement did not confirm a fresh durable sample"); }
        Ok(())
    }.await;
    match result { Ok(()) => timer.finish(SetupStatus::Ok, "fresh heartbeat durably acknowledged; optional source delivery remains independently checked by heartbeatagent check"), Err(e) => timer.finish(SetupStatus::Error,e) }
}

fn fresh_heartbeat_ack(ack: &Value) -> bool {
    ack.get("accepted").and_then(Value::as_u64) == Some(1)
        && ack
            .get("heartbeat_id")
            .and_then(Value::as_i64)
            .is_some_and(|id| id > 0)
        && ack
            .get("received_at")
            .and_then(Value::as_str)
            .is_some_and(|time| chrono::DateTime::parse_from_rfc3339(time).is_ok())
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
