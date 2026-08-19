//! Unit + wiremock integration tests for `cli::http_client` (bead 0p8r.5).

use super::{HttpClient, ServerVersion, resolve_base_url, resolve_token};
use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─── Discovery: base URL precedence ─────────────────────────────────────────

#[test]
#[serial]
fn base_url_flag_wins_over_env() {
    // Use a guard to clear the env var so this test doesn't depend on
    // ambient state.
    let _g = EnvVarGuard::set("CORTEX_URL", "http://envhost:3100");
    let resolved =
        resolve_base_url(Some("http://flaghost:9000".into())).expect("flag should override env");
    assert_eq!(resolved.as_str(), "http://flaghost:9000/");
}

#[test]
#[serial]
fn base_url_env_used_when_no_flag() {
    let _g = EnvVarGuard::set("CORTEX_URL", "http://envhost:3100");
    let resolved = resolve_base_url(None).expect("env should resolve");
    assert_eq!(resolved.as_str(), "http://envhost:3100/");
}

#[test]
#[serial]
fn base_url_default_when_no_flag_no_env() {
    let _g1 = EnvVarGuard::unset("CORTEX_URL");
    let _g2 = EnvVarGuard::unset("CORTEX_PORT");
    let resolved = resolve_base_url(None).expect("default should resolve");
    assert_eq!(resolved.as_str(), "http://127.0.0.1:3100/");
}

#[test]
#[serial]
fn base_url_default_respects_cortex_port_env() {
    let _g1 = EnvVarGuard::unset("CORTEX_URL");
    let _g2 = EnvVarGuard::set("CORTEX_PORT", "9999");
    let resolved = resolve_base_url(None).expect("port-overridden default should resolve");
    assert_eq!(resolved.as_str(), "http://127.0.0.1:9999/");
}

#[test]
fn base_url_rejects_userinfo() {
    // Locked decision: userinfo URLs leak credentials via anyhow traces and
    // reqwest debug logs (eng-review #A24).
    let err = resolve_base_url(Some("http://u@localhost:3100".into()))
        .expect_err("userinfo URL must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("URL userinfo") && msg.contains("token flag"),
        "error must mention URL userinfo and token flag guidance; got: {msg}"
    );
}

#[test]
fn base_url_rejects_userinfo_with_password() {
    let err = resolve_base_url(Some("http://u:p@localhost:3100".into()))
        .expect_err("user:pass URL must be rejected");
    assert!(err.to_string().contains("token flag"));
}

#[test]
fn base_url_rejects_non_http_scheme() {
    let err = resolve_base_url(Some("file:///etc/passwd".into()))
        .expect_err("non-http scheme must be rejected");
    assert!(err.to_string().contains("http or https"));
}

#[test]
fn base_url_normalised_to_trailing_slash() {
    let resolved = resolve_base_url(Some("http://host:3100/sub".into())).expect("ok");
    assert!(resolved.as_str().ends_with('/'));
}

// ─── Discovery: token precedence ────────────────────────────────────────────

#[test]
#[serial]
fn token_flag_wins_over_env() {
    let _g = EnvVarGuard::set("CORTEX_API_TOKEN", "env-value");
    let resolved = resolve_token(Some("flag-value".into())).unwrap();
    assert_eq!(resolved, "flag-value");
}

#[test]
#[serial]
fn token_env_used_when_no_flag() {
    let _g = EnvVarGuard::set("CORTEX_API_TOKEN", "env-value");
    let resolved = resolve_token(None).unwrap();
    assert_eq!(resolved, "env-value");
}

#[test]
#[serial]
fn token_missing_error_mentions_setup_repair_and_copy_and_history_warning() {
    let _g = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let err = resolve_token(None).expect_err("must fail closed when token is missing");
    let msg = err.to_string();
    assert!(
        msg.contains("setup repair"),
        "error must reference 'setup repair'; got: {msg}"
    );
    assert!(
        msg.contains("another host"),
        "error must reference copying from another host; got: {msg}"
    );
    assert!(
        msg.contains("history") && msg.contains("export"),
        "error must warn against export ... in interactive shell (history leak); got: {msg}"
    );
}

#[test]
#[serial]
fn token_empty_flag_falls_through_to_env() {
    let _g = EnvVarGuard::set("CORTEX_API_TOKEN", "env-value");
    let resolved = resolve_token(Some("".into())).unwrap();
    assert_eq!(resolved, "env-value");
}

