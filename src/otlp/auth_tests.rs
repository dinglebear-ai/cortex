//! Tests for the OTLP bearer-token auth gate and unauthorized-attempt
//! diagnostics/rate-limiting.

use super::*;

use std::sync::Arc;

use axum::http::HeaderValue;

use super::super::OtlpCounters;

fn state_with_token(token: Option<&str>) -> OtlpState {
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(10);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    // Use Mounted when a token is configured so is_authorized enforces it.
    let auth_policy = if token.is_some() {
        crate::mcp::AuthPolicy::Mounted { auth_state: None }
    } else {
        crate::mcp::AuthPolicy::LoopbackDev
    };
    OtlpState::new(
        ingest,
        token.map(String::from),
        Arc::new(OtlpCounters::default()),
        auth_policy,
    )
}

#[test]
fn auth_disabled_when_no_token() {
    let state = state_with_token(None);
    let headers = HeaderMap::new();
    assert!(is_authorized(&state, &headers));
}

#[test]
fn auth_required_with_correct_bearer() {
    let state = state_with_token(Some("secret"));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer secret"),
    );
    assert!(is_authorized(&state, &headers));
}

#[test]
fn auth_rejects_wrong_token() {
    let state = state_with_token(Some("secret"));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer wrong"),
    );
    assert!(!is_authorized(&state, &headers));
}

#[test]
fn auth_rejects_missing_header() {
    let state = state_with_token(Some("secret"));
    let headers = HeaderMap::new();
    assert!(!is_authorized(&state, &headers));
}

#[test]
fn auth_rejects_non_bearer_scheme() {
    let state = state_with_token(Some("secret"));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Basic secret"),
    );
    assert!(!is_authorized(&state, &headers));
}

#[test]
fn unauthorized_diagnostics_hashes_bearer_without_logging_token() {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer wrong"),
    );
    headers.insert(
        axum::http::header::USER_AGENT,
        HeaderValue::from_static("otel-test/1.0"),
    );

    let diagnostics = unauthorized_diagnostics(&headers);

    assert!(diagnostics.has_auth);
    assert_eq!(diagnostics.auth_scheme, "bearer");
    assert_eq!(diagnostics.bearer_sha256_12, sha256_12("wrong"));
    assert!(!diagnostics.bearer_sha256_12.contains("wrong"));
    assert_eq!(diagnostics.user_agent, "otel-test/1.0");
}

#[test]
fn unauthorized_diagnostics_handles_missing_auth() {
    let diagnostics = unauthorized_diagnostics(&HeaderMap::new());

    assert!(!diagnostics.has_auth);
    assert_eq!(diagnostics.auth_scheme, "none");
    assert_eq!(diagnostics.bearer_sha256_12, "none");
    assert_eq!(diagnostics.user_agent, "unknown");
}

fn test_lru(cap: usize) -> LruCache<String, Instant> {
    LruCache::new(NonZeroUsize::new(cap).unwrap())
}

#[test]
fn unauthorized_warning_rate_limit_suppresses_repeats_per_key() {
    let mut warnings = test_lru(1024);
    let now = std::time::Instant::now();
    let interval = std::time::Duration::from_secs(60);
    let key = "198.51.100.1|bearer|abcdef123456|otel".to_string();

    assert!(record_unauthorized_warning(
        &mut warnings,
        key.clone(),
        now,
        interval
    ));
    assert!(!record_unauthorized_warning(
        &mut warnings,
        key.clone(),
        now + std::time::Duration::from_secs(30),
        interval,
    ));
    assert!(record_unauthorized_warning(
        &mut warnings,
        key,
        now + interval,
        interval,
    ));
}

// Regression test for syslog-mcp-zy9bs: the old scan-based eviction
// (`retain` entries newer than `interval`) never dropped fresh entries, so
// once an attacker filled the cap with distinct fingerprints inside one
// interval, EVERY subsequent distinct key -- including a real, different
// attacker -- was silently suppressed until the flooded entries aged out.
// LRU eviction guarantees the newest distinct key is always recorded and
// warned on, at the cost of evicting the least-recently-seen entry.
#[test]
fn unauthorized_warning_rate_limit_evicts_oldest_key_when_at_capacity() {
    let mut warnings = test_lru(4);
    let now = std::time::Instant::now();
    let interval = std::time::Duration::from_secs(60);

    for i in 0..4 {
        assert!(record_unauthorized_warning(
            &mut warnings,
            format!("key-{i}"),
            now,
            interval,
        ));
    }
    // A 5th distinct key at capacity still warns -- it is never silently
    // dropped -- and evicts the least-recently-used entry (key-0) to do so.
    assert!(record_unauthorized_warning(
        &mut warnings,
        "key-4".to_string(),
        now,
        interval,
    ));
    assert_eq!(warnings.len(), 4);
    assert!(warnings.peek("key-0").is_none());
    assert!(warnings.peek("key-4").is_some());
}

#[test]
fn unauthorized_diagnostics_truncates_user_agent() {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::USER_AGENT,
        HeaderValue::from_str(&"x".repeat(MAX_DIAGNOSTIC_FIELD_LEN + 10)).unwrap(),
    );

    let diagnostics = unauthorized_diagnostics(&headers);

    assert_eq!(diagnostics.user_agent.len(), MAX_DIAGNOSTIC_FIELD_LEN);
}
