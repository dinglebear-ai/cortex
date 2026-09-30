# 2026-05-23 MCPB Package

## Summary

Added a Linux MCP Bundle packaging path for the existing `syslog mcp` stdio server.

The bundle is query-only. It launches the bundled release binary as `server/syslog mcp`, reads the configured local SQLite database, and does not start syslog listeners, HTTP MCP, REST, Docker Compose, or deploy flows.

## Branch

- Branch: `feat/mcpb-package`
- Worktree: `/home/jmagar/workspace/syslog-mcp/.worktrees/mcpb-package`

## Artifact

- Generated artifact: `dist/syslog-mcp-0.28.2-linux.mcpb`
- Archive contents verified with `unzip -l`:
  - `manifest.json`
  - `server/syslog`
- `npx --yes @anthropic-ai/mcpb info dist/syslog-mcp-0.28.2-linux.mcpb` reports the bundle is unsigned.

## Changes

- Added `mcpb/manifest.json` using MCPB manifest v0.4 and `server.type = "binary"`.
- Added `scripts/build-mcpb.sh` to build release, stage the bundle, validate, pack, and inspect the MCPB.
- Added `just build-mcpb`.
- Added `mcpb/manifest.json` to version-sync checks.
- Documented MCPB usage in `docs/mcp/CONNECT.md` and release packaging in `docs/mcp/PUBLISH.md`.
- Bumped release metadata to `0.28.2`.

## Verification

- `npx --yes @anthropic-ai/mcpb validate mcpb/manifest.json` passed.
- `scripts/build-mcpb.sh` passed.
- `just build-mcpb` passed.
- `unzip -l dist/syslog-mcp-0.28.2-linux.mcpb` showed only manifest and binary.
- `cargo check` passed.
- `cargo fmt --check` passed.
- `cargo test stdio` passed.
- `cargo clippy -- -D warnings` passed.
- `cargo test` passed.
- `bash scripts/check-version-sync.sh --require-changelog` passed.
- `bash scripts/validate-marketplace.sh` passed.
- `just check` still fails on the pre-existing module-size guard:
  - `src/cli/args.rs` has 535 lines.
  - `src/cli/dispatch_surface.rs` has 502 lines.