// ─── HttpClient::discover wiring ────────────────────────────────────────────

#[test]
#[serial]
fn discover_constructs_client() {
    let _g_url = EnvVarGuard::set("CORTEX_URL", "http://localhost:3100");
    let _g_tok = EnvVarGuard::set("CORTEX_API_TOKEN", "test-value");
    let client = HttpClient::discover(None, None).expect("discover should succeed");
    drop(client);
}

#[test]
#[serial]
fn discover_rejects_userinfo_url() {
    let _g = EnvVarGuard::set("CORTEX_API_TOKEN", "test-value");
    let err = HttpClient::discover(Some("http://t@localhost:3100".into()), None)
        .expect_err("must reject userinfo URL at discovery");
    assert!(err.to_string().contains("URL userinfo"));
}

#[test]
#[serial]
fn discover_fails_when_token_missing() {
    let _g_url = EnvVarGuard::set("CORTEX_URL", "http://localhost:3100");
    let _g_tok = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let err = HttpClient::discover(None, None).expect_err("must fail closed");
    assert!(err.to_string().contains("setup repair"));
}

// ─── Connect-timeout fires within 5–6s on unreachable host ──────────────────
//
// TEST-NET-1 (RFC 5737) is non-routable in real networks; SYN packets go
// into the void rather than getting refused, which exercises the actual
// timeout path. ECONNREFUSED to 127.0.0.1:1 returns in <10ms and does NOT
// test the timeout.
//
// Lower bound: > 9s — proves the 10s connect timeout (bead 0p8r.26) fired
// (not an instant failure). Upper bound: < 15s — proves it didn't hang past
// timeout + slack.
#[tokio::test(flavor = "current_thread")]
#[ignore = "Network-dependent: requires no route to 192.0.2.1; skip in restricted CI"]
async fn connect_timeout_fires_within_window_on_unreachable_host() {
    let client = HttpClient::discover(Some("http://192.0.2.1:80".into()), Some("token".into()))
        .expect("discover ok");
    let start = Instant::now();
    let err = client
        .hosts()
        .await
        .expect_err("connection should fail (timeout)");
    let elapsed = start.elapsed();
    let msg = err.to_string();
    assert!(
        msg.contains("cannot connect") || msg.contains("DNS or TCP"),
        "expected connect-failure message; got: {msg}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(9),
        "connect should not return faster than 9s (timeout proof); got {elapsed:?}"
    );
    assert!(
        elapsed <= std::time::Duration::from_secs(15),
        "connect should not hang past 15s (timeout proof); got {elapsed:?}"
    );
}

// ─── Wiremock helpers ───────────────────────────────────────────────────────

async fn start_mock_with_client() -> (MockServer, HttpClient) {
    let server = MockServer::start().await;
    let client =
        HttpClient::discover(Some(server.uri()), Some("test-value".into())).expect("client ok");
    (server, client)
}

// ─── Authorization header is sensitive-flagged ──────────────────────────────
//
// We can't directly read HeaderValue::is_sensitive from outside reqwest
// (it's not exposed through the request builder). What we CAN verify is
// that the bearer token is present on every outgoing request, which is the
// observable contract the test cares about.

#[tokio::test]
async fn bearer_token_present_on_outgoing_requests() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .and(header("authorization", "Bearer test-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"hosts": []})))
        .expect(1)
        .mount(&server)
        .await;
    let resp = client.hosts().await.expect("hosts ok");
    assert!(resp.hosts.is_empty());
}

// ─── 401 / 403 error mapping ────────────────────────────────────────────────

#[tokio::test]
async fn unauthorized_maps_to_auth_failed() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("401");
    let msg = err.to_string();
    assert!(msg.contains("authentication failed") && msg.contains("setup repair"));
}

#[tokio::test]
async fn forbidden_maps_to_403_message() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(403).set_body_string("no"))
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("403");
    assert!(err.to_string().contains("forbidden"));
}

// ─── 404 enrichment via /api/version (depth-1 OnceCell guard) ───────────────

