use std::sync::Arc;

use lab_auth::AuthLayer;

use crate::app::CortexService;
use crate::config::{McpConfig, NotificationsConfig};
use crate::observability::RuntimeObservability;
use crate::otlp::OtlpCounters;

mod action_flags;
mod actions;
mod artifact_evidence_schema;
mod prompts;
mod rmcp_server;
mod routes;
mod schemas;
mod tools;

pub use action_flags::{Defaults, FlagSpec, ValueKind};
#[cfg(test)]
pub(crate) use actions::required_scope_for;
pub use actions::{
    action_names, defaults_for, description_for, examples_for, flags_for, positional_for,
};
pub use rmcp_server::{
    CortexRmcpServer, rmcp_server, streamable_http_config, streamable_http_service,
};
pub use routes::router;

/// Authentication policy attached to [`AppState`].
///
/// This is intentionally an enum (not `Option<Arc<AuthState>>` and not a
/// `bool`) so that constructing an `AppState` requires an *explicit* choice
/// between "no auth wired (loopback dev)" and "auth wired". There is no
/// `Default` impl — code that builds an `AppState` must name the variant.
///
/// Locked by the OAuth epic post-spike: when `auth_state` is `Some`, the
/// shared [`lab_auth::state::AuthState`] backs both the dual-mode middleware
/// and the OAuth router. When `None`, only static-bearer auth is active —
/// middleware still validates the token but no OAuth flow is wired.
/// AuthContext flows per-request via axum extension propagation
/// (see `docs/internal/rmcp-auth-spike.md`); no session-keyed map on
/// `AppState`.
#[derive(Clone)]
pub enum AuthPolicy {
    /// No authentication is wired. Only legal when the MCP listener is
    /// bound to a loopback address (validated by [`crate::config::Config::load`]).
    /// Scope checks are bypassed in this variant — the bind itself is the
    /// trust boundary.
    LoopbackDev,
    /// No authentication is wired because an upstream gateway is expected to
    /// enforce access before traffic reaches cortex.
    TrustedGatewayUnscoped,
    /// Authentication middleware is mounted. Scope checks MUST run.
    /// `auth_state` is:
    /// - `Some` when OAuth mode is active (Google flow + JWKS issuance
    ///   available; the OAuth router is mounted on these paths);
    /// - `None` when only static-bearer mode is active (no OAuth router
    ///   mounted; middleware validates `CORTEX_TOKEN` via lab-auth's
    ///   `AuthLayer::with_static_token`).
    Mounted {
        auth_state: Option<Arc<lab_auth::state::AuthState>>,
    },
}

// Manual Debug impl: `lab_auth::state::AuthState` does not implement Debug
// (it holds RSA signing keys we never want printed), but we still want
// `AuthPolicy` to be `Debug` for use in `Result::expect`/`expect_err` and
// startup tracing.
impl std::fmt::Debug for AuthPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthPolicy::LoopbackDev => f.write_str("AuthPolicy::LoopbackDev"),
            AuthPolicy::TrustedGatewayUnscoped => f.write_str("AuthPolicy::TrustedGatewayUnscoped"),
            AuthPolicy::Mounted {
                auth_state: Some(_),
            } => f.write_str("AuthPolicy::Mounted { auth_state: Some(<lab_auth::AuthState>) }"),
            AuthPolicy::Mounted { auth_state: None } => {
                f.write_str("AuthPolicy::Mounted { auth_state: None /* bearer-only */ }")
            }
        }
    }
}

/// Shared app state
#[derive(Clone)]
pub struct AppState {
    pub service: CortexService,
    pub config: McpConfig,
    /// Notifications subsystem configuration. Carried separately from `config`
    /// because `McpConfig` is the MCP-layer slice; notifications config lives
    /// on the top-level `Config` struct. Tools that need Apprise URLs (e.g.
    /// `notifications_test`) must read from here, not from caller-supplied args,
    /// to prevent SSRF.
    pub notifications_config: NotificationsConfig,
    pub otlp_counters: Arc<OtlpCounters>,
    /// Authentication policy. Construction MUST name a variant — there is no
    /// implicit default. See [`AuthPolicy`].
    pub auth_policy: AuthPolicy,
    pub observability: Arc<RuntimeObservability>,
}

/// Build an [`AuthLayer`] from an [`AuthPolicy`], or return `None` for
/// no-auth policies (no layer needed — loopback or trusted gateway is the
/// trust boundary).
///
/// Centralises the `AuthLayer` construction shared by `api.rs` and
/// `mcp/routes.rs`.
///
/// # Invariant
/// `AuthLayer` MUST NOT add any DB write path. JWT validation is stateless
/// RS256 verify; static token is constant-time compare. If audit logging is
/// ever needed, push to an async background channel.
///
/// # Static token scopes (bearer-only)
/// When `auth_state` is `None` (bearer-only mode), `AuthLayer::new()` starts
/// with `static_token_scopes: Vec::new()` and `with_auth_state(None)` does
/// not set scopes (it only reads scopes from a `Some(AuthState)`). Without an
/// explicit call to `with_static_token_scopes`, a static-bearer request would
/// produce an `AuthContext` with no scopes and fail every scope check.
/// We therefore call `with_static_token_scopes` unconditionally so that the
/// bearer-only path grants the configured scopes for the static token.
///
/// # Static token admin opt-in
/// By default, static bearer tokens receive only `cortex:read`. Set
/// `static_token_is_admin = true` (via `CORTEX_STATIC_TOKEN_ADMIN=true`
/// or `[mcp] static_token_is_admin = true` in config.toml) to also grant
/// `cortex:admin`. OAuth tokens are unaffected — their scopes come from the
/// JWT claims.
pub fn build_auth_layer(
    policy: &AuthPolicy,
    static_token: Option<Arc<str>>,
    resource_url: Option<Arc<str>>,
    static_token_is_admin: bool,
) -> Option<AuthLayer> {
    match policy {
        AuthPolicy::LoopbackDev | AuthPolicy::TrustedGatewayUnscoped => None,
        AuthPolicy::Mounted { auth_state } => {
            // Default: static bearer tokens receive read-only scope.
            // Opt-in: set CORTEX_STATIC_TOKEN_ADMIN=true to also grant admin.
            // OAuth tokens gate admin via the scope claims in the JWT.
            let static_scopes: Vec<String> = if static_token_is_admin {
                vec!["cortex:read".to_string(), "cortex:admin".to_string()]
            } else {
                vec!["cortex:read".to_string()]
            };
            Some(
                AuthLayer::new()
                    .with_static_token(static_token)
                    .with_auth_state(auth_state.clone())
                    // When auth_state is None (bearer-only), with_auth_state does not
                    // populate static_token_scopes (it only copies from Some(AuthState)).
                    // Explicitly set here so static bearer tokens receive the configured
                    // scopes in both bearer-only and OAuth modes.
                    .with_static_token_scopes(static_scopes)
                    .with_resource_url(resource_url)
                    .with_allow_session_cookie(false),
            )
        }
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
