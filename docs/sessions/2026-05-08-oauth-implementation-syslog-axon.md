---
date: 2026-05-08 18:55:30 EST
repo: https://github.com/jmagar/syslog-mcp
branch: fix/oauth-scope-bearer-bugs
head: be77514
agent: Claude (claude-sonnet-4-6)
session id: 3a8bdaf9-721c-4e0b-8a6b-cffe2740c8d5
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/3a8bdaf9-721c-4e0b-8a6b-cffe2740c8d5.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
worktree: /home/jmagar/workspace/syslog-mcp/.claude/worktrees/oauth-integration [worktree-oauth-integration]
pr: "#18 fix(oauth): fail-conservative unknown scope + bearer-only static token scopes — https://github.com/jmagar/syslog-mcp/pull/18 (MERGED)"
---

## User Request

Implement OAuth 2.0 authentication for the syslog-mcp MCP server using a shared `lab-auth` crate extracted from the lab homelab gateway, supporting both static bearer tokens and Google-backed JWTs simultaneously. After implementation, replicate the same OAuth pattern for `axon_rust`.

## Session Overview

Implemented end-to-end OAuth 2.0 for two Rust MCP servers (syslog-mcp and axon_rust) using a shared `lab-auth` crate. The lab-auth crate was refactored from a lab-specific library into a parameterizable shared crate. Both services now support Google OAuth login with PKCE, JWT issuance, scope-based authorization, and dynamic client registration — alongside the existing static bearer token. The full flow was debugged live against Claude Code and claude.ai clients, exposing and fixing several production issues.

## Sequence of Events

1. **Planning phase**: Explored lab-auth codebase (~6,900 LOC), designed 10-bead epic (`syslog-mcp-brt0`) for OAuth port. Multiple research rounds (6 agents), design reviews (24 recommendations applied), engineering review (5 critical + 8 important findings).

2. **Lab-auth crate refactor** (lab PRs: L1–L3):
   - L1: Parameterized all `"lab"`-branded constants (env prefix, scopes, resource path, cookie name). Added `AuthConfigBuilder` consuming-builder pattern. JWKS 5s timeout. JWT issuer in `Validation::set_issuer()` per RFC 7519.
   - L2: Moved `authenticate_request` middleware into `AuthLayer` (Tower `Layer` impl). Added `bearer_only_router()` and `router()`. `AuthContext` type moved to crate.
   - L3: Updated lab consumer to use parameterized crate; deleted ~322 LOC of duplicated middleware.
   - Lab PR #51 merged at SHA `87cec324`.

3. **syslog-mcp integration** (bead waves S1–S6):
   - S1: Added lab-auth dep (git+rev), `AuthConfig` in config.rs, 5 OAuth env vars, non-loopback safety gate using `IpAddr::is_loopback()`.
   - S2: `AuthPolicy` enum on `AppState` (Mounted/LoopbackDev), `AuthState` initialized in `RuntimeCore`, backup scripts updated.
   - S3: Replaced legacy `require_auth` middleware with `AuthLayer`, de-duplicated between `mcp/routes.rs` and `api.rs`.
   - S4: Mounted full OAuth router (`/register`, `/.well-known/*`, `/jwks`, `/authorize`, `/token`, `/auth/google/callback`). Extended CORS/Host allowlist from `SYSLOG_MCP_PUBLIC_URL`.
   - S5: Fail-closed scope check via `AuthPolicy` enum match. `tools/list` exempt from scope. AuthContext via rmcp extension propagation (Pattern (a) confirmed by spike B0).
   - S6: Integration tests (266 total), OAuth docs (`docs/OAUTH.md`), smoke test extension, SHA-bump automation.
   - syslog-mcp PR #17 merged at `e40f54b`.