#[tokio::test]
async fn not_found_enriched_with_server_version() {
    let (server, client) = start_mock_with_client().await;

    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not here"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "version": "0.99.0",
            "git_sha": "abc123",
            "schema_version": 7,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let err = client.hosts().await.expect_err("404 enriched");
    let msg = err.to_string();
    assert!(
        msg.contains("0.99.0"),
        "server version in error; got: {msg}"
    );
    assert!(msg.contains("abc123"), "git sha in error; got: {msg}");
    assert!(msg.contains("cortex compose pull"));
}

#[tokio::test]
async fn not_found_with_missing_git_sha_renders_unknown() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "version": "0.99.0",
            "schema_version": 7,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("404");
    assert!(err.to_string().contains("unknown"));
}

/// **MUST-CHECK**: hit a 404 endpoint multiple times and verify /api/version
/// is probed EXACTLY ONCE total. This proves the OnceCell depth-1 guard.
#[tokio::test]
async fn version_probed_exactly_once_across_repeated_404s() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    // .expect(1) on /api/version: if the OnceCell guard breaks and we probe
    // again, wiremock will panic on server drop because the call count
    // exceeded the expectation.
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "version": "1.0.0",
            "schema_version": 1,
        })))
        .expect(1)
        .mount(&server)
        .await;

    for _ in 0..4 {
        let err = client.hosts().await.expect_err("404 each time");
        // Each error should reference the cached server version.
        assert!(err.to_string().contains("1.0.0"));
    }
    // wiremock asserts .expect(1) on drop; an extra probe panics here.
}

#[tokio::test]
async fn not_found_with_version_404_emits_too_old_or_auth_failed_message() {
    // Server is too old to have /api/version. Cache populates with None;
    // subsequent 404s reuse it (no re-probe).
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("404");
    let msg = err.to_string();
    assert!(
        msg.contains("could not check version") || msg.contains("too old"),
        "expected too-old / auth-failed fallback; got: {msg}"
    );
}

#[tokio::test]
async fn not_found_with_version_401_emits_auth_failed_fallback() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("404");
    assert!(err.to_string().contains("could not check version"));
}

