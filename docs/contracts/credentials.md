---
title: "Credentials Inventory"
created: 2026-05-16
updated: 2026-07-30
---

# Credentials Inventory

**Status:** Contract — source of truth
**Date:** 2026-05-16
**Pinning header:**

> Contract derived from cross-cutting audit of `src/config.rs`, all six Epic
> specs in `docs/superpowers/specs/`, and the current production deployment.
> Supersedes scattered references to secret env vars throughout the specs.
> Changing this requires updating `src/config.rs`, `src/setup.rs`,
> redaction policy, and (when applicable) `docs/contracts/agent-protocol.md`.

---

## 1. Threat Model

cortex is a single-tenant homelab service. It sits behind SWAG (an nginx
reverse proxy that terminates TLS) on the `jakenet` Docker network. Secrets
live in two places only: the operator's filesystem (env files, mounted
volumes) and the cortex process address space at runtime. We do **not**
integrate with a secret manager in V1 — no Vault, no SOPS, no AWS/GCP secret
APIs, no envelope encryption at rest. The operator owns rotation discipline.

The expected attacker model is:

- **Tailnet-adjacent.** Anyone on the operator's tailnet can reach the syslog
  TCP/UDP listener (1514) and (depending on bind) the MCP HTTP port (3100).
  Static bearer tokens defend the MCP/API path; the syslog listener itself
  is unauthenticated by design (it speaks RFC 3164/5424 on a private LAN).
- **No active intra-host attacker.** If an attacker gets root on the host
  running cortex, secrets at rest and in memory are forfeit. The
  rotation primitives below assume this scenario as the recovery path, not
  the prevention path.
- **Logs are sensitive.** Logs may contain prompt text, IPs, usernames, and
  occasional secrets pasted into AI sessions. The `scrub_prompts` enrichment
  pass (default on) and the redaction discipline in §6 are the mitigations.

V1 explicitly accepts: env-var-leak-via-process-listing (any UID on the host
running the container can read `/proc/<pid>/environ`), config-toml-in-git
(operator discipline; we reject TOML files that contain known secret keys),
and key-file-perms-bit-rot (startup re-checks `0600` on every boot).

---

## 2. Inventory

One row per distinct secret. **Sensitivity tiers:**

- `high` — process compromise on cortex (read/write the entire log
  corpus, mint JWTs, ingest forged logs, exfil prompt text).