4. **Review passes**: lavra-review (4 agents), simplify (2 fixes), pr-review-toolkit (critical: wrong-key JWT test, wrong aud test, `tools/list` blocked pre-auth), gh-address-comments (19 threads resolved for PR #17, 6 threads for lab PR #51).

5. **Live deployment and debugging**:
   - Deployed OAuth binary via systemd drop-in (`~/.config/systemd/user/syslog-mcp.service.d/oauth.conf`).
   - Fixed DCR: `bearer_only_router()` excludes `/register`; switched to `router()`.
   - Fixed scope validation: lab-auth rejected `syslog:read syslog:admin` (space-separated); added multi-scope support in `authorize.rs`.
   - Fixed SWAG proxy for `syslog.example.internal`: removed `oauth.conf` include, added direct OAuth endpoint routes. Fixed `WWW-Authenticate` path-based PRM URL (`/mcp/.well-known/oauth-protected-resource`).
   - Fixed claude.ai redirect URI: added `https://claude.ai/api/mcp/auth_callback` to `allowed_client_redirect_uris`.
   - OAuth flow verified end-to-end: Claude Code plugin authenticated successfully.

6. **Post-merge bug fixes** (PR #18):
   - `required_scope_for` fallback was fail-permissive (`Some("syslog:read")` → `Some("syslog:__deny__")`); 12 missing valid actions added.
   - `build_auth_layer` didn't set `static_token_scopes` in bearer-only mode; static bearer got empty scopes and failed all scope checks.

7. **axon_rust OAuth** (PR #76, #77):
   - Dispatched agent to implement OAuth matching syslog-mcp's pattern. Key difference: axon uses sqlx-sqlite 0.8 which pins `libsqlite3-sys 0.30`, conflicting with lab-auth's rusqlite 0.39. Resolved by vendoring lab-auth with rusqlite downgraded to 0.32.
   - PR #76 merged. Post-merge fixes: fail-conservative scope default, artifacts/scrape write scope, x-api-key header normalization.
   - Configured SWAG proxy for `axon.example.internal` matching syslog pattern.
   - Discovered rmcp `StreamableHttpServerConfig` didn't have `with_allowed_hosts` set → requests from Cloudflare rejected. Fixed in PR #77.

## Key Findings

- **rmcp extension propagation (B0 spike)**: Pattern (a) confirmed — `ctx.extensions.get::<axum::http::request::Parts>()?.extensions.get::<AuthContext>()` works in rmcp 1.6 with `stateful_mode=false`. The research finding claiming it didn't was wrong.
- **scope validation**: `validate_scope` in lab-auth only accepted exact `default_scope` match; MCP clients send `syslog:read syslog:admin` as a space-separated string. Fixed in lab-auth `authorize.rs:431`.
- **DCR flow**: `bearer_only_router()` excludes `/register` unconditionally; Claude Code MCP SDK requires DCR. Must use `router()` (full) with `enable_dynamic_registration(true)`.
- **path-based PRM**: `WWW-Authenticate` header points to `<resource_url>/.well-known/oauth-protected-resource` (e.g., `https://syslog.example.internal/mcp/.well-known/oauth-protected-resource`). SWAG must proxy this path to the upstream.
- **rmcp allowed_hosts** (axon): `StreamableHttpServerConfig::default()` only allows loopback. Must call `.with_allowed_hosts(...)` with the public hostname to allow requests from external reverse proxies.
- **libsqlite3-sys conflict** (axon): sqlx-sqlite 0.8 pins `libsqlite3-sys 0.30`; rusqlite 0.39 requires `0.37`. Both declare `links = "sqlite3"` — Cargo hard error. Vendored lab-auth with rusqlite 0.32.

## Technical Decisions

- **Shared crate (Route B)** over copy-and-adapt: single source of truth for OAuth primitives; security fixes propagate to both syslog-mcp and axon. git+rev pinned at SHA, not path dep (CI safety).
- **AuthPolicy enum** (not bool): `Mounted { auth_state: Option<Arc<AuthState>> }` and `LoopbackDev`. No `Default` impl — every constructor must name a variant. Guards against accidental permit-without-context.
- **Fail-closed scope check**: unknown actions return `Some("syslog:__deny__")` sentinel, not `Some("syslog:read")`. A future action added to dispatch but missing from scope map is denied, not silently permitted.
- **Env vars only for secrets/URLs/mode**: TTLs, rate limits, paths, policy flags → `config.toml [mcp.auth]`. Only 5 env vars: `SYSLOG_MCP_AUTH_MODE`, `SYSLOG_MCP_PUBLIC_URL`, `SYSLOG_MCP_GOOGLE_CLIENT_ID`, `SYSLOG_MCP_GOOGLE_CLIENT_SECRET` + existing `SYSLOG_MCP_TOKEN`.
- **systemd drop-in** for deployment: `~/.config/systemd/user/syslog-mcp.service.d/oauth.conf` survives plugin system resets.
- **Full OAuth router** (not bearer_only_router): needed for DCR (`/register`). The distinction matters — `bearer_only_router` was designed for headless consumers that don't need browser flows.

## Files Modified

**lab repo (lab-auth crate):**
- `crates/lab-auth/src/config.rs` — AuthConfigBuilder, 10 new parameterizable fields
- `crates/lab-auth/src/middleware.rs` (NEW) — AuthLayer (Tower Layer), authenticate_request, AuthContext
- `crates/lab-auth/src/auth_context.rs` (NEW) — AuthContext struct moved from lab/src/api/oauth.rs
- `crates/lab-auth/src/routes.rs` — bearer_only_router(), full router() with conditional /register
- `crates/lab-auth/src/authorize.rs` — multi-scope validation (space-separated)
- `crates/lab/src/api/router.rs` — deleted ~165 LOC of duplicated middleware; uses AuthLayer
- `crates/lab/src/api/oauth.rs` — collapsed to `pub use lab_auth::AuthContext;`

**syslog-mcp repo:**
- `src/config.rs` — AuthConfig struct, 5 env vars, `IpAddr::is_loopback()` safety gate, validate_auth_config
- `src/runtime.rs` — build_auth_policy, AuthState init, from_config_inner(is_stdio), query_only override
- `src/mcp.rs` — AuthPolicy enum, build_auth_layer() helper
- `src/mcp/routes.rs` — OAuth router merge, AuthLayer application
- `src/mcp/rmcp_server.rs` — fail-closed scope check, required_scope_for (all 18 actions), tools/list exemption
- `src/api.rs` — AuthLayer via build_auth_layer, de-duplicated
- `src/lib.rs` — pub mod testing (cfg-gated), state helpers for integration tests
- `tests/auth_modes.rs` (NEW) — 15 integration tests
- `tests/oauth_flow.rs` (NEW) — 8 JWT tests including wrong-key and wrong-aud
- `scripts/backup.sh` — auth.db WAL checkpoint, jwt.pem via `install -m 600`
- `docs/OAUTH.md` (NEW) — OAuth setup guide
- `config.toml` — `[mcp.auth]` section
- `.github/workflows/lab-auth-bump.yml` — SHA-bump automation

**SWAG nginx (edgehost:/mnt/appdata/swag/nginx/proxy-confs/):**
- `syslog-mcp.subdomain.conf` — removed `oauth.conf` include + `auth_request`, added direct OAuth routes + `/mcp/.well-known/oauth-protected-resource`
- `axon.subdomain.conf` — same pattern applied for axon.example.internal

**axon_rust repo:**
- `src/mcp/auth.rs` — AuthPolicy enum, build_auth_policy, build_auth_layer, x-api-key normalizer
- `src/mcp/server.rs` — fail-closed scope check (axon:read/axon:write)
- `src/mcp/server/http.rs` — rmcp with_allowed_hosts from AXON_MCP_ALLOWED_ORIGINS
- `vendor/lab-auth/` — vendored with rusqlite 0.32 (libsqlite3-sys conflict workaround)

## Commands Executed

```bash
# Lab PR creation and merge
git push origin worktree-lab-auth-extract
gh pr create --title "feat: extract lab-auth..."  # lab PR #51
gh pr merge 51 --squash  # merged at 87cec324

# syslog-mcp build and deploy
cargo build --release  # worktree
systemctl --user daemon-reload && systemctl --user restart syslog-mcp

# OAuth endpoint verification
curl -s https://syslog.example.internal/.well-known/oauth-authorization-server
curl -s https://syslog.example.internal/mcp/.well-known/oauth-protected-resource
curl -si https://syslog.example.internal/mcp -X POST -H "Authorization: Bearer <token>" ...

# syslog-mcp PR merge
gh pr merge 17 --squash  # merged at e40f54b
gh pr merge 18 --squash  # bug fixes merged

# axon deploy
systemctl --user stop axon-mcp
cp target/release/axon /home/jmagar/.claude/plugins/cache/.../bin/axon
systemctl --user start axon-mcp
```

## Errors Encountered

| Error | Root cause | Resolution |
|-------|-----------|------------|
| `scope must be syslog:read` on `/authorize` | `validate_scope` only accepted exact `default_scope` string, not space-separated combinations | Added multi-scope validation in `lab-auth/src/authorize.rs:431` |
| `/register` returns 404 | `bearer_only_router` excludes `/register` unconditionally | Switched to `router()` with `enable_dynamic_registration(true)` |
| `403 Forbidden: Host header is not allowed` (axon via Cloudflare) | `StreamableHttpServerConfig::default()` only allows loopback; external hosts rejected at rmcp layer before axon middleware | Added `with_allowed_hosts(...)` from `AXON_MCP_ALLOWED_ORIGINS` in `mcp_http_router` |
| Stdio mode crashes (`Config::load()` rejects `0.0.0.0` without auth) | `validate_auth_config` ran before stdio mode could override the bind check | Added `Config::load_for_stdio()` that skips the bind gate; `query_only()` forces `LoopbackDev` |
| `libsqlite3-sys` link conflict in axon | sqlx-sqlite 0.8 pins `libsqlite3-sys 0.30`; rusqlite 0.39 needs `0.37`; Cargo rejects duplicate `links = "sqlite3"` | Vendored lab-auth with rusqlite 0.32 |
| Static bearer token gets 403 on scope-gated actions | `build_auth_layer` didn't call `with_static_token_scopes` in bearer-only mode; `AuthLayer` initialized scopes as empty | Added `.with_static_token_scopes(["syslog:read","syslog:admin"])` conditionally in bearer-only branch |

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| `/mcp` auth | Static bearer token only (`SYSLOG_MCP_TOKEN`) | Static bearer + Google OAuth JWTs; both work simultaneously |
| Scope enforcement | None — any authenticated caller could call any action | Fail-closed: `syslog:read` for all 18 read actions; unknown → denied |
| OAuth discovery | No endpoints | `/.well-known/oauth-authorization-server`, `/jwks`, `/authorize`, `/token`, `/register`, `/auth/google/callback` |
| Redirect URI allowlist | N/A | `https://claude.ai/api/mcp/auth_callback` explicitly allowed |
| Stdio mode (`cargo run -- mcp`) | Bearer token validated against static token | Always `LoopbackDev` (no auth enforced; process isolation is trust boundary) |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test --features test-support` | 277+ pass | 277 pass | ✅ |
| `curl https://syslog.example.internal/.well-known/oauth-authorization-server` | issuer=https://syslog.example.internal | issuer: https://syslog.example.internal | ✅ |
| `curl https://syslog.example.internal/mcp/.well-known/oauth-protected-resource` | 200 with resource URL | 200, resource: https://syslog.example.internal/mcp | ✅ |
| Claude Code plugin OAuth flow | Authenticate → Google → Connected | Authentication successful. Connected to syslog. | ✅ |
| `curl https://axon.example.internal/.well-known/oauth-authorization-server` | issuer=https://axon.example.internal | issuer: https://axon.example.internal | ✅ |
| axon POST /mcp via Cloudflare | 200 (after init) | Unexpected message (correct — needs initialize first) | ✅ |

## Risks and Rollback

- **lab-auth rev pin**: syslog-mcp pins lab-auth at SHA `87cec324`. If lab-auth changes break compatibility, update the rev in `Cargo.toml` and rebuild. A weekly GitHub Actions workflow (`.github/workflows/lab-auth-bump.yml`) handles automated bumps.
- **axon libsqlite3-sys vendoring**: axon vendors lab-auth with rusqlite 0.32. If rusqlite API changes, update the vendor copy. Track upstream when sqlx-sqlite or rusqlite resolve the conflict.
- **Rollback syslog-mcp**: Set `SYSLOG_MCP_AUTH_MODE=bearer` + `SYSLOG_MCP_TOKEN=<token>` in the systemd drop-in env file. The server falls to bearer-only mode without touching the binary.
- **Rollback axon**: Same — `AXON_MCP_AUTH_MODE` absent defaults to bearer mode; `AXON_MCP_HTTP_TOKEN` continues to work.

## Decisions Not Taken

- **Copy-and-adapt (Route A)**: Vendoring ~3,000 LOC of OAuth primitives into syslog-mcp avoided. Chosen: shared git+rev dep so security fixes propagate to both services.
- **mcp-auth.example.internal as external AS**: The homelab already runs `mcp-auth.example.internal` as a central OAuth server. Rejected in favor of each service owning its OAuth AS, matching the planned architecture.
- **Browser sessions** (`/auth/login`): Excluded from syslog-mcp since no browser-facing routes exist. The full `router()` does mount `/auth/login`; syslog-mcp doesn't wire the cookie middleware.
- **RFC 9700 refresh token rotation**: Deferred. Mitigated by reducing default refresh TTL from 30d → 8h. The 30d non-rotating tokens are a normative MUST violation per RFC 9700 for public clients.

## References

- [lab PR #51](https://github.com/jmagar/lab/pull/51) — lab-auth parameterization
- [syslog-mcp PR #17](https://github.com/jmagar/syslog-mcp/pull/17) — OAuth integration
- [syslog-mcp PR #18](https://github.com/jmagar/syslog-mcp/pull/18) — scope bug fixes
- [axon PR #76](https://github.com/jmagar/axon/pull/76) — axon OAuth
- [axon PR #77](https://github.com/jmagar/axon/pull/77) — rmcp allowed_hosts fix
- [MCP Authorization Spec (2026)](https://modelcontextprotocol.io/specification/draft/basic/authorization)
- [RFC 9700 — OAuth 2.0 Security BCP](https://www.rfc-editor.org/rfc/rfc9700.html)
- [RFC 8707 — Resource Indicators](https://www.rfc-editor.org/rfc/rfc8707.html)
- `docs/internal/rmcp-auth-spike.md` — rmcp extension propagation spike results

## Open Questions

- **Refresh token rotation**: RFC 9700 MUST for public clients. Currently 8h non-rotating. Needs a follow-up bead in lab-auth.
- **`/auth/login` mounted on syslog-mcp**: The full `router()` mounts `/auth/login` (browser session flow). syslog-mcp doesn't wire the cookie middleware so it's a dead endpoint — but it's reachable at `https://syslog.example.internal/auth/login`. Low risk but inconsistent.
- **axon vendor drift**: axon's vendored lab-auth will fall behind upstream. No automation tracks this. Ideally resolve the rusqlite conflict upstream.

## Next Steps

**Started but not fully complete:**
- axon OAuth flow still not end-to-end verified (host allowlist fix deployed but live client test interrupted)

**Follow-on tasks not yet started:**
- Merge axon PR #77 fixes back into main and rebuild deployed binary with final version
- Set up SWAG proxy for `axon.example.internal` `/mcp` — currently serves but the axon binary rebuilds need to be redone from main (not the worktree)
- Add `axon:write` scope enforcement in axon's action dispatch (currently only `axon:read`/`axon:write` mapped for known actions)
- RFC 9700 refresh token rotation — create a follow-up bead in lab-auth
- Publish `lab-auth` to crates.io (deferred; currently git+rev dep)
- Resolve axon/syslog-mcp `libsqlite3-sys` version conflict at source (upgrade sqlx-sqlite or rusqlite)
