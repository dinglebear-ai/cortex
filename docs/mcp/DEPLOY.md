# Deployment guide

Use [SETUP.md](../SETUP.md) for the full setup sequence and the root [README.md](../../README.md) for supported native/npm installation. This guide separates a development process, an installed server, and a query-only client.

## Local source development

Build with the checked-in Rust toolchain and a locked dependency graph:

```bash
cargo build --locked
```

Before starting `cargo run -- serve mcp`, configure a writable `CORTEX_DB_PATH` and the required `CORTEX_API_TOKEN`. The MCP bind defaults to loopback; a non-loopback listener must satisfy the configured auth policy. The default database path is container-oriented, so do not assume a fresh host can write `/data/cortex.db`. Follow the environment setup in [SETUP.md](../SETUP.md).

The root package has `publish = false`, so `cargo install cortex` is not the supported installation path. An intentional source installation uses `cargo install --path . --locked`.

## Client versus server

`cortex serve mcp` starts the server, ingest listeners, and maintenance. `cortex mcp` is a local query-only stdio process: it reads the configured SQLite database and does not start a second receiver or HTTP service. A client-only host should connect to its authoritative server instead of accidentally creating another ingestion/database owner.

## Managed installation

The installer and `cortex setup` own the managed `~/.cortex` layout: runtime environment, Compose assets, persistent data, and the selected server profile. Start with read-only diagnostics:

```bash
cortex setup check
cortex compose doctor
cortex compose status --json
cortex update --dry-run
```

`cortex setup repair`, non-dry-run deployment/update, and Compose lifecycle operations mutate the installation. Confirm the target and obtain explicit operational authorization first. Preserve existing credentials; a documentation audit is not authorization to restart the server.

For deployment-specific options and host layouts, use [SETUP.md](../SETUP.md), [deploy/README.md](../../deploy/README.md), and the command's help. Run the relevant preflight/dry-run before a remote deployment. Client-agent credential preservation and first-time bootstrap are separate from server publication.

## Containers

The source-build definition is [`config/Dockerfile`](../../config/Dockerfile), selected by [`docker-compose.yml`](../../docker-compose.yml). It uses a Rust 1.97.1 builder and a Debian bookworm-slim runtime, with the binary plus required runtime tools and backup helper. The runtime runs as UID/GID 1000 and uses `CMD ["cortex", "serve", "mcp"]`; there is no separate shell entrypoint to maintain.

```bash
just docker-build
# Equivalent source build:
docker build -f config/Dockerfile -t cortex .
```

[`docker-compose.prod.yml`](../../docker-compose.prod.yml) uses the release image. Use the checked-in or installed Compose definition rather than a copied YAML fragment from an old guide. Verify persistent bind paths/ownership, token configuration, published interfaces, and resource limits for the selected project. `config.toml` is not copied into the image; defaults and configured environment apply.

The current Docker log path is the host-local cortex agent. The legacy central pull path is compatibility-only for explicit remote Docker Engine HTTP endpoints; see [architecture.md](../architecture.md).

## Ports and trust

Syslog defaults to UDP/TCP 1514. HTTP defaults to TCP 3100 and hosts MCP, REST, OTLP, agent ingest, health, and `/app`. These surfaces do not all use the same credential. Review [SECURITY.md](../SECURITY.md), [OAUTH.md](../OAUTH.md), and [CONFIG.md](../CONFIG.md) before remote exposure.

MCP uses stateless JSON responses on `POST /mcp`, not a persistent SSE endpoint. A reverse proxy must preserve the intended authentication and Host/Origin policy; do not weaken it or blindly reuse a historical proxy configuration.

## Verification

Check health and ownership, then the relevant bounded ingest/query evidence. A built or pulled image is not proof of correct deployment. Use [LIVE_QUALIFICATION.md](../LIVE_QUALIFICATION.md) for isolated profiles and separately authorized fleet checks. Production reset, restore, and release are distinct operations.
