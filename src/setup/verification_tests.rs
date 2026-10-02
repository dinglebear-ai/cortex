use super::*;
use axum::{
    Router,
    extract::Json,
    routing::{get, post},
};

#[test]
fn normalize_rejects_credentials_and_preserves_proxy_path() {
    assert_eq!(
        normalize_server_url("https://example.test/cortex/mcp/").unwrap(),
        "https://example.test/cortex"
    );
    for url in [
        "ftp://example.test",
        "https://user:secret@example.test",
        "https://example.test?token=x",
        "localhost:3100",
    ] {
        assert!(normalize_server_url(url).is_err());
    }
}

async fn fixture(auth: bool) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/api/stats",
            get(move |headers: axum::http::HeaderMap| async move {
                if auth
                    && headers
                        .get("Authorization")
                        .is_none_or(|h| h != "Bearer rest")
                {
                    return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})));
                }
                (axum::http::StatusCode::OK, Json(json!({"total":0})))
            }),
        )
        .route(
            "/mcp",
            post(
                move |headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                    if auth
                        && headers
                            .get("Authorization")
                            .is_none_or(|h| h != "Bearer machine")
                    {
                        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})));
                    }
                    let result = if body["method"] == "initialize" {
                        json!({"serverInfo":{"name":"cortex"}})
                    } else {
                        json!({"content":[{"type":"text","text":"status"}]})
                    };
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result})),
                    )
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}
#[tokio::test]
async fn healthy_endpoint_cannot_hide_bad_credentials() {
    let (url, task) = fixture(true).await;
    let phases = verify_connection(
        &url,
        Some("wrong"),
        Some("wrong"),
        false,
        Duration::from_secs(1),
    )
    .await;
    assert!(
        phases
            .iter()
            .any(|p| p.name == "health" && p.status == SetupStatus::Ok)
    );
    assert!(
        phases
            .iter()
            .any(|p| p.name == "mcp-auth" && p.status == SetupStatus::Error)
    );
    assert!(
        phases
            .iter()
            .any(|p| p.name == "rest-auth" && p.status == SetupStatus::Error)
    );
    assert!(!serde_json::to_string(&phases).unwrap().contains("wrong"));
    task.abort();
}
#[tokio::test]
async fn validates_both_authenticated_surfaces() {
    let (url, task) = fixture(true).await;
    let phases = verify_connection(
        &url,
        Some("machine"),
        Some("rest"),
        false,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(phases.len(), 3);
    assert!(phases.iter().all(|p| p.status == SetupStatus::Ok));
    task.abort();
}
#[tokio::test]
async fn rest_only_verification_never_requests_mcp() {
    let (url, task) = fixture(true).await;
    let phases = verify_requested_connection(
        &url,
        None,
        Some("rest"),
        false,
        Duration::from_secs(1),
        false,
    )
    .await;
    assert_eq!(phases.len(), 2);
    assert!(phases.iter().all(|p| p.status == SetupStatus::Ok));
    assert!(!phases.iter().any(|p| p.name == "mcp-auth"));
    task.abort();
}

#[test]
fn overlay_consent_uses_runtime_boolean_spellings() {
    for value in ["true", "TRUE", "1", "yes", "on"] {
        let _overlay = crate::config::PluginEnvGuard::install(
            [(
                "CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP".into(),
                value.into(),
            )]
            .into(),
        );
        let consent = super::super::auth_policy::enabled(
            crate::config::config_env_var("CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP").as_deref(),
        );
        assert!(validate_credential_transport("http://100.120.1.2:3100", consent).is_ok());
    }
    for value in ["false", "0", "off"] {
        assert!(
            validate_credential_transport(
                "http://100.120.1.2:3100",
                super::super::auth_policy::enabled(Some(value))
            )
            .is_err()
        );
    }
}
#[tokio::test]
async fn oauth_is_pending_until_client_login_not_falsely_verified() {
    let (url, task) = fixture(true).await;
    let phases = verify_connection(&url, None, Some("rest"), true, Duration::from_secs(1)).await;
    assert!(
        phases
            .iter()
            .any(|p| p.name == "mcp-auth" && p.status == SetupStatus::Warn)
    );
    task.abort();
}

#[test]
fn plaintext_credentials_require_explicit_overlay_consent() {
    assert!(validate_credential_transport("http://100.120.1.2:3100", false).is_err());
    assert!(validate_credential_transport("http://100.120.1.2:3100", true).is_ok());
    assert!(validate_credential_transport("http://127.0.0.1:3100", false).is_ok());
    assert!(validate_credential_transport("https://example.test", false).is_ok());
}

#[test]
fn fresh_heartbeat_ack_rejects_empty_or_duplicate_receipts() {
    assert!(!fresh_heartbeat_ack(&json!({})));
    assert!(!fresh_heartbeat_ack(
        &json!({"accepted":0,"heartbeat_id":1,"received_at":"2026-10-02T00:00:00Z"})
    ));
    assert!(!fresh_heartbeat_ack(
        &json!({"accepted":1,"heartbeat_id":1,"received_at":"invalid"})
    ));
    assert!(fresh_heartbeat_ack(
        &json!({"accepted":1,"heartbeat_id":1,"received_at":"2026-10-02T00:00:00Z"})
    ));
}
