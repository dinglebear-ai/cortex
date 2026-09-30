# RMCP Stdio Support Follow-Up Plan

> **Follow-up to:** [2026-05-04-rmcp-streamable-http-refactor.md](complete/2026-05-04-rmcp-streamable-http-refactor.md)
> **Purpose:** Add first-party stdio MCP support after the RMCP server/tool adapter exists, without weakening the daemon-oriented HTTP deployment.

## Goal

Support stdio MCP clients directly, while preserving the current daemon model for syslog ingestion and HTTP/RMCP access.

The target is a query-only stdio MCP surface that exposes the same seven MCP tools as HTTP:

- `search_logs`
- `tail_logs`
- `get_errors`
- `list_hosts`
- `correlate_events`
- `get_stats`
- `syslog_help`

## Research Findings

MCP stdio is a subprocess transport:

- The client launches the MCP server as a child process.
- The server reads JSON-RPC messages from stdin.
- The server writes JSON-RPC messages to stdout.
- Messages are newline-delimited.
- The server may log to stderr.
- The server must not write anything to stdout except valid MCP protocol messages.

RMCP supports this directly:

- Server-side stdio is enabled by the `transport-io` feature.
- `rmcp::transport::stdio()` returns `(tokio::io::stdin(), tokio::io::stdout())`.
- A server handler can be served with `server.serve(stdio()).await?`.
- RMCP also supports client-side child-process testing through `TokioChildProcess`.

Current repo shape already makes a stdio query adapter feasible:

- `src/lib.rs` exports `app`, `config`, `mcp`, and `runtime`.
- `RuntimeCore::load_query_only()` initializes DB/app service without syslog listeners or HTTP server.
- `LogService` owns typed query use cases.
- `src/bin/syslog-cli.rs` already proves direct query-only access works without the MCP HTTP server.

## Design Decision

Implement stdio as a separate query-only RMCP adapter, not as the default daemon process.

Recommended shape:

- Add a dedicated binary: `src/bin/cortex-stdio.rs`.
- It initializes tracing to stderr only.
- It loads `RuntimeCore::load_query_only()`.
- It constructs the same RMCP server handler used by HTTP.
- It serves that handler over `rmcp::transport::stdio()`.
- It never starts syslog UDP/TCP listeners.
- It never starts the HTTP server.
- It never starts retention/storage maintenance tasks by default.

This keeps `cortex` as the long-running daemon and gives stdio-only clients a real local MCP process.

## Non-Negotiables

- Never log to stdout in stdio mode.
- Do not start syslog listeners from a stdio child process.
- Do not start the HTTP server from a stdio child process.
- Do not require `CORTEX_TOKEN` for stdio mode; stdio is local child-process access, not network access.
- Do not duplicate MCP tool implementations between HTTP and stdio.
- Do not reintroduce hand-rolled JSON-RPC dispatch for stdio.
- Do not package the daemon binary itself as a stdio MCP server unless its stdio invocation is explicitly query-only.
- Preserve the exact same tool names, schemas, app-layer behavior, and error contract as HTTP RMCP.

## Relationship To RMCP HTTP Plan

This plan should start after these pieces exist:

- `cortex-hea.1`: RMCP dependency and compatibility harness.
- `cortex-hea.2`: RMCP Syslog server handler over shared `LogService`.

It does not need to wait for full HTTP route replacement if the RMCP handler is independently testable.

Docs and packaging changes should coordinate with:

- `cortex-hea.5`: transport docs and manifests.

## Proposed Beads

Create a follow-up epic, for example:

- `Add first-party RMCP stdio support`

Suggested child tasks:

1. Add RMCP stdio transport feature and stdio compatibility test.
2. Add query-only `cortex-stdio` binary.
3. Add child-process integration tests for stdio tools.
4. Update docs and manifests for dual HTTP + stdio support.
5. Decide package/distribution model for registry and client manifests.

## Task 1: Add Stdio Transport Feature And Harness

**Files:**

- `Cargo.toml`
- `Cargo.lock`
- RMCP compatibility tests

**Steps:**

- [ ] Ensure `rmcp` includes `transport-io`.
- [ ] If child-process tests are added in Rust, include the client-side feature needed for `TokioChildProcess`.
- [ ] Add a minimal stdio harness test using a tiny RMCP handler if production handler is not ready.
- [ ] Confirm tracing/logging in the harness writes to stderr, not stdout.
- [ ] Keep this additive; do not modify daemon startup yet.