- `medium` — service compromise of an upstream that cortex polls
  (read-only UniFi/AdGuard/Apprise access in the operator's blast radius).
- `low` — log-only / observability (used to authenticate one specific feed;
  loss does not grant arbitrary read of cortex data).

| # | Name | Purpose | Env var | File path fallback | File mode | Owning epic / introduced | Sensitivity | Rotation procedure | Logging policy |
|---|------|---------|---------|--------------------|-----------|--------------------------|-------------|---------------------|----------------|
| 1 | **MCP bearer token** | Authenticates `/mcp` JSON-RPC and `/v1/logs` OTLP HTTP writes. Sole gate when `auth_mode=bearer`. | `CORTEX_TOKEN` (legacy alias `CORTEX_API_TOKEN`, logged as deprecated) | none — env-only | n/a | Existing (pre-V1, current prod secret) | `high` | Edit env (`~/.cortex/.env` or compose), SIGTERM, restart. Window of acceptance ≈ container restart latency. No grace window — old token is rejected the instant the new process is up. | Never logged. The two log sites that touch the token redact to prefix-only (`token=xxxxxx…`); see §6. |
| 2 | **OAuth Google client ID** | Public-ish OAuth identifier. Not a secret strictly speaking, but co-located with the secret and treated as such for operational discipline. | `CORTEX_GOOGLE_CLIENT_ID` | none | n/a | Existing (when `auth_mode=oauth`) | `low` | Regenerate in Google Cloud Console; update env; restart. | Not redacted (public value). |
| 3 | **OAuth Google client secret** | OAuth confidential client secret. Required when `[mcp.auth].mode = "oauth"`. | `CORTEX_GOOGLE_CLIENT_SECRET` | none | n/a | Existing (`src/config.rs::AuthConfig::google_client_secret`) | `high` | Generate new secret in Google Cloud Console, update env, restart. Existing JWTs continue to validate (they're signed by us, not Google), but new logins require the new secret. | Never logged. Redaction enforced by `lab-auth` middleware. |
| 4 | **JWT signing private key** | RSA/Ed25519 PEM used to sign JWT access + refresh tokens. Verifying party is also us; this is symmetric trust on a per-process basis. | none — file-only | `<data_dir>/auth-jwt.pem` (relative path resolved against `[storage].db_path` dir) | `0600` | Existing (`src/config.rs::AuthConfig::key_path`) | `high` | `rm auth-jwt.pem`, restart (regenerates on first boot). **Side effect:** every issued access + refresh token is invalidated; all OAuth users must re-login. This is the documented "kill all sessions" effect — see §5. | Key material never logged. Path is logged at INFO. |
| 5 | **Non-MCP API token** | Bearer for the optional `[api]` JSON API (separate from `/mcp`). Required when `CORTEX_API_ENABLED=true`. | `CORTEX_API_TOKEN` | none | n/a | Existing (`src/config.rs::ApiConfig::api_token`) | `high` | Same as MCP token — env edit + restart. Validation rejects empty tokens at startup. | Never logged; same prefix-only redaction discipline as the MCP token. |
| 6 | **OTLP token (logical alias of MCP token)** | Authenticates `/v1/logs` OTLP HTTP ingestion. **In V1, this is the same token as `CORTEX_TOKEN`.** The OTLP path only honors the static bearer; the OAuth path does not gate OTLP. The non-loopback safety gate in `validate_auth_config` enforces this explicitly. | `CORTEX_TOKEN` (same var) | none | n/a | Existing | `high` | Same as MCP token. If split into a dedicated `CORTEX_OTLP_TOKEN` later, add a new row here and bump this contract. | Never logged. |
| 7 | **Agent enrollment token (one-time)** | One-shot bearer printed by `cortex agent issue --host <h>`. Operator pastes it into the agent host's `/etc/syslog-agent/token`. Server stores only `BLAKE3(token)` in `agents.token_hash` and never the raw value. | none — printed once to stdout by admin CLI | (ephemeral; never persisted server-side in plaintext) | n/a | Epic A — agent mode (`docs/superpowers/specs/2026-05-16-agent-mode-design.md` §6.2) | `medium` | Single use. If the operator loses it before the agent enrolls, revoke (`cortex agent revoke --host-id <uuid>`) and re-issue. | Server: token hash only; raw token never written to disk or logs. Printed once on the admin CLI stdout — operator owns transport (typically scp/paste). |
| 8 | **Agent long-lived token** | After successful enrollment, the agent's persistent bearer used on every reconnect. Sent in the first JSON-RPC `agent.hello.params.token` message; never in URL params or headers. | none — file-only | `/etc/syslog-agent/token` on the agent host (or `~/.config/cortex/agent-token` for user installs) | `0600` | Epic A — agent mode (§6.2) | `medium` | `cortex agent rotate --host-id <uuid>` issues a new token. Server keeps both `token_hash` (new) and `token_hash_prev` (old) for `rotation_grace_secs = 300` seconds (5 min, from spec §6.2). The agent receives the new token on its next reconnect (delivered via `agent.shutdown` payload); after the grace window the old hash is dropped. | Never logged. `HelloParams` overrides `Display`/`Debug` to redact `token`; test-verified per spec §10. |
| 9 | **UniFi controller API key** | `X-API-KEY` header for read-only access to `/proxy/network/api/s/<site>/{stat/event,stat/alarm}`. Issued in the UniFi OS console UI. | `CORTEX_POLLERS_UNIFI_API_KEY` | `~/.cortex/.env` line `CORTEX_POLLERS_UNIFI_API_KEY=…` (loaded by `load_setup_env_file` if not already in process env; symlinks rejected) | `0600` on the env file | Epic C — API pollers (`docs/superpowers/specs/2026-05-16-api-pollers-design.md` §4) | `medium` | Revoke in UniFi OS admin → System → Application UI → Admins → API keys; issue replacement; update env; restart (or send `SIGHUP` once dynamic reload lands — currently restart-only). | Never logged. UniFi poller `Debug` impls redact the key. |
| 10 | **AdGuard Home credentials** | HTTP Basic auth for `/control/querylog`. Token-based auth is not exposed by AdGuard. | `CORTEX_POLLERS_ADGUARD_USERNAME` + `CORTEX_POLLERS_ADGUARD_PASSWORD` (separate vars, not the `user:pass` form the original draft contemplated — verified against `api-pollers-design.md` §5 lines 243–244) | `~/.cortex/.env` | `0600` on the env file | Epic C — API pollers (§5) | `medium` | Change in AdGuard Home admin UI; update env; restart. AdGuard does not support overlapping credentials, so rotation is a hard cutover — operator should expect one missed poll cycle. | Username may be logged at debug; password never logged. |
| 11 | **Apprise API token** | Optional `X-Apprise-Token` header on outbound POSTs to `apprise-api`'s `/notify/{config_key}` endpoint. Only needed when the operator's apprise-api is auth-protected. | `CORTEX_APPRISE_TOKEN` | none — env-only | n/a | Epic E — digest + notifications (`docs/superpowers/specs/2026-05-16-digest-notifications-design.md` §3) | `low` | Rotate in the apprise-api admin; update env; restart. Loss only affects outbound notification delivery — no read access to logs. | Never logged. |
| 12 | **Reserved LLM/RAG API key** | Reserved for the historical `suggest_fix` / semantic `ask_history` design. The current `ask_history` implementation is SQLite FTS5 only and does not call an LLM or Axon. | n/a | none | n/a | Historical Epic F design | n/a today | No rotation needed until an LLM-backed action lands. | n/a |

### Auxiliary identity values (not secrets, listed for completeness)

These are co-located with the secrets above and operators frequently confuse
them. They are **not** redacted and **are** safe to log.

| Name | Env var | Purpose |
|------|---------|---------|
| Bootstrap admin email | `CORTEX_AUTH_ADMIN_EMAIL` | Single Google account permitted to log in via OAuth. This is the only enforced OAuth email gate in V1. |
| OAuth public URL | `CORTEX_PUBLIC_URL` | Externally reachable base URL for issuer/audience derivation. |
| OAuth allowed redirect URIs | `CORTEX_AUTH_ALLOWED_REDIRECT_URIS` | Non-loopback redirect URIs accepted by lab-auth (loopback is implicit). |
| Authelia source-IP gate | `CORTEX_AUTHELIA_SOURCE_IP` | Optional prefix that gates Authelia severity reclassification (anti-spoof). |
| AdGuard source-IP gate | `CORTEX_ADGUARD_SOURCE_IP` | Same gating for the AdGuard parser tag classification. |

---

## 3. Historical policy: LLM API key for `suggest_fix` / semantic `ask_history`

The current production `ask_history` action is deterministic SQLite FTS5 over
AI transcript rows. It returns matching sessions and nearby non-AI log context;
it does not call an LLM, Axon, or Qdrant.

The policy below is retained as historical design context for a future
LLM-backed action. It is not active runtime behavior today.

The RAG spec (§7.5) confirmed `axon ask` does its own retrieval and synthesis;
cortex does not invoke the model directly. **However, axon itself needs
an API key** to call the model. Three deployment realities apply:

1. The user runs a local axon instance on this host (same homelab). axon's
   own config holds the API key. cortex does not need its own.
2. The user wants cortex to call a different model than axon's default
   (e.g. Anthropic for diagnostic narrative quality, while axon uses an
   embeddings-only path). cortex needs its own credential.
3. The user has no LLM access at all. `suggest_fix` and `ask_history` must
   degrade gracefully.

### Decision (locked V1)

cortex supports **two modes** with explicit precedence:

| Mode | When active | Env vars | Behavior |
|------|-------------|----------|----------|
| **A — Anthropic native (default)** | `CORTEX_RAG_ANTHROPIC_API_KEY` is set | `CORTEX_RAG_ANTHROPIC_API_KEY` | cortex's `suggest_fix` / `ask_history` wraps `axon ask` and additionally passes the Anthropic key for Claude synthesis. Implicit model: claude-sonnet (operator may override via `CORTEX_RAG_MODEL`). |
| **B — OpenAI-compatible (Ollama, vLLM, openai.com, openrouter, …)** | `CORTEX_RAG_LLM_BASE_URL` is set | `CORTEX_RAG_LLM_BASE_URL` + `CORTEX_RAG_LLM_API_KEY` (+ optional `CORTEX_RAG_MODEL`) | cortex issues OpenAI-compatible chat completions against the given base URL. Covers self-hosted LLMs with no API key (set key to `"-"` or any non-empty placeholder for Ollama). |
| **Disabled** | Neither Mode A nor Mode B configured | n/a | `suggest_fix` returns `synthesis_unavailable` error; `ask_history` falls back to `similar_incidents` ranked-hits payload with `synthesized: false, reason: "no llm configured"`. |

### Precedence

**Mode A wins if both are set.** Rationale: Anthropic-native synthesis is the
recommended path; an operator who sets the Anthropic key explicitly is
overriding any prior Mode B config and we honor that intent. An explicit
override is available via `CORTEX_RAG_LLM_PROVIDER = anthropic | openai`
when an operator needs to force a path; otherwise the implicit precedence
applies.

### Rotation

- **Mode A:** rotate in `console.anthropic.com`, update env, restart.
- **Mode B:** rotate at the provider (openai.com dashboard, openrouter
  dashboard, etc.) or — for self-hosted Ollama/vLLM where the key is
  cosmetic — rotate the placeholder string and restart.

### Logging

The API key is never logged. The wrapper code in `src/app/rag.rs` (when it
lands) MUST strip the `Authorization` header from any reqwest error before
emitting it to tracing, and MUST redact the key from any error surfaced back
to the MCP caller via the JSON-RPC error body. Verified by a unit test on
the redaction helper (spec §10 of the RAG epic).

---

## 4. File location and storage policy

1. **Env-var-first.** For every secret that supports both env var and file
   path, the env var wins. Process-env overrides file content; this is
   verified by `load_setup_env_file` (`src/config.rs::load_setup_env_file`).
2. **`~/.cortex/.env` discipline.** The local env file is parsed at
   startup by `load_setup_env_file`. Symlinks are refused (symlink-attack
   mitigation; see `symlink_metadata` check). Keys are filtered by
   `is_supported_setup_env_key` to a closed set of `CORTEX_*` prefixes.
3. **File mode 0600 enforced.** All file-backed secrets (`auth-jwt.pem`,
   `/etc/syslog-agent/token`, `~/.cortex/.env`) MUST be `0600`. The
   server-side files (JWT key, agent enrollment env file) are perm-checked
   on startup and the process exits with a clear error if perms are wrong.
   (The agent-side token file is checked by `syslog-agent`, not this
   process.) See §10 of `docs/superpowers/specs/2026-05-16-agent-mode-design.md`.
4. **No secrets in `config.toml`.** The operator may commit `config.toml`
   to git for non-secret deployment config. Secret keys listed in §2 MUST
   come from env or env-file, not from `config.toml`. Enforcement is by code
   review + this contract in V1; a TOML-level validation pass that rejects
   known secret keys is planned for V1.1 (secrets-hardening lane).
5. **No secrets in compose `environment:` blocks committed to git.** Operators
   who commit `docker-compose.yml` must use `env_file: ~/.cortex/.env`,
   not inline `environment:` entries. The plugin setup hook honors this.

---

## 5. Rotation discipline (normative)

For each tier, this is the canonical procedure.

### MCP bearer token (`CORTEX_TOKEN`)

1. Edit env (`~/.cortex/.env` or compose env_file).
2. `docker compose restart cortex` (or systemd equivalent).
3. Restart latency is the window of acceptance — once the new process binds,
   the old token is rejected. No grace window. All MCP/OTLP clients must
   refresh in lockstep.

For zero-downtime rotation, operators must either (a) reissue with the same
value (no rotation), or (b) accept a brief reject window. There is no
two-token blue/green primitive in V1.

### JWT signing key (`auth-jwt.pem`)

1. `rm <data_dir>/auth-jwt.pem` (the daemon regenerates on next boot).
2. Restart.
3. **All OAuth sessions die.** Every issued access + refresh token now
   fails signature verification; every user must complete the OAuth flow
   again. This is the documented "kill all sessions" effect and is the
   intended primitive for "rotate after a suspected leak."

### OAuth Google client secret

1. Generate replacement in Google Cloud Console.
2. Update `CORTEX_GOOGLE_CLIENT_SECRET` in env.
3. Restart. Existing JWTs continue to validate (they're signed by us). Only
   *new* logins require the new secret. There's no kill-all-sessions side
   effect — if you want that, also rotate the JWT key per the previous
   procedure.

### Agent long-lived token

1. `cortex agent rotate <host_id>` on the server. This sets
   `token_hash_prev = token_hash` and writes a new `token_hash`. The new
   raw token is printed once to stdout — the operator delivers it to the
   agent host (e.g. via scp or paste) and writes it to `/etc/syslog-agent/token`.
2. The agent picks up the new token on its next reconnect by reading the
   updated token file. The server accepts both the old and new hashes during
   the grace window.
3. **Grace window:** `rotation_grace_secs = 300` (5 min, from agent-mode
   spec §6.2). For 5 minutes both the old and the new `token_hash` are
   accepted; after that, the old is dropped.

### Agent enrollment token (one-time)

Single use. There is no rotation — only revocation (`cortex agent
revoke <host_id>`) and re-issuance (`cortex agent issue
--host <h>`). If a token is exposed before the agent enrolls, revoke
the row and issue a new one.

### Pollers (UniFi, AdGuard, Apprise)

All three are external-service-owned. Rotate in the external service's
admin UI, update env, restart. No grace window. Operators should expect a
single missed poll cycle on AdGuard (no overlap window) and on UniFi
(rotation is a hard cutover).

### LLM API key (Mode A / Mode B)

Rotate at the LLM provider, update `CORTEX_RAG_ANTHROPIC_API_KEY` or
`CORTEX_RAG_LLM_API_KEY`, restart. `suggest_fix` may return
`synthesis_unavailable` for the duration of the restart; this is acceptable
because the action is operator-initiated and rarely time-critical.

---

## 6. Audit logging and redaction discipline

**Rule:** no secret is ever logged in full at any level. Acceptable redaction
forms:

- `token=xxxxxx…` (prefix-only, first 6 bytes max — the first bytes are
  insufficient to compromise the secret but useful to disambiguate WHICH
  token failed in multi-tenant deployments).
- `<redacted>` (placeholder).
- Field omission entirely.

**Enforcement points (current):**

- `src/mcp/auth` middleware — compares bearer hashes, never logs the bearer
  value. Errors log only `auth_failed: true` plus the request id.
- `lab-auth` middleware (when `auth_mode=oauth`) — same discipline, plus
  redacts the Google client secret from error chains.
- `src/syslog/enrichment.rs::scrub_prompts` — best-effort scrub of
  credential-shaped patterns in AI-source prompt text before insert. Default
  on (`CORTEX_SCRUB_PROMPTS=true`). Disabling is an audited config
  decision — see the env-file warning at startup.

**Enforcement points (new — must land with the epics that introduce the
secrets):**

- `src/agent/handshake.rs::HelloParams::Debug` — token field redacted
  (Epic A spec §10 explicitly mandates this and gates it with a unit test).
- `src/pollers/unifi.rs` and `src/pollers/adguard.rs` — `Debug` impls on
  config structs omit `api_key` / `password`. reqwest error rendering must
  strip `Authorization` and `Basic` headers.
- `src/app/rag.rs` LLM wrapper — `Authorization` header stripped from
  error reqwest renderings before emit; API key stripped from any JSON-RPC
  error body.

**Trace-level prefix logging is opt-in.** At `RUST_LOG=trace`, the auth path
may log `token=xxxxxx…` to help diagnose "which client" auth failures. At
INFO and below, only the binary outcome (`ok|bad_token|version`) is logged
(per Epic A spec §10, `syslog_agent_handshakes_total{result}`).

---

## 7. Compromise response procedure

If a secret leaks, the response depends on tier.

### `high` tier (MCP/API token, JWT key, OAuth client secret, LLM API key)

1. **Rotate immediately** per §5.
2. **Audit log corpus** for the rotated period: query for unexpected MCP
   actions, ingestion from unknown sources, OTLP writes outside the trusted
   tailnet range. The `audit-log` MCP action (when it lands) is the
   intended tool; for V1 grep `~/data/cortex.db` directly for the
   request-id range covering the leak window.
3. **For JWT key compromise specifically:** the kill-all-sessions side effect
   *is* the recovery — there is no token-revocation list in V1. If you need
   per-session revocation, that's a V2 lab-auth feature.
4. **For LLM API key compromise:** rotate at provider, then check provider
   usage dashboards for unexpected calls in the leak window. The leaked key
   does not grant access to cortex data; it only grants the ability to
   bill the operator's LLM account.

### `medium` tier (agent enrollment token, agent long-lived token, UniFi key, AdGuard creds)

1. **Revoke + re-issue** per §5.
2. For agent long-lived tokens: `cortex agent revoke <host_id>` immediately
   evicts the agent's active session and NULLs both token hashes. The agent
   will need to re-enroll.
   For agent enrollment tokens (one-time, pre-use): revoke the pending row
   with `cortex agent revoke <host_id>` and re-issue a fresh one-time token.
3. For poller credentials: rotate at the external service; there is no
   harm beyond loss of polling continuity.

### `low` tier (Apprise token, OAuth client ID)

1. Rotate per §5.
2. No corpus audit needed — these are outbound or public values; their loss
   does not grant access to log data.

---

## 8. Self-check

Every env var ending in `_TOKEN`, `_SECRET`, `_KEY`, or `_PASSWORD` that the
codebase or any of the six Epic specs reads is documented in §2 or §3:

- `CORTEX_TOKEN` — §2 row 1
- `CORTEX_API_TOKEN` (deprecated alias) — §2 row 1
- `CORTEX_GOOGLE_CLIENT_SECRET` — §2 row 3
- `CORTEX_API_TOKEN` — §2 row 5
- `CORTEX_POLLERS_UNIFI_API_KEY` — §2 row 9
- `CORTEX_POLLERS_ADGUARD_PASSWORD` — §2 row 10
- `CORTEX_APPRISE_TOKEN` — §2 row 11
- `CORTEX_RAG_ANTHROPIC_API_KEY` — §3 (NEW)
- `CORTEX_RAG_LLM_API_KEY` — §3 (NEW)

File-backed secrets:

- `<data_dir>/auth-jwt.pem` — §2 row 4
- `/etc/syslog-agent/token` — §2 row 8
- `~/.cortex/.env` (carrier for env-backed secrets) — §4
