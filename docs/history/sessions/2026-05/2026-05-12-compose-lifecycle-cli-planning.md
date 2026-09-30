# 2026-05-12 Compose Lifecycle CLI Planning Session

## Context

- Repo: `/home/jmagar/workspace/syslog-mcp`
- Branch: `main`
- HEAD: `3fd81bfbb22fb2eefab157f1ad62e48f7b0b3685`
- Saved at: `2026-05-12T10:22:16-04:00`

This session started from a Docker Compose deployment conflict:

```text
Error response from daemon: Conflict. The container name "/syslog-mcp" is already in use
```

Investigation showed the live `syslog-mcp` container was healthy and owned by a plugin data-dir Compose project, not the repo checkout:

- Compose project: `syslog-jmagar-lab`
- Compose working dir: `/home/jmagar/.claude/plugins/data/syslog-jmagar-lab`
- Data mount: `/home/jmagar/.claude/plugins/data/syslog-jmagar-lab -> /data`
- Health endpoint: `http://localhost:3100/health` returned `status: ok`

The decision was to add first-class lifecycle commands under `syslog compose ...` so agents and operators can diagnose and manage the correct Compose project instead of blindly running `docker compose` from cwd.

## Artifacts Created

- `docs/plans/2026-05-12-compose-lifecycle-cli.md`
  - Reviewed spec/architecture plan.
  - Captures design decisions, safety rules, MCP boundaries, and verification requirements.

- `docs/superpowers/plans/2026-05-12-compose-lifecycle-cli.md`
  - Execution-grade `writing-plans` implementation plan.
  - Includes task-by-task steps, file paths, code sketches, tests, and command gates.

Both paths are ignored by repo `.gitignore`; they are local planning artifacts unless force-added later.

## Key Decisions

- Build a shared `src/compose.rs` layer first; split into submodules only if the implementation grows enough to justify it.
- CLI and MCP stay thin. The shared layer owns target discovery, preflight checks, subprocess execution, redaction, and response models.
- Compose commands must not load `RuntimeCore::load_query_only()` or require a readable SQLite DB.
- First-pass CLI commands:
  - `syslog compose status`
  - `syslog compose doctor`
  - `syslog compose up`
  - `syslog compose down`
  - `syslog compose restart`
  - `syslog compose pull`
  - `syslog compose logs --tail N`
- Deferred from first pass:
  - `syslog compose config`
  - `syslog compose upgrade`
  - `syslog compose logs --follow`
  - MCP mutations
  - MCP `compose_config`
  - dedicated `syslog:ops:read` scope
- MCP first pass exposes only redacted read-only diagnostics:
  - `compose_status`
  - `compose_doctor`
- MCP must reject target overrides and return a dedicated `ComposeMcpStatus`, not raw `ComposeStatus`.

## Safety Requirements Added

- Mutating commands must run preflight checks before `up`, `down`, `restart`, or `pull`.
- `--project-name` alone is not a safe mutation target.
- Cwd fallback requires `--allow-cwd-target`.
- Owner/image mismatch requires `--allow-foreign-project`.
- `down` requires a confirmed Compose target and `--yes` in non-interactive mode.
- First pass checks only `syslog-mcp.service` plus listeners on `1514` and `3100`.
- `up` and `restart` refuse when systemd or a non-target listener owns the ports.
- `pull` warns on systemd/listener conflicts but does not refuse because it does not mutate a running process.
- Compose invocation must preserve project semantics:
  - use resolved project directory as `current_dir` or `--project-directory`
  - include all compose files in label order
  - use detached `docker compose up -d SERVICE`
- Subprocess runner must:
  - enforce timeouts
  - drain stdout/stderr concurrently
  - cap output and continue discarding after cap
  - redact sensitive output
  - terminate/kill/reap process trees on timeout when practical
  - report timeout cleanup status

## Review History

The plan went through two engineering review passes with architecture, simplicity, security, and performance lanes.

First review led to:

- Removing MCP `compose_config` from first pass.
- Deferring `upgrade`.
- Deferring or isolating `logs --follow`.
- Adding mutation safety rules.
- Adding systemd/listener conflict checks.
- Adding output bounds and redaction.

Second review led to:

- Explicit detached `up -d` mapping.
- Process-tree timeout cleanup requirements.
- Concurrent stdout/stderr draining requirements.
- Compose cwd/project-directory semantics.
- Rejection of `--project-name` alone for mutation.
- MCP target override rejection.
- Dedicated MCP-safe DTO.
- Deferring user-facing `config`.

## Verification Performed

No code implementation tests were run because this session only created planning artifacts.

Commands run during planning/review included:

```bash
docker ps -a --filter name='^/syslog-mcp$'
docker compose ps -a
docker inspect syslog-mcp
docker logs --tail 80 syslog-mcp
curl -fsS http://localhost:3100/health
git log --oneline -20
git status --short --branch --ignored
```

The live service was healthy during the initial investigation.

## Current Repo State

At save time:

```text
## main...origin/main
!! docs/plans/
!! docs/sessions/
!! docs/superpowers/plans/2026-05-12-compose-lifecycle-cli.md
```

No tracked code files were modified by this planning session.

## Open Questions

- Should the default container name eventually be configurable through an env var, or remain hard-coded as `syslog-mcp` with CLI overrides?
- Should `compose logs --follow` remain deferred, or be implemented as CLI-only streaming after bounded logs lands?
- Should a dedicated MCP operational read scope be introduced after the first pass, and what exact scope name should it use?
- Should the execution plan be force-added later, or remain local under ignored docs paths?

## Next Step Options

1. Execute the implementation plan task-by-task using subagent-driven development.
2. Execute inline with checkpoints.
3. Convert the plan into Beads issues before implementation.
4. Keep the artifacts as local planning references only.
