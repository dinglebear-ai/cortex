---
title: "Cortex Security Model"
created: 2026-06-07
updated: 2026-07-30
---

# Cortex Security Model

This document collects operator-facing trust assumptions that are otherwise
spread across the code and setup docs.

## Surfaces

| Surface | Default | Trust boundary |
| --- | --- | --- |
| Syslog UDP/TCP `:1514` | unauthenticated | any reachable sender can submit frames; restrict by bind address, firewall, or `CORTEX_ALLOWED_SOURCE_CIDRS` |
| MCP HTTP `/mcp` | bearer auth when `CORTEX_TOKEN` is set | `cortex:read` for read actions, `cortex:admin` for write/admin actions |
| OAuth/JWT | disabled unless `CORTEX_AUTH_MODE=oauth` | Google identity plus the configured cortex allowlist; static token is disabled by default in OAuth mode |
| OTLP `/v1/logs` | loopback or bearer-token protected | OAuth JWTs do not authorize OTLP ingest today |
| Docker ingest | host-local agent for current deployments; legacy pull disabled unless configured | trust the deployed host-local cortex agent and its local Docker socket access; legacy central pull endpoints must stay private/read-only |
| SSH inventory/deploy | disabled unless hosts are configured | inventory and remote Docker events use validated host aliases, strict host keys, shared concurrency limits, and retry backoff; deploy uses the same host validation, `--` delimiter, and host-key argument policy |

## MCP Bind Default

Docker Compose publishes the MCP/HTTP port 3100 on `127.0.0.1` by default
(`CORTEX_MCP_BIND` overrides the host interface; the in-process bind is
`CORTEX_HOST`, also loopback by default). The Labby gateway and other
containers on the same Docker network reach cortex at `http://cortex:3100`
regardless of the host publish address. The syslog ingest port 1514 stays
published wide because log senders must reach it. **If you expose port 3100
beyond loopback (`CORTEX_MCP_BIND=0.0.0.0` or `CORTEX_HOST=0.0.0.0`), set
`CORTEX_TOKEN`** or configure OAuth; startup validation rejects an
unauthenticated non-loopback bind.

## Auth Scopes

The current public scopes are `cortex:read` and `cortex:admin`.
`cortex:admin` satisfies `cortex:read`. Static bearer tokens receive
`cortex:read` by default; set `CORTEX_STATIC_TOKEN_ADMIN=true` only for
operators that need `ack_error`, `unack_error`, or `notifications_test`.

## TrustedGatewayUnscoped Mode

Setting `CORTEX_NO_AUTH=true` together with
`CORTEX_TRUSTED_GATEWAY_NO_AUTH=true` enables the TrustedGatewayUnscoped
policy: it disables **both** authentication **and** the read/admin scope
gates. Every request that reaches the port can run every action — including
the write actions `ack_error`, `unack_error`, and `notifications_test` — with
no token and no scope check.

This mode exists for one topology only: an upstream gateway (e.g. Labby) that
authenticates callers itself and reaches cortex over a private Docker network.

**NEVER combine TrustedGatewayUnscoped with host-published ports.** If port
3100 is published beyond loopback while this mode is active, every host that
can reach the port has full unauthenticated read *and* write access. Keep
`CORTEX_MCP_BIND=127.0.0.1` (the default), or do not use this mode at all —
a 401 behind a reverse proxy is a configuration problem to fix with
`CORTEX_TOKEN`, not a reason to disable auth.

## SSH Key Exposure (Inventory Mount)

The Compose files mount an SSH key directory read-only into the container at
`/home/cortex/.ssh` so the inventory collectors can reach fleet hosts. The
default source directory is a **dedicated key dir, `~/.cortex/ssh`**
(override with `CORTEX_SSH_VOLUME`).

**Never point `CORTEX_SSH_VOLUME` at `~/.ssh`.** Mounting your personal SSH
directory hands the container (and anything that compromises it) every
identity, agent socket config, and host alias you own — a direct
lateral-movement path across the fleet.

Provision a least-privilege deploy key instead:

```bash
# 1. Dedicated keypair, used only by cortex inventory
mkdir -p ~/.cortex/ssh && chmod 700 ~/.cortex/ssh
ssh-keygen -t ed25519 -f ~/.cortex/ssh/id_ed25519 -C "cortex-inventory" -N ""

# 2. Minimal SSH config listing only the hosts cortex should collect from
cat > ~/.cortex/ssh/config <<'EOF'
Host host-a
    HostName host-a.example.net
    User cortex-inventory
    IdentityFile ~/.ssh/id_ed25519
EOF

# 3. Curated known_hosts (no TOFU in production)
ssh-keyscan host-a.example.net > ~/.cortex/ssh/known_hosts
```

On each fleet host, restrict what the key can do in `authorized_keys`:

```
restrict,command="docker ps --format json" ssh-ed25519 AAAA... cortex-inventory
```

(Adjust the forced command to the read-only collection commands you allow; at
minimum use `restrict` to disable port/agent/X11 forwarding and PTY
allocation, and prefer a dedicated low-privilege remote user.)

Inventory SSH defaults to `StrictHostKeyChecking=yes`; point
`CORTEX_INVENTORY_SSH_KNOWN_HOSTS` at the curated file above and leave
`CORTEX_INVENTORY_SSH_TRUST_ON_FIRST_USE` unset outside of bootstrap.

## OAuth Platform Assumption

OAuth/JWT key and database file permission checks are Linux/Unix oriented.
On non-Unix platforms cortex fails closed instead of silently accepting weaker
ACL validation. Treat OAuth mode as Linux-only unless non-Unix ACL validation is
implemented and tested.

## SSH Host Keys

Inventory and deploy SSH default to `StrictHostKeyChecking=yes`. Bootstrap TOFU
requires explicit opt-in with `CORTEX_INVENTORY_SSH_TRUST_ON_FIRST_USE=true`.
Use `CORTEX_INVENTORY_SSH_KNOWN_HOSTS` to point cortex at a managed known-hosts
file for fleet automation.

## Identity Fields

Syslog `hostname` is sender-claimed. For UniFi CEF messages, the stored
`hostname` comes from `UNIFIdeviceName` in the message body. `source_ip` is the
network-observed source identifier and is the better trust boundary for
correlation and inventory decisions.

## Redaction Limits

cortex redacts known credential-looking environment keys and sensitive setup
values before persisting inventory artifacts. Redaction is defensive, not a
formal data-loss-prevention guarantee. Treat log messages, transcript text,
paths, Docker metadata keys, and source-specific `metadata_json` as sensitive
operator data.

## Dependency Exceptions

`cargo deny check` ignores `RUSTSEC-2023-0071` for transitive RSA usage through
`lab-auth -> jsonwebtoken -> rsa`. The accepted path is JWT signing and
verification, not PKCS#1 v1.5 decryption. The owner is the cortex maintainer;
review the exception every release and remove it when `lab-auth` or
`jsonwebtoken` moves to a hardened dependency path.

`cargo deny` also allows duplicate crate versions and wildcard git dependency
metadata because the current dependency graph includes transitive MCP/auth and
platform-target duplication plus a pinned `lab-auth` git revision. Source
allowlists, license policy, yanked crates, and advisory checks remain enforced.
