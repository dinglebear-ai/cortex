---
title: "OAuth Authentication"
created: 2026-05-08
updated: 2026-07-30
---

# OAuth Authentication

cortex supports Google OAuth 2.0 for MCP clients. Both bearer and OAuth modes leave `/health` unauthenticated and honour the same scope-based tool dispatch.

---

## Architecture

```
                         ┌────────────────────────────────────┐
Client (browser/Claude)  │  cortex HTTP :3100             │
                         │                                    │
  GET /authorize      ──▶  OAuth router (lab-auth)            │
  POST /token         ──▶  RS256 JWT issuance (lab-auth)      │
                         │                                    │
  POST /mcp           ──▶  AuthLayer (lab-auth middleware)    │
    Bearer JWT        ──▶    RS256 verify → AuthContext       │
    Bearer static     ──▶    constant-time compare            │
                         │                                    │
                         │  RMCP tool dispatch                │
                         │    scope check (cortex:read)       │
                         │    → SyslogService / SQLite        │
                         └────────────────────────────────────┘

OAuth discovery endpoints (mounted when AUTH_MODE=oauth):
  GET /.well-known/oauth-authorization-server
  GET /.well-known/oauth-protected-resource
  GET /jwks
  GET /authorize
  GET /auth/google/callback
  POST /token

Intentionally not mounted:
  POST /register   (dynamic client registration — disabled)
```

### Auth flow (authorization_code grant)

1. Client sends unauthenticated request to `/mcp` → receives `401 WWW-Authenticate: Bearer resource_metadata="…"`.
2. Client fetches `/.well-known/oauth-protected-resource` to discover the authorization server.
3. Client fetches `/.well-known/oauth-authorization-server` for the full metadata document.
4. Client constructs an `/authorize` URL (PKCE S256, `scope=cortex:read`), opens in browser.
5. User authenticates with Google; Google redirects to `/auth/google/callback`.
6. Server validates the Google email against `admin_email` plus any lab-auth `allowed_users` rows, issues an RS256 access token (1h TTL) and a refresh token (8h TTL).
7. Client uses `POST /token?grant_type=refresh_token` to obtain new access tokens without re-prompting.

---

## Google Console setup

