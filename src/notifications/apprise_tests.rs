use super::*;

#[test]
fn escape_replaces_at_sign() {
    assert_eq!(
        escape_for_notification("user@example.com"),
        "user＠example.com"
    );
}

#[test]
fn escape_neutralizes_angle_brackets_without_dropping_content() {
    // Markup is escaped (not executable) but the inner text is preserved,
    // so HTML/markdown targets cannot be injected yet nothing is silently lost.
    assert_eq!(
        escape_for_notification("<script>alert(1)</script>"),
        "&lt;script&gt;alert(1)&lt;/script&gt;"
    );
}

#[test]
fn escape_preserves_normalizer_placeholders() {
    // Regression: signatures emit <n>, <hex>, <path> etc. Stripping the
    // brackets collapsed them into unreadable runs; escaping keeps them.
    assert_eq!(
        escape_for_notification("<n>-<n>-<n>T<n>:<n>:<n> path=<path>"),
        "&lt;n&gt;-&lt;n&gt;-&lt;n&gt;T&lt;n&gt;:&lt;n&gt;:&lt;n&gt; path=&lt;path&gt;"
    );
}

#[test]
fn escape_combined() {
    assert_eq!(
        escape_for_notification("Out of memory: Killed process 1234 (nginx) <@root>"),
        "Out of memory: Killed process 1234 (nginx) &lt;＠root&gt;"
    );
}

#[test]
fn escape_ampersand() {
    assert_eq!(escape_for_notification("a & b"), "a &amp; b");
}

#[test]
fn escape_clean_string() {
    let clean = "normal log message without special chars";
    assert_eq!(escape_for_notification(clean), clean);
}

/// Test AppriseClient against a mock axum server.
#[tokio::test]
async fn mock_server_200() {
    let (client, _server) = make_mock_server(axum::http::StatusCode::OK).await;
    let result = client
        .notify(&["test://".to_string()], "Test", "Body", NotifyType::Info)
        .await;
    assert_eq!(result.unwrap().status_code, 200);
}

#[tokio::test]
async fn mock_server_207_partial_success() {
    let (client, _server) = make_mock_server(axum::http::StatusCode::MULTI_STATUS).await;
    let result = client
        .notify(
            &["test://".to_string()],
            "Test",
            "Body",
            NotifyType::Warning,
        )
        .await;
    assert_eq!(
        result
            .expect("207 should be treated as success")
            .status_code,
        207
    );
}

#[tokio::test]
async fn mock_server_204_permanent() {
    // 204 No Content means Apprise had no targets / nothing was sent.
    // It must NOT be treated as a success — it is a permanent failure.
    let (client, _server) = make_mock_server(axum::http::StatusCode::NO_CONTENT).await;
    let result = client
        .notify(&["test://".to_string()], "Test", "Body", NotifyType::Info)
        .await;
    assert!(
        result.is_err(),
        "204 should be treated as permanent failure"
    );
    match result.unwrap_err() {
        AppriseError::Permanent { code, .. } => assert_eq!(code, 204),
        other => panic!("expected Permanent, got {other}"),
    }
}

#[tokio::test]
async fn mock_server_400_permanent() {
    let (client, _server) = make_mock_server(axum::http::StatusCode::BAD_REQUEST).await;
    let result = client
        .notify(&["test://".to_string()], "Test", "Body", NotifyType::Info)
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        AppriseError::Permanent { code, .. } => assert_eq!(code, 400),
        other => panic!("expected Permanent, got {other}"),
    }
}

#[tokio::test]
async fn mock_server_500_transient() {
    let (client, _server) = make_mock_server(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
    let result = client
        .notify(&["test://".to_string()], "Test", "Body", NotifyType::Info)
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        AppriseError::Transient(_) => {}
        other => panic!("expected Transient, got {other}"),
    }
}

#[tokio::test]
async fn mock_server_timeout() {
    use tokio::net::TcpListener;

    // Bind but never accept — causes timeout
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    // Keep the listener alive so the port exists but no response comes
    let _listener = listener;

    let client = AppriseClient::new(base_url).with_timeout(Duration::from_millis(50));
    let result = client
        .notify(&["test://".to_string()], "Test", "Body", NotifyType::Info)
        .await;
    assert!(result.is_err());
    // Could be Timeout or Transient depending on OS behavior
    match result.unwrap_err() {
        AppriseError::Timeout | AppriseError::Transient(_) => {}
        other => panic!("expected Timeout or Transient, got {other}"),
    }
}

// Helper: spin up an axum server that always responds with `status_code`.
async fn make_mock_server(
    status_code: axum::http::StatusCode,
) -> (AppriseClient, tokio::task::JoinHandle<()>) {
    use axum::{Router, routing::post};
    use tokio::net::TcpListener;

    let app = Router::new().route("/notify/", post(move || async move { status_code }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    let client = AppriseClient::new(format!("http://{addr}")).with_timeout(Duration::from_secs(2));
    (client, handle)
}