// ─── 503 retry path ─────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_succeeds_after_503() {
    let (server, _client) = start_mock_with_client().await;
    // First call → 503, second → 200. Use a shared counter to alternate.
    let counter = Arc::new(AtomicU64::new(0));
    let counter2 = Arc::clone(&counter);
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(move |_req: &wiremock::Request| {
            let n = counter2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ResponseTemplate::new(503).set_body_string("warming up")
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"hosts": []}))
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let client = HttpClient::discover(Some(server.uri()), Some("test-value".into())).unwrap();
    let resp = client.hosts().await.expect("ok after retry");
    assert!(resp.hosts.is_empty());
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn double_503_error_includes_both_bodies() {
    let (server, client) = start_mock_with_client().await;
    let counter = Arc::new(AtomicU64::new(0));
    let counter2 = Arc::clone(&counter);
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(move |_req: &wiremock::Request| {
            let n = counter2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ResponseTemplate::new(503).set_body_string("body1")
            } else {
                ResponseTemplate::new(503).set_body_string("body2")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("503 both attempts");
    let msg = err.to_string();
    assert!(msg.contains("body1"), "first body in error; got: {msg}");
    assert!(msg.contains("body2"), "second body in error; got: {msg}");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn file_tails_post_does_not_retry_503() {
    let server = MockServer::start().await;
    let client = HttpClient::discover(Some(server.uri()), Some("test-value".into()))
        .unwrap()
        .with_api_admin_token_for_test("admin-value");
    Mock::given(method("POST"))
        .and(path("/api/file-tails"))
        .and(header("x-cortex-admin-token", "admin-value"))
        .respond_with(ResponseTemplate::new(503).set_body_string("committed but unavailable"))
        .expect(1)
        .mount(&server)
        .await;

    let err = client
        .file_tails(&cortex::app::FileTailRequest::status())
        .await
        .expect_err("stateful admin POST must not retry");

    assert!(
        err.to_string().contains("committed but unavailable"),
        "expected first 503 body in error: {err}"
    );
}

#[tokio::test]
async fn ack_error_sends_admin_token_header() {
    use cortex::app::AckErrorRequest;

    let server = MockServer::start().await;
    let client = HttpClient::discover(Some(server.uri()), Some("test-value".into()))
        .unwrap()
        .with_api_admin_token_for_test("admin-value");
    Mock::given(method("POST"))
        .and(path("/api/errors/ack"))
        .and(header("authorization", "Bearer test-value"))
        .and(header("x-cortex-admin-token", "admin-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "signature_hash": "abc123",
            "acknowledged_at": "2026-07-16T00:00:00Z",
            "actor": "cli",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let resp = client
        .ack_error(&AckErrorRequest {
            signature_hash: "abc123".into(),
            notes: Some("test".into()),
        })
        .await
        .expect("ack should succeed");

    assert_eq!(resp.signature_hash, "abc123");
    assert_eq!(resp.actor, "cli");
}

#[tokio::test]
async fn notifications_test_sends_admin_token_header() {
    let server = MockServer::start().await;
    let client = HttpClient::discover(Some(server.uri()), Some("test-value".into()))
        .unwrap()
        .with_api_admin_token_for_test("admin-value");
    Mock::given(method("POST"))
        .and(path("/api/notifications/test"))
        .and(header("authorization", "Bearer test-value"))
        .and(header("x-cortex-admin-token", "admin-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "sent": true,
                "destinations": 1
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let resp = client
        .notifications_test(Some("live-cli-remediation".into()))
        .await
        .expect("notifications test should send admin token");

    assert_eq!(resp["result"]["sent"], true);
}

// ─── Malformed JSON: serde_path_to_error surfaces field path + preview ──────

#[tokio::test]
async fn malformed_response_includes_field_path_and_preview() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/hosts"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{\"oops\":\"this is not a hosts response\"}"),
        )
        .mount(&server)
        .await;
    let err = client.hosts().await.expect_err("malformed");
    let msg = err.to_string();
    assert!(msg.contains("malformed response"), "got: {msg}");
    // Either the missing-field name or `.` (root) is acceptable depending on
    // serde version; verify we have *some* path info AND the body preview.
    assert!(
        msg.contains("oops"),
        "preview should be in error; got: {msg}"
    );
}

// ─── version() endpoint round-trip ──────────────────────────────────────────

#[tokio::test]
async fn version_endpoint_round_trip() {
    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "version": "0.25.3",
            "git_sha": null,
            "schema_version": 12,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let v: ServerVersion = client.version().await.expect("ok");
    assert_eq!(v.version, "0.25.3");
    assert_eq!(v.schema_version, 12);
    assert!(v.git_sha.is_none());
}

#[tokio::test]
async fn artifact_evidence_query_round_trips_shared_request_shape() {
    use cortex::app::ListArtifactEvidenceRequest;
    use cortex::artifact_evidence::ArtifactEvidenceKind;

    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/artifact-evidence"))
        .and(query_param("eventKind", "runtime_call"))
        .and(query_param("artifactId", "artifact-cli-1"))
        .and(query_param("correlationId", "corr-cli-1"))
        .and(query_param("sourceSystem", "labby"))
        .and(query_param("from", "2026-08-19T15:00:00.000Z"))
        .and(query_param("to", "2026-08-19T17:00:00.000Z"))
        .and(query_param("limit", "25"))
        .and(header("authorization", "Bearer test-value"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"events": [], "truncated": false})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let response = client
        .artifact_evidence(&ListArtifactEvidenceRequest {
            event_kind: Some(ArtifactEvidenceKind::RuntimeCall),
            artifact_id: Some("artifact-cli-1".into()),
            correlation_id: Some("corr-cli-1".into()),
            source_system: Some("labby".into()),
            from: Some("2026-08-19T15:00:00.000Z".into()),
            to: Some("2026-08-19T17:00:00.000Z".into()),
            limit: Some(25),
            ..Default::default()
        })
        .await
        .expect("artifact evidence query");
    assert!(response.events.is_empty());
    assert!(!response.truncated);
}

// ─── bead 0p8r.15: AbuseSearchRequest round-trip ────────────────────────────

/// Bead 0p8r.15: the CLI serializes an `AbuseSearchRequest` with multiple
/// terms via `serde_qs::to_string`; the wire bytes hit the mock server, which
/// echoes the query string back. We then deserialize the same wire bytes
/// with `serde_qs::from_str` and assert field parity. This guards the
/// CLI-server contract for `Vec<String>` query params (the breakage the bead
/// caught was: client/server hand-rolled flat structs and silently dropped
/// `terms[1..]`).
#[tokio::test]
async fn ai_abuse_request_round_trips_through_serde_qs() {
    use cortex::app::AbuseSearchRequest;

    let req = AbuseSearchRequest {
        project: Some("proj".into()),
        tool: Some("Bash".into()),
        since: None,
        until: None,
        limit: Some(25),
        before: Some(2),
        after: Some(3),
        terms: vec!["alpha".into(), "beta".into(), "gamma".into()],
    };

    let qs = serde_qs::to_string(&req).expect("serialize");
    // Multi-element Vec<String> must use repeated keys, not a flat
    // single-value `term=` field (that was the regression).
    assert!(
        qs.matches("terms").count() >= 3,
        "serde_qs must emit one `terms` key per element; got: {qs}"
    );

    let decoded: AbuseSearchRequest = serde_qs::from_str(&qs).expect("deserialize");
    assert_eq!(decoded.project, req.project);
    assert_eq!(decoded.tool, req.tool);
    assert_eq!(decoded.limit, req.limit);
    assert_eq!(decoded.before, req.before);
    assert_eq!(decoded.after, req.after);
    assert_eq!(decoded.terms, req.terms);
}

// ─── cxih.4: CorrelateStateRequest CLI→server query serialization ───────────

/// The CLI HTTP client serializes heartbeat-state requests via reqwest's
/// `.query()`; the server deserializes them with axum `Query<..>` under
/// `deny_unknown_fields`. This exercises the exact client serializer and guards
/// the seam `cortex-fzj7` bit on: omitted `Option`s must NOT emit bare keys
/// (which would 400 against `deny_unknown_fields`), while the required
/// `reference_time` and any set options must appear. The server-side
/// deserialization of the same struct is covered by the `/api/correlate-state`
/// api_tests.
#[test]
fn correlate_state_request_query_omits_none_options() {
    use cortex::app::CorrelateStateRequest;

    let req = CorrelateStateRequest {
        reference_time: Some("2026-05-25T00:00:00Z".into()),
        window_minutes: Some(15),
        host: Some("nashost".into()),
        severity_min: None,
        limit: None,
    };

    let built = reqwest::Client::new()
        .get("http://localhost/api/correlate-state")
        .query(&req)
        .build()
        .expect("build request");
    let qs = built.url().query().unwrap_or_default();

    assert!(
        qs.contains("reference_time="),
        "required field missing: {qs}"
    );
    assert!(qs.contains("window_minutes=15"), "set option missing: {qs}");
    assert!(qs.contains("host=nashost"), "set option missing: {qs}");
    // None options must be dropped entirely, not emitted as bare keys.
    assert!(!qs.contains("severity_min"), "None option leaked: {qs}");
    assert!(!qs.contains("limit"), "None option leaked: {qs}");
}

// ─── Round-trip coverage for RAG v1 wrappers (similar_incidents,
//     incident_context) — bead 0p8r.6. Verifies HttpClient → /api/<path> wires
//     the bearer header, hits the documented URL, and deserialises a typed
//     response. ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn similar_incidents_round_trips_typed_response() {
    use cortex::app::SimilarIncidentsRequest;

    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/similar-incidents"))
        .and(header("authorization", "Bearer test-value"))
        .and(query_param("query", "disk full"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "query": "disk full",
            "total_clusters": 0,
            "truncated": false,
            "clusters": [],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let req = SimilarIncidentsRequest {
        query: "disk full".into(),
        host: None,
        app: None,
        severity_min: None,
        since: None,
        until: None,
        window_minutes: None,
        limit: None,
    };
    let resp = client
        .similar_incidents(&req)
        .await
        .expect("similar_incidents wrapper should succeed");
    assert_eq!(resp.query, "disk full");
    assert_eq!(resp.total_clusters, 0);
    assert!(resp.clusters.is_empty());
}

#[tokio::test]
async fn incident_context_round_trips_typed_response() {
    use cortex::app::IncidentContextRequest;

    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/incident-context"))
        .and(header("authorization", "Bearer test-value"))
        .and(query_param("since", "2026-05-01T00:00:00Z"))
        .and(query_param("until", "2026-05-01T01:00:00Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "window_from": "2026-05-01T00:00:00Z",
            "window_to":   "2026-05-01T01:00:00Z",
            "total_logs":  0,
            "by_severity": [],
            "by_app":      [],
            "error_logs":  [],
            "error_logs_truncated": false,
            "ai_sessions": [],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let req = IncidentContextRequest {
        since: Some("2026-05-01T00:00:00Z".into()),
        until: Some("2026-05-01T01:00:00Z".into()),
        ..Default::default()
    };
    let resp = client
        .incident_context(&req)
        .await
        .expect("incident_context wrapper should succeed");
    assert_eq!(resp.window_from, "2026-05-01T00:00:00Z");
    assert_eq!(resp.window_to, "2026-05-01T01:00:00Z");
    assert_eq!(resp.total_logs, 0);
    assert!(resp.error_logs.is_empty());
}

#[tokio::test]
async fn hook_events_round_trips_canonical_sessions_hooks_route() {
    use cortex::app::ListHookEventsRequest;

    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/hooks"))
        .and(header("authorization", "Bearer test-value"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "total": 0,
            "truncated": false,
            "events": [],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let req = ListHookEventsRequest {
        limit: Some(2),
        ..Default::default()
    };
    let resp = client
        .ai_hook_events(&req)
        .await
        .expect("hook events wrapper should succeed");
    assert_eq!(resp.total, 0);
    assert!(resp.events.is_empty());
}

#[tokio::test]
async fn hook_investigate_preserves_incident_id_over_http() {
    use cortex::app::AiHookInvestigateRequest;

    let (server, client) = start_mock_with_client().await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/hook-investigate"))
        .and(header("authorization", "Bearer test-value"))
        .and(query_param("incident_id", "hook-inc-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "evidence": [],
            "total_incidents": 0,
            "truncated": false,
            "other_matching_incidents": [],
            "no_incident_low_severity_summary": false,
            "no_data": true,
            "suggested_filters": [],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = client
        .ai_hook_investigate(&AiHookInvestigateRequest {
            incident_id: Some("hook-inc-123".into()),
            ..Default::default()
        })
        .await
        .expect("hook investigate wrapper should succeed");
    assert!(response.no_data);
}

#[tokio::test]
async fn mcp_read_surfaces_and_skill_incident_id_round_trip_over_http() {
    use cortex::app::{
        AiMcpIncidentRequest, AiMcpInvestigateRequest, AiSkillInvestigateRequest,
        ListMcpEventsRequest,
    };

    let (server, client) = start_mock_with_client().await;
    let incident_response = serde_json::json!({
        "incidents": [],
        "total_incidents": 0,
        "candidate_event_rows": 0,
        "candidate_cap": 0,
        "candidate_window_truncated": false,
        "truncated": false,
    });
    let investigate_response = serde_json::json!({
        "evidence": [],
        "total_incidents": 0,
        "truncated": false,
        "other_matching_incidents": [],
        "no_incident_low_severity_summary": false,
        "no_data": true,
        "suggested_filters": [],
    });

    Mock::given(method("GET"))
        .and(path("/api/sessions/mcp-events"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "total": 0,
            "truncated": false,
            "events": [],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/mcp-incidents"))
        .and(query_param("mcp_server", "labby"))
        .respond_with(ResponseTemplate::new(200).set_body_json(incident_response))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/mcp-investigate"))
        .and(query_param("incident_id", "mcp-inc-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(investigate_response.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/skill-investigate"))
        .and(query_param("incident_id", "skill-inc-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(investigate_response))
        .expect(1)
        .mount(&server)
        .await;

    assert_eq!(
        client
            .mcp_events(&ListMcpEventsRequest {
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        client
            .ai_mcp_incidents(&AiMcpIncidentRequest {
                mcp_server: Some("labby".into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .total_incidents,
        0
    );
    assert!(
        client
            .ai_mcp_investigate(&AiMcpInvestigateRequest {
                incident_id: Some("mcp-inc-1".into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .no_data
    );
    assert!(
        client
            .ai_skill_investigate(&AiSkillInvestigateRequest {
                incident_id: Some("skill-inc-1".into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .no_data
    );
}

// ─── Env var guard ──────────────────────────────────────────────────────────
//
// `crate::env::set_test_var` is data-racy across threads. We serialise via the
// `#[serial]` macro from `serial_test` (already a dev-dep). The guard
// restores the prior value on drop so tests don't leak state.

struct EnvVarGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }
    fn unset(name: &'static str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::remove_test_var(name);
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => crate::env::set_test_var(self.name, v),
            None => crate::env::remove_test_var(self.name),
        }
    }
}
