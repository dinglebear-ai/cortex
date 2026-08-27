//! OTLP request authorization: bearer-token gate, unauthorized-response
//! shaping, and rate-limited unauthorized-attempt logging.

use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::{
    http::{
        HeaderMap,
        header::{AUTHORIZATION, USER_AGENT},
    },
    response::IntoResponse,
};
use lru::LruCache;
use sha2::{Digest, Sha256};

use crate::mcp::AuthPolicy;
use lab_auth::middleware::{parse_bearer_token, tokens_equal};

use super::OtlpState;

pub(super) fn is_authorized(state: &OtlpState, headers: &HeaderMap) -> bool {
    // No-auth policies: loopback bind or upstream gateway is the trust boundary.
    if matches!(
        state.auth_policy,
        AuthPolicy::LoopbackDev | AuthPolicy::TrustedGatewayUnscoped
    ) {
        return true;
    }
    // Mounted auth: require the static bearer token. If none is configured
    // (OAuth-only deployment), OTLP is denied — there is no OAuth flow for
    // machine-to-machine OTLP exporters.
    let Some(expected) = state.api_token.as_deref() else {
        return false;
    };
    let Some(auth) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    parse_bearer_token(auth).is_some_and(|tok| tokens_equal(&tok, expected))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UnauthorizedDiagnostics {
    pub(super) has_auth: bool,
    pub(super) auth_scheme: String,
    pub(super) bearer_sha256_12: String,
    pub(super) user_agent: String,
}

// LRU-bounded (not scan-evicted): a flooding attacker generating >MAX_KEYS
// distinct fingerprints per interval can still evict older entries, but each
// *new* distinct key is always recorded and warned on — eviction never
// permanently wedges the table the way a "drop only if stale" scan can.
static UNAUTHORIZED_WARNINGS: LazyLock<Mutex<LruCache<String, Instant>>> = LazyLock::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(UNAUTHORIZED_WARNING_MAX_KEYS)
            .expect("UNAUTHORIZED_WARNING_MAX_KEYS > 0"),
    ))
});

const UNAUTHORIZED_WARNING_INTERVAL: Duration = Duration::from_secs(60);
const UNAUTHORIZED_WARNING_MAX_KEYS: usize = 1024;
const MAX_DIAGNOSTIC_FIELD_LEN: usize = 128;

pub(super) fn should_warn_unauthorized(
    peer: &SocketAddr,
    diagnostics: &UnauthorizedDiagnostics,
) -> bool {
    let key = unauthorized_warning_key(peer, diagnostics);
    let now = Instant::now();
    let Ok(mut warnings) = UNAUTHORIZED_WARNINGS.lock() else {
        return true;
    };
    record_unauthorized_warning(&mut warnings, key, now, UNAUTHORIZED_WARNING_INTERVAL)
}

fn unauthorized_warning_key(peer: &SocketAddr, diagnostics: &UnauthorizedDiagnostics) -> String {
    format!(
        "{}|{}|{}|{}",
        peer.ip(),
        diagnostics.auth_scheme,
        diagnostics.bearer_sha256_12,
        diagnostics.user_agent
    )
}

fn record_unauthorized_warning(
    warnings: &mut LruCache<String, Instant>,
    key: String,
    now: Instant,
    interval: Duration,
) -> bool {
    match warnings.get(&key).copied() {
        Some(last) if now.duration_since(last) < interval => false,
        _ => {
            warnings.put(key, now);
            true
        }
    }
}

pub(super) fn unauthorized_diagnostics(headers: &HeaderMap) -> UnauthorizedDiagnostics {
    let auth = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    let bearer = auth.and_then(parse_bearer_token);
    UnauthorizedDiagnostics {
        has_auth: auth.is_some(),
        auth_scheme: auth_scheme(auth),
        bearer_sha256_12: bearer
            .as_deref()
            .map(sha256_12)
            .unwrap_or_else(|| "none".to_string()),
        user_agent: headers
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(truncate_diagnostic_field)
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

fn auth_scheme(auth: Option<&str>) -> String {
    auth.and_then(|value| value.split_ascii_whitespace().next())
        .filter(|scheme| !scheme.is_empty())
        .unwrap_or("none")
        .to_ascii_lowercase()
}

fn sha256_12(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    format!("{digest:x}")[..12].to_string()
}

fn truncate_diagnostic_field(value: &str) -> String {
    value.chars().take(MAX_DIAGNOSTIC_FIELD_LEN).collect()
}

pub(super) fn otlp_auth_policy_label(policy: &AuthPolicy) -> &'static str {
    match policy {
        AuthPolicy::LoopbackDev => "loopback_dev",
        AuthPolicy::TrustedGatewayUnscoped => "trusted_gateway",
        AuthPolicy::Mounted { .. } => "mounted",
    }
}

pub(super) fn unauthorized() -> axum::response::Response {
    super::error::OtlpError::Unauthorized.into_response()
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
