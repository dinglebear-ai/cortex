# 2026-05-23 CLI Remote Deploy

## Summary

Implemented `syslog deploy remote <host>` as a CLI-only SSH deployment flow for the Docker Compose server bundle.

REST and MCP deploy mutation handlers remain intentionally out of scope. Existing REST/MCP Compose surfaces are read-only diagnostics only.

## Branch

- Branch: `feat/cli-remote-deploy`
- Bead: `syslog-mcp-i0q6`

## Changes

- Added `src/deploy.rs` with OpenSSH-based remote orchestration, dry-run support, phase reporting, and testable runner abstraction.
- Extended top-level deploy parsing and usage for `syslog deploy remote HOST [--dry-run] [--json]`.
- Reused setup Compose/env assets instead of adding another deploy model.
- Documented remote deploy behavior and `.env` overwrite semantics in CLI and MCP deployment docs.
- Bumped release metadata to `0.28.2`.

## Verification

- `cargo fmt --check` passed.
- `cargo test deploy` passed.
- `cargo clippy -- -D warnings` passed.
- `cargo test` passed.
- `bash scripts/check-version-sync.sh --require-changelog` passed.
- `bash scripts/validate-marketplace.sh` passed.
- `just check` still fails on the pre-existing module-size guard:
  - `src/cli/args.rs` has 535 lines.
  - `src/cli/dispatch_surface.rs` has 502 lines.
