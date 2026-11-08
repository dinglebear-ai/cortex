use super::*;

const COLLECTOR_LOG: &str = r#"2026-11-07T18:32:27.504Z	error	internal/queue_sender.go:62	Exporting failed. Dropping data.	{"resource":{"service.instance.id":"private-instance","service.name":"otelcol"},"otelcol.component.id":"otlp_http/cortex","otelcol.signal":"metrics","error":"Post http://127.0.0.1:3100/v1/metrics: connection refused","dropped_items":32}"#;

async fn delivered_payload(body: &str) -> serde_json::Value {
    use axum::{Json, Router, routing::post};
    use tokio::net::TcpListener;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let app = Router::new().route(
        "/notify/",
        post(move |Json(payload): Json<serde_json::Value>| {
            let sender = sender.clone();
            async move {
                sender.send(payload).await.unwrap();
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    AppriseClient::new(format!("http://{address}"))
        .notify(
            &["test://".to_string()],
            "⚠️ Cortex alert",
            body,
            NotifyType::Warning,
        )
        .await
        .unwrap();
    let payload = receiver.recv().await.unwrap();
    server.abort();
    let _ = server.await;
    payload
}

#[tokio::test]
async fn delivery_turns_collector_json_into_readable_text() {
    let payload = delivered_payload(COLLECTOR_LOG).await;
    assert_eq!(payload["format"], "text");
    let body = payload["body"].as_str().unwrap();
    assert!(body.contains("Exporting failed. Dropping data."));
    assert!(body.contains("Cause: Post http://127.0.0.1:3100/v1/metrics: connection refused"));
    assert!(body.contains("Dropped items: 32"));
    assert!(!body.contains("service.instance.id"));
    assert!(!body.contains("queue_sender.go"));
    assert!(!body.contains('{') && !body.contains('}'));
}

#[tokio::test]
async fn delivery_reformats_queued_legacy_recurring_alerts() {
    let payload = delivered_payload(&format!(
        "Signature: <n>-<n>-<n>T<n>:<n> {{\"resource\":\"normalized\"}}\nSample: {COLLECTOR_LOG}\nOccurrences: 6"
    )).await;
    let body = payload["body"].as_str().unwrap();
    assert!(body.contains("Problem: Exporting failed. Dropping data."));
    assert!(body.contains("Occurrences: 6"));
    assert!(!body.contains("Signature:") && !body.contains("<n>"));
    assert!(!body.contains("queue_sender.go"));
}

#[tokio::test]
async fn delivery_formats_nested_json_and_json_encoded_strings() {
    let payload = delivered_payload(r#"{"message":"Export failed","error":"{\"reason\":\"connection refused\"}","attempts":[1,2],"password":"do-not-deliver"}"#).await;
    let body = payload["body"].as_str().unwrap();
    assert!(body.contains("Export failed"));
    assert!(body.contains("connection refused"));
    assert!(
        !body.contains("Cause: Cause:"),
        "Nested error fields need one readable label"
    );
    assert!(!body.contains('{') && !body.contains('['));
    assert!(!body.contains("do-not-deliver"));
}

#[tokio::test]
async fn delivery_hides_malformed_structured_payloads() {
    for message in [
        "Export failed {\"resource\":{\"service.instance.id\":\"unfinished",
        "Export failed [\"unfinished",
    ] {
        let payload = delivered_payload(message).await;
        let body = payload["body"].as_str().unwrap();
        assert!(body.contains("Export failed"));
        assert!(body.contains("Cortex"));
        assert!(!body.contains('{') && !body.contains('['));
        assert!(!body.contains("service.instance.id"));
    }
}

#[tokio::test]
async fn delivery_formats_or_omits_entire_multiline_structured_payloads() {
    for (message, complete) in [
        (
            "Host: alpha\nProblem: {\n  \"message\": \"failed\",\n  \"password\": \"private-value\"\n}",
            true,
        ),
        (
            "Host: alpha\nProblem: {\n  \"message\": \"failed\",\n  \"password\": \"private-value\"",
            false,
        ),
        (
            "Signature: <n>\nSample: {\n  \"message\": \"failed\",\n  \"password\": \"private-value\"\n}\nOccurrences: 6",
            true,
        ),
    ] {
        let payload = delivered_payload(message).await;
        let body = payload["body"].as_str().unwrap();
        if complete {
            assert!(body.contains("failed"));
        } else {
            assert!(body.contains("Additional details are available in Cortex."));
        }
        assert!(
            !body.contains("\"message\"")
                && !body.contains("password")
                && !body.contains("private-value")
        );
        assert!(!body.contains('{') && !body.contains('}'));
        assert!(!body.contains("Problem: Problem:"), "Use a single label");
    }
}

#[tokio::test]
async fn delivery_formats_inline_boolean_arrays_without_altering_ipv6_or_severity() {
    let payload = delivered_payload("[WARNING] http://[::1]:3100 Flags: [true,false,null]").await;
    let body = payload["body"].as_str().unwrap();
    assert!(body.contains("[WARNING] http://[::1]:3100"));
    assert!(body.contains("true") && body.contains("false"));
    assert!(!body.contains("[true"));
}

#[tokio::test]
async fn delivery_preserves_bracketed_ipv6_destinations_and_the_error_cause() {
    let failure = "Post http://[2001:db8::1]:4318/v1/metrics: connection refused";
    for message in [
        failure.to_string(),
        serde_json::json!({ "error": failure }).to_string(),
    ] {
        let payload = delivered_payload(&message).await;
        let body = payload["body"].as_str().unwrap();
        assert!(
            body.contains(failure),
            "IPv6 destination and cause lost: {body}"
        );
        assert!(!body.contains("Additional details are available in Cortex."));
    }
}

#[tokio::test]
async fn delivery_does_not_expose_json_encoded_keys_or_escaped_fragments() {
    for input in [
        r#"{"{\"message\":\"encoded\"}":"failure"}"#,
        r#"Export failed {\"error\":\"connection refused\"}"#,
    ] {
        let payload = delivered_payload(input).await;
        let body = payload["body"].as_str().unwrap();
        assert!(!body.contains('{') && !body.contains('}'));
    }
}

#[tokio::test]
async fn delivery_keeps_text_literal_without_double_encoding() {
    let payload = delivered_payload("Host &amp;lt;nas&amp;gt; &amp;amp; admin@example.com").await;
    assert_eq!(payload["format"], "text");
    assert_eq!(payload["body"], "Host <nas> & admin＠example.com");
}

#[tokio::test]
async fn delivery_bounds_unicode_and_omits_large_payload_tail() {
    let body = format!("{}{{\"resource\":\"hidden\"}}", "🐙".repeat(3000));
    let payload = delivered_payload(&body).await;
    let text = payload["body"].as_str().unwrap();
    assert!(text.chars().count() <= 1200);
    assert!(text.ends_with('…'));
    assert!(!text.contains('{'));
}

#[test]
fn escape_replaces_at_sign() {
    assert_eq!(
        escape_for_notification("user@example.com"),
        "user＠example.com"
    );
}

#[test]
fn text_preserves_angle_brackets_for_literal_delivery() {
    assert_eq!(
        escape_for_notification("<script>alert(1)</script>"),
        "<script>alert(1)</script>"
    );
}

#[test]
fn escape_preserves_normalizer_placeholders() {
    // Text delivery preserves these as literal characters without entities.
    assert_eq!(
        escape_for_notification("<n>-<n>-<n>T<n>:<n>:<n> path=<path>"),
        "<n>-<n>-<n>T<n>:<n>:<n> path=<path>"
    );
}

#[test]
fn escape_combined() {
    assert_eq!(
        escape_for_notification("Out of memory: Killed process 1234 (nginx) <@root>"),
        "Out of memory: Killed process 1234 (nginx) <＠root>"
    );
}

#[test]
fn escape_ampersand() {
    assert_eq!(escape_for_notification("a & b"), "a & b");
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
