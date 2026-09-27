---
title: "Technology choices"
created: 2026-04-04
updated: 2026-09-27
---

# Technology choices

The current versions and feature selections live in [Cargo.toml](../../Cargo.toml), [Cargo.lock](../../Cargo.lock), and [web/package.json](../../web/package.json). This page explains the boundaries, not a second dependency inventory.

## Service implementation

Rust supplies the shared library and `cortex` binary. Tokio handles asynchronous listeners, channels, cancellation, and task supervision. Its features are explicitly selected in Cargo rather than enabled with `full`; blocking database work uses the established blocking/service helpers. Memory safety is not a guarantee against overload or dropped UDP input.

Axum and Tower compose the HTTP surfaces. RMCP provides the MCP adapter and transport; the workspace pins `rmcp = "=3.1.0"`. The HTTP MCP service uses stateless JSON responses. Application configuration and `lab-auth` implement authentication policies; do not describe all routes as one constant-time static-token check.

## Evidence storage

Rusqlite uses bundled SQLite, with pooled connections and WAL-backed storage. FTS5 indexes support log/session text search; typed tables retain metrics, traces, heartbeats, receipts, and projections. SQLite avoids an external database service, but WAL means copying only the main file during active writes is not a backup. Use online backup or coordinate every writer.

The shared `CortexService` boundary keeps query validation and limits consistent across adapters. Graph and Observatory projections derive from evidence; they must retain provenance rather than replacing the evidence store.

## Parsing and observability

`syslog_loose` handles lenient syslog parsing. Serde, serde_json, TOML, and the OTLP/protobuf types handle their respective formats. Provider-local transcript parsers are described by `src/scanner/providers.rs`, including unsupported/partial evidence lanes. Chrono handles time values; structured `tracing` and configured filters support diagnostics. Typed errors and `anyhow` serve different layers of error handling.

## Browser and packaging

`web/` is the browser investigation workspace, with Next.js/React and its own pinned package manifest and tests. Rust serves its static export through `src/web_app.rs`. Platform-specific release artifacts, the npm launcher, container image, MCP bundles, and client skills are separate packaging surfaces, not proof of a universally static binary.

See [architecture.md](../architecture.md), [RUST.md](../RUST.md), and [PUBLISH.md](../mcp/PUBLISH.md).
