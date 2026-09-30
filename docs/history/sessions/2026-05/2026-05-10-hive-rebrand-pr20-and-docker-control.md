# 2026-05-10 - Hive Rebrand PR #20 and Docker Control Direction

## Repo State

- Repo: `/home/jmagar/workspace/syslog-mcp`
- Main worktree branch: `main`
- Main worktree status at save time: clean, `main...origin/main`
- Active feature worktree: `/home/jmagar/workspace/syslog-mcp/.worktrees/hive-rebrand`
- Feature branch: `hive-rebrand`
- Latest feature commit: `0d883c47554811bdc690ec0c212b847384046cec`
- PR: <https://github.com/jmagar/syslog-mcp/pull/20>
- PR title: `feat!: rebrand syslog-mcp to Hive`
- PR state at save time: open, merge state `CLEAN`

## What Changed In This Session

- Merged the planning branch back into `main`.
- Created and worked in `.worktrees/hive-rebrand`.
- Implemented the Hive / `hive-mcp` rebrand flow.
- Created PR #20.
- Ran the requested review flow:
  - `lavra-review`
  - `pr-review-toolkit:full-review`
  - `code_simplifier` agent review
  - `gh-address-comments`
- Addressed all PR review comments.
- Closed all PR-thread beads created by `gh-address-comments`.
- Closed Hive rebrand final verification bead `syslog-mcp-s4jl.6`.
- Closed Hive rebrand epic `syslog-mcp-s4jl`.

## Review Follow-up Commit

Final review-response commit:

```text
0d883c4 fix: address Hive PR review comments
```

This commit:

- Moved shared binary entrypoint logic from `src/main.rs` into the library crate and made the legacy `syslog` binary call `hive_mcp::entry()`.
- Kept current GitHub repository URLs on `jmagar/syslog-mcp` until the actual GitHub repo rename lands.
- Narrowed `scripts/bump-version.sh` image-tag replacement to the `server.json` Hive OCI identifier instead of using a broad `:${CURRENT}"` substitution.
- Added Hive-first env override precedence for `scripts/check-runtime-current.sh`.
- Moved the Docker CLI guard before the first Docker invocation in `scripts/verify-compose-upgrade.sh`.
- Cleaned the `.claude-plugin/plugin.json` `no_auth` description.

## Verification Evidence

Local verification passed:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- `cargo audit --json --file ./Cargo.lock`
- `bash scripts/check-version-sync.sh`
- `bash scripts/validate-marketplace.sh`
- `bash scripts/verify-compose-upgrade.sh`
- `git diff --check`

Remote PR checks passed:

- Build and Push Docker Image / `build-and-push`
- CI / Formatting
- Codex Plugin Quality Gate / `scan`
- CI / Clippy
- CI / Tests
- CI / Security Audit
- CI / Secret Scan
- CI / MCP Integration Tests
- CodeRabbit
- GitGuardian Security Checks

`gh-address-comments` verification:

- 14 review threads resolved or outdated.
- 0 open review threads.
- Pre-merge checklist passed with `--require-approvals 0`.

Beads:

- No open beads with `hive` or `pr-review` labels at save time.
- `bd dolt push` was attempted and skipped because no Dolt remote is configured.

## Current Docker/Bollard Discussion

Confirmed current repo dependency:

- `Cargo.toml` has `bollard = { version = "0.19", default-features = false, features = [...] }`
- `Cargo.lock` contains `bollard`

Current usage:

- Docker ingest uses Bollard for read-oriented Docker operations.
- It lists containers, streams logs, watches Docker events, and tracks checkpoints.
- The current documented deployment model is intentionally through `docker-socket-proxy` with read-oriented permissions like `CONTAINERS=1`, `EVENTS=1`, `PING=1`, `VERSION=1`, and `POST=0`.

Potential next direction:

- Add Docker management/control as a separate opt-in capability, not as part of default log ingest.
- Good phase 1: read-only Docker inventory/control visibility, such as hosts, containers, inspect, and events.
- Good phase 2: controlled lifecycle actions for existing containers, such as start, stop, restart, pause, unpause, and remove.
- Higher-risk phase 3: Compose/stack operations through a constrained host-side runner or SSH command allowlist, because Compose is a CLI/plugin layer rather than a clean Docker Engine API abstraction.
- Highest-risk phase 4: build/deploy, because Docker API build requires shipping a build context tar to the daemon and can easily become host-root-equivalent.

## Open Questions

- Should Docker control live in Hive itself, or should Hive call into a separate homelab control-plane service for write/admin operations?
- What exact MCP scope model should gate write actions? Candidate: keep `hive:read` for observability and require `hive:admin` or a new `hive:docker:write` for Docker lifecycle actions.
- Should Docker write access require a separate config block from `docker_ingest`, so log ingest can remain read-only and low-risk?
- Should remote Docker control use `docker-socket-proxy` with explicit write endpoint flags, SSH to known hosts, or a small per-host agent?
- For stacks, should the contract be "known compose projects only" with configured project directories, rather than arbitrary `docker compose` commands?

## Next Suggested Step

Create a planning bead for Docker control with an explicit security model:

- Inventory current Bollard wrapper boundaries in `src/docker_ingest/client.rs`.
- Define a new Docker control config separate from `docker_ingest`.
- Decide read-only inventory actions first.
- Gate lifecycle actions behind admin scope and per-host/per-action allowlists.
- Add audit logging for every attempted write operation.