1. Go to [Google Cloud Console](https://console.cloud.google.com) → **APIs & Services** → **Credentials**.
2. Click **Create Credentials** → **OAuth client ID** → **Web application**.
3. Add an authorized redirect URI: `https://YOUR_PUBLIC_URL/auth/google/callback`.
4. Copy the **Client ID** and **Client Secret**.

---

## Configuration

### Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `CORTEX_AUTH_MODE` | yes | Set to `oauth` to activate |
| `CORTEX_PUBLIC_URL` | yes | Base URL (e.g. `https://cortex.example.com`). Sets issuer + audience. |
| `CORTEX_GOOGLE_CLIENT_ID` | yes | From Google Console |
| `CORTEX_GOOGLE_CLIENT_SECRET` | yes | From Google Console |
| `CORTEX_AUTH_ADMIN_EMAIL` | yes | Bootstrap allowed Google account |
| `CORTEX_AUTH_ALLOWED_REDIRECT_URIS` | no | Comma-separated non-loopback OAuth client callbacks, such as a Codex callback URL |
| `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH` | no | Defaults to `true`; set `false` to keep `CORTEX_TOKEN` working while OAuth is active |

### config.toml `[mcp.auth]` fields

These are **not** env vars — they go in `config.toml`:

```toml
[mcp.auth]
mode = "oauth"
public_url = "https://cortex.example.com"
google_client_id = "..."         # overridden by CORTEX_GOOGLE_CLIENT_ID
google_client_secret = "..."     # overridden by CORTEX_GOOGLE_CLIENT_SECRET

# Bootstrap config-backed OAuth email
admin_email = "you@example.com"

# Reserved for future config-backed multi-user enforcement. Do not set in OAuth
# mode today: startup rejects non-empty allowed_emails until cortex passes
# or enforces that list.
allowed_emails = []

# File paths (relative to the syslog DB directory)
sqlite_path = "auth.db"
key_path = "auth-jwt.pem"

# Token TTLs
access_token_ttl_secs = 3600    # 1h (default)
refresh_token_ttl_secs = 28800  # 8h (default; lab-auth default is 30d)

# Set false to keep static CORTEX_TOKEN as break-glass when OAuth is active
disable_static_token_with_oauth = true   # default: true
```

---

## Gotchas

- **Refresh token TTL is 8h**, not lab-auth's default of 30d. This suits the read-only homelab profile. Adjust via `[mcp.auth].refresh_token_ttl_secs`.
- **`admin_email` is required**. It is the only config-backed OAuth email gate cortex passes into lab-auth today. lab-auth also honors rows in its `allowed_users` table. Startup rejects OAuth configs with a blank `admin_email`, and also rejects non-empty config-level `allowed_emails` until cortex can pass or enforce that list.
- **`disable_static_token_with_oauth` defaults to `true` for `/mcp`**. OAuth-mode `/mcp` rejects `CORTEX_TOKEN` by default. Set `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false` or `disable_static_token_with_oauth = false` in config.toml for break-glass bearer access.
- **Non-loopback OAuth deployments still need `CORTEX_TOKEN` for OTLP `/v1/logs` unless OTLP exposure is loopback-only or service auth is explicitly disabled behind an upstream auth layer.** OTLP ingest does not accept OAuth JWTs today.
- **OAuth file-permission checks are Unix-oriented and fail closed on non-Unix platforms.** Run OAuth mode on Linux/Unix until cortex grows audited non-Unix ACL validation.
- **Stdio mode always uses LoopbackDev**. `cargo run -- mcp` ignores the auth config entirely — no credentials are needed or enforced.
- **Docker bind-mount ownership**. `auth.db` and `auth-jwt.pem` are written by the container UID. Host-side backup scripts may need `sudo` or a sidecar copy step.
- **`/register` is never mounted**. cortex supports authorization-code OAuth routes but disables dynamic client registration.
- **RFC 9700 refresh-token rotation** is not yet implemented. The same refresh token is returned on each `POST /token?grant_type=refresh_token` call. This is tracked as known debt in CHANGELOG.md.

---

## Operator FAQ

**How do I revoke a user's access?**

For the configured `admin_email`, replace `admin_email` with the remaining authorized account and restart. Future authorization attempts by the removed account will fail at the callback unless the email is still present in lab-auth's `allowed_users` table.

Also remove any DB allowlist row and existing refresh/browser-session rows. `refresh_tokens` is keyed by Google subject, not email, so derive the subject from `browser_sessions` first:

```sql
BEGIN;
DELETE FROM allowed_users WHERE lower(email) = lower('user@example.com');
DELETE FROM refresh_tokens
WHERE subject IN (
  SELECT subject FROM browser_sessions WHERE lower(email) = lower('user@example.com')
);
DELETE FROM browser_sessions WHERE lower(email) = lower('user@example.com');
COMMIT;
```

**How do I rotate the JWT signing key?**

```bash
# Stop the server, replace the key file, restart
docker compose down
rm /data/auth-jwt.pem   # or the path from key_path in config.toml
docker compose up -d    # server generates a new key on first boot
```

All existing access tokens become invalid immediately (they reference the old `kid`). Users must re-authenticate. Refresh tokens in the DB are also invalidated because new tokens issued with the new key will not verify against old JWTs held by clients.

**How do I add a new allowed user without restarting?**

Not through cortex config in V1. Config-level OAuth user changes are restart-only, and non-empty `[mcp.auth].allowed_emails` is rejected at startup because cortex does not pass or enforce that config list yet. lab-auth-managed `allowed_users` rows are still part of the enforced OAuth allowlist.

**How do I check which emails are currently allowed?**

Inspect the configured `[mcp.auth].admin_email` value in `config.toml` or the `CORTEX_AUTH_ADMIN_EMAIL` environment variable, then inspect lab-auth's DB allowlist:

```sql
SELECT email, created_at FROM allowed_users ORDER BY created_at;
```

---

## Runtime model

The auth middleware (`lab_auth::AuthLayer`) runs on every `/mcp` request:

- **Static token**: constant-time string compare — O(1), no DB access.
- **JWT**: stateless RS256 verify — ~250µs per request, no DB access, no I/O.
- **JWKS fetch**: bounded 5s timeout; result cached in `AuthState`; no per-request fetch.

The tokio runtime is shared between the auth middleware, RMCP handler, syslog ingest, and DB writer. Auth does not write to any DB in the hot path. Under auth burst, the bottleneck is the RSA verify, not the database.