**Verification:**

```bash
cargo check --all-targets
cargo test stdio -- --nocapture
```

**Done When:**

- RMCP stdio transport compiles.
- A test or harness documents the stdio serving pattern.
- No production daemon behavior changes.

## Task 2: Add Query-Only Stdio Binary

**Files:**

- `src/bin/cortex-stdio.rs`
- `src/mcp/rmcp_server.rs` or equivalent RMCP handler module
- `src/runtime.rs` only if query-only initialization needs a narrow helper

**Steps:**

- [ ] Add `src/bin/cortex-stdio.rs`.
- [ ] Initialize tracing with `.with_writer(std::io::stderr)`.
- [ ] Avoid `println!`, `print!`, and stdout-based startup banners.
- [ ] Load `RuntimeCore::load_query_only()`.
- [ ] Build the shared RMCP Syslog server handler from `runtime.service()`.
- [ ] Serve it with `rmcp::transport::stdio()`.
- [ ] Await service shutdown with `service.waiting().await?`.
- [ ] Do not start syslog ingest.
- [ ] Do not mount HTTP routes.
- [ ] Do not spawn retention/storage maintenance tasks by default.

**Expected Skeleton:**

```rust
use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use cortex::{mcp, runtime::RuntimeCore};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();

    let runtime = RuntimeCore::load_query_only()?;
    let server = mcp::rmcp_server(runtime.service(), runtime.config.mcp.clone());
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

The exact handler constructor should match the RMCP HTTP refactor. Do not create a second tool implementation for stdio.

**Verification:**

```bash
cargo build --bin cortex-stdio
cargo test --bin cortex-stdio
```

Manual inspector check:

```bash
npx @modelcontextprotocol/inspector cargo run --bin cortex-stdio
```

**Done When:**

- `cortex-stdio` runs as a stdio MCP server.
- It can initialize and list all seven tools.
- It emits no stdout text outside MCP messages.

## Task 3: Add Child-Process Stdio Integration Tests

**Files:**

- `tests/stdio_mcp.rs`
- `Cargo.toml`
- test fixtures/helpers as needed

**Steps:**

- [ ] Add an integration test that builds/spawns the stdio binary as a child process.
- [ ] Use RMCP client-side child-process transport if available.
- [ ] Set `CORTEX_DB_PATH` to a temp DB path.
- [ ] Seed data directly through shared DB/app helpers or use a temp runtime fixture.
- [ ] Call `initialize`.
- [ ] Call `tools/list` and assert all seven tool names.
- [ ] Call `get_stats`.
- [ ] Call at least one parameterized query tool.
- [ ] Capture stderr separately and assert stdout is only MCP protocol traffic.

**Verification:**

```bash
cargo test --test stdio_mcp -- --nocapture
cargo test
```

**Done When:**

- Stdio behavior is covered by automated tests.
- Tests do not depend on the HTTP server running.
- Tests do not bind syslog UDP/TCP ports.

## Task 4: Define Stdio Runtime Policy

**Files:**

- `src/runtime.rs`
- `docs/mcp/TRANSPORT.md`
- `docs/mcp/DEPLOY.md`

**Steps:**

- [ ] Document that stdio mode is query-only.
- [ ] Document that ingestion still requires the daemon process to be running somewhere.
- [ ] Decide whether stdio may run storage budget enforcement on startup. Default recommendation: no, because multiple stdio clients can be launched concurrently by MCP hosts.
- [ ] Decide whether stdio may run retention cleanup. Default recommendation: no, same concurrency reason.
- [ ] Confirm SQLite WAL read concurrency is acceptable for simultaneous daemon + stdio query process.
- [ ] Confirm storage guard write-block state is reported accurately enough for query-only stats.

**Verification:**

```bash
cargo test runtime -- --nocapture
cargo test app -- --nocapture
```

**Done When:**

- Stdio mode has an explicit lifecycle contract.
- It cannot accidentally start network listeners.
- It cannot accidentally perform background cleanup from multiple child processes.

## Task 5: Update Client Configs And Manifests

**Files:**

- `docs/mcp/CONNECT.md`
- `docs/mcp/TRANSPORT.md`
- `docs/plugin/CONFIG.md`
- `docs/plugin/PLUGINS.md`
- `server.json`
- `gemini-extension.json`
- `.mcp.json`
- `.claude-plugin/plugin.json`
- `.codex-plugin/plugin.json`
- `README.md`

**Steps:**

- [ ] Document HTTP as the default daemon transport.
- [ ] Document stdio as local query-only transport.
- [ ] Add Claude/Codex/Gemini stdio examples that point to `cortex-stdio`, not the daemon mode.
- [ ] Preserve HTTP examples for remote and Docker deployments.
- [ ] Explain when to use `mcp-remote` instead of direct stdio.
- [ ] Update `server.json` only after deciding what registry package shape is valid for dual transport.
- [ ] Update `gemini-extension.json` to call `cortex-stdio` if Gemini requires command-style stdio.
- [ ] Decide whether plugin manifests should expose both HTTP and stdio variants or keep HTTP-only default.
- [ ] Include `CORTEX_DB_PATH` in stdio setup examples.
- [ ] Do not document bearer token as required for stdio mode.

**Example Stdio Client Config:**

```json
{
  "mcpServers": {
    "cortex": {
      "command": "/path/to/cortex-stdio",
      "env": {
        "CORTEX_DB_PATH": "/data/cortex.db",
        "RUST_LOG": "warn"
      }
    }
  }
}
```

**Verification:**

```bash
rg "not stdio|does not support stdio|stdio|cortex-stdio|mcp-remote|CORTEX_DB_PATH" \
  README.md docs server.json gemini-extension.json .mcp.json .claude-plugin .codex-plugin

