# MCP Server Documentation -- cortex

Follow the root [AGENTS.md](../../AGENTS.md). This file is the canonical scoped index for the `mcp/` documentation subdirectory. These docs cover the MCP server implementation, transport, tools, testing, and deployment.

## File index

| File | Purpose |
| --- | --- |
| `AUTH.md` | Authentication: bearer token, constant-time comparison, unauthenticated endpoints |
| `CICD.md` | Current CI, release-please, native/npm/container and registry workflows |
| `CONNECT.md` | Client connection methods: plugin install, Claude Code, Codex, Gemini, curl |
| `DEPLOY.md` | Deployment: local dev, Docker, Docker Compose, port assignment |
| `DEV.md` | Development workflow: build cycle, adding tools, debugging, code style |
| `CORRELATION.md` | Correlation behavior: time-window, AI anchor, historical incident, and context actions |
| `ELICITATION.md` | MCP elicitation: cortex does not use elicitation; admin writes use scope gates |
| `ENV.md` | Environment variable reference (concise cross-ref to CONFIG.md) |
| `LOGS.md` | Logging: RUST_LOG, tracing, structured output, error handling |
| `MCPORTER.md` | Live smoke testing with mcporter |
| `MCPUI.md` | MCP UI patterns: schema annotations for client rendering |
| `PATTERNS.md` | Code patterns: flat tool dispatch, run_db helper, batch writer |
| `PRE-COMMIT.md` | Pre-commit hook configuration |
| `PUBLISH.md` | Publishing: release-please, native/npm, GHCR, MCP registry |
| `RESOURCES.md` | MCP resources: schema resource and URI conventions |
| `SCHEMA.md` | Tool schema documentation: JSON Schema definitions in Rust |
| `TESTS.md` | Testing: nextest, doctests, live qualification, coverage |
| `TOOLS.md` | MCP tools reference: one `cortex` tool with action shapes from `ACTION_SPECS` |
| `TRANSPORT.md` | RMCP Streamable HTTP transport, stateless mode, port assignment |
| `WEBMCP.md` | Web MCP: CORS configuration, browser access restrictions |
