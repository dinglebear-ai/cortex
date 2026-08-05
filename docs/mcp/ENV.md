---
title: "Environment Variable Reference -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Environment Variable Reference -- cortex

Concise reference. See [CONFIG.md](../CONFIG.md) for full documentation including config.toml overlay and validation rules.

## Deployment paths

`cortex serve mcp` runs as an HTTP MCP server because it needs persistent syslog UDP/TCP listeners:

| Path | How | Credentials |
|------|-----|-------------|
| **Plugin** | Claude Code connects via HTTP; server mode delegates setup to `cortex setup repair` / `cortex setup deploy local` | `${user_config.*}` in `.mcp.json`; setup writes `~/.cortex/.env` |
| **One-line installer** | `curl .../install.sh \| sh` then `cortex setup` | `~/.cortex/.env` |
| **Docker** | `docker compose up -d` | `.env` file |
| **Bare metal** | `cargo run --release -- serve mcp` or `cortex serve mcp` | `config.toml` or env vars |

`cortex mcp` is a query-only local child process mode for stdio MCP clients. It uses `CORTEX_DB_PATH` and logging variables, but does not require `CORTEX_TOKEN` and does not bind network ports.

Direct CLI commands such as `cortex search`, `cortex tail`, and `cortex stats`
use the same query-only runtime and the same `CORTEX_DB_PATH`. Installed
CLI commands automatically load `$CORTEX_HOME/.env` or
`~/.cortex/.env` when present, while explicit process environment variables
still win. They are not MCP transports and do not use `CORTEX_TOKEN`.

## Syslog listener

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_RECEIVER_HOST` | no | `0.0.0.0` | Listen host for UDP+TCP syslog | no |
| `CORTEX_RECEIVER_PORT` | no | `1514` | Listen port (shared UDP and TCP) | no |
| `CORTEX_MAX_MESSAGE_SIZE` | no | `8192` | Max message size in bytes | no |
| `CORTEX_BATCH_SIZE` | no | `100` | Entries per batch flush | no |
| `CORTEX_FLUSH_INTERVAL` | no | `500` | Batch flush interval in ms | no |
| `CORTEX_WRITE_CHANNEL_CAPACITY` | no | `10000` | Internal parsed-message queue capacity | no |

## MCP server

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_HOST` | no | `0.0.0.0` | HTTP bind address | no |
| `CORTEX_PORT` | no | `3100` | HTTP listen port | no |
| `CORTEX_TOKEN` | no | (none) | Bearer token for `/mcp`. Generate: `openssl rand -hex 32` | **yes** |
| `CORTEX_ALLOWED_HOSTS` | no | (none) | Extra comma-separated Host header values for RMCP Host validation | no |
| `CORTEX_ALLOWED_ORIGINS` | no | (none) | Extra comma-separated browser origins for RMCP Origin validation | no |

## Storage

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_DB_PATH` | no | `/data/cortex.db` | SQLite database file path | no |
| `CORTEX_POOL_SIZE` | no | `8` | Connection pool size; reads get `pool_size - 1` permits | no |
| `CORTEX_SQLITE_PAGE_CACHE_MB` | no | `128` | Total SQLite page-cache budget across the pool | no |
| `CORTEX_SQLITE_MMAP_MB` | no | `256` | Bounded SQLite mmap size; resident mapped pages may still count toward cgroup memory | no |
| `CORTEX_HEAVY_READ_CONCURRENCY` | no | `1` | Shared service-layer limiter for SQLite-heavy reads | no |
| `CORTEX_WAL_CHECKPOINT_MB` | no | `256` | WAL size threshold for bounded PASSIVE checkpoint attempts | no |
| `CORTEX_RETENTION_DAYS` | no | `90` | Days before automatic purge (0 = forever) | no |

## Storage budget

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_MAX_DB_SIZE_MB` | no | `1024` | Soft DB size limit in MB (0 = disable) | no |
| `CORTEX_RECOVERY_DB_SIZE_MB` | no | `900` | Cleanup target after DB-size breach | no |
| `CORTEX_MIN_FREE_DISK_MB` | no | `0` | Min free disk in MB (0 = disable) | no |
| `CORTEX_RECOVERY_FREE_DISK_MB` | no | `0` | Recovery threshold after free-disk breach (0 = disabled with min-free guard) | no |
| `CORTEX_CLEANUP_INTERVAL_SECS` | no | `60` | Enforcement check interval in seconds | no |
| `CORTEX_CLEANUP_CHUNK_SIZE` | no | `2000` | Rows deleted per chunk (1 to 1,000,000) | no |

## Docker log ingest

Current deployments use the host-local cortex agent, which streams Docker logs
from each host's local Docker socket. The variables below are the legacy central
pull compatibility mode for explicit remote Docker Engine HTTP endpoints.

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_DOCKER_INGEST_ENABLED` | no | `false` | Enable legacy central pull Docker log ingestion from remote Docker-compatible HTTP endpoints | no |
| `CORTEX_DOCKER_HOSTS` | yes, if Docker ingest is enabled | (none) | Comma-separated hostnames — each becomes `http://<host>:2375` (e.g. `squirts,tootie`) | no |
| `CORTEX_DOCKER_RECONNECT_INITIAL_MS` | no | `1000` | Initial reconnect delay after host stream failure | no |
| `CORTEX_DOCKER_RECONNECT_MAX_MS` | no | `30000` | Maximum reconnect delay after repeated failures | no |

Hosts specified via `CORTEX_DOCKER_HOSTS` default to plain `http://` on port 2375 — use only on trusted private networks or behind firewall/TLS controls. Older setups may point these at docker-socket-proxy, but the deployed agent path is preferred.

## Logging

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `RUST_LOG` | no | `info` | Tracing filter directive (e.g. `debug`, `cortex=trace`) | no |

## Docker / container

| Variable | Required | Default | Description | Sensitive |
| --- | --- | --- | --- | --- |
| `CORTEX_UID` | no | `1000` | Container user ID | no |
| `CORTEX_GID` | no | `1000` | Container group ID | no |
| `CORTEX_FILE_TAIL_GROUP` | no | `adm` | Supplementary container group for reading group-owned mounted log files | no |
| `CORTEX_CONFIG_VOLUME` | no | `./config` | Read-only config mount for optional config files | no |
| `DOCKER_NETWORK` | no | `cortex` | External Docker network name | no |

## Token generation

```bash
openssl rand -hex 32
```

Store the result in `CORTEX_TOKEN` in your `.env` file.

## See also

- [AUTH.md](AUTH.md) -- how tokens are used for authentication
- [TRANSPORT.md](TRANSPORT.md) -- transport-specific variable usage
- [../CONFIG.md](../CONFIG.md) -- full configuration reference with validation rules