jq empty server.json gemini-extension.json .mcp.json .claude-plugin/plugin.json .codex-plugin/plugin.json
bash bin/check-version-sync.sh
```

**Done When:**

- Docs distinguish HTTP daemon mode, direct stdio query mode, and HTTP-to-stdio bridge mode.
- No docs tell users to launch the daemon binary as a stdio MCP server unless the command explicitly enters query-only stdio mode.

## Task 6: Packaging And Release

**Files:**

- `Justfile`
- Docker/build scripts if release packaging changes
- `.gitattributes` if binary artifact handling changes
- `CHANGELOG.md`
- version-bearing manifests

**Steps:**

- [ ] Ensure release builds include both `cortex` and `cortex-stdio`.
- [ ] Update `just build-plugin` if plugin packaging should include the stdio binary.
- [ ] Decide whether Docker image needs the stdio binary. It may be useful for local `docker exec`, but stdio MCP hosts usually launch host binaries, not long-running containers.
- [ ] Update version-bearing files according to repo policy.
- [ ] Add changelog entry describing stdio support and its query-only contract.

**Verification:**

```bash
cargo build --release --bins
just build-plugin
bash bin/check-version-sync.sh
```

**Done When:**

- Release artifacts include the intended stdio entrypoint.
- Plugin/registry manifests reference real shipped binaries.
- Version files and changelog are in sync.

## Final Verification

Run the normal suite:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
bash bin/check-version-sync.sh
```

Verify stdio manually:

```bash
cargo build --bin cortex-stdio
npx @modelcontextprotocol/inspector cargo run --bin cortex-stdio
```

Verify no stdout contamination:

```bash
RUST_LOG=info target/debug/cortex-stdio > /tmp/cortex-stdio.stdout 2> /tmp/cortex-stdio.stderr
```

Expected: stdout contains only MCP protocol responses after a client sends MCP input. Startup logs and errors go to stderr.

Verify HTTP daemon still works:

```bash
cargo build --bin cortex
cargo test mcp::routes -- --nocapture
```

## Open Decisions

- Dedicated binary `cortex-stdio` vs daemon subcommand `cortex mcp-stdio`.
- Whether stdio should ever run retention/storage-budget cleanup.
- Whether registry packaging should advertise both HTTP and stdio entries or only one.
- Whether plugin defaults should remain HTTP-first or offer stdio-first for local installs.
- Whether direct stdio should query the DB directly, or optionally proxy to a running HTTP daemon for deployments where the DB path is not local.
- Whether stdio mode should expose any extra diagnostic tool explaining that ingestion requires the daemon.
