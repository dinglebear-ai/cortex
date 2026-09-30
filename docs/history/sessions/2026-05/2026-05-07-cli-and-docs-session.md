# 2026-05-07 CLI and Docs Session

## Summary

This session implemented and documented a direct `syslog` CLI intended to
accompany the MCP server. The intended design was to keep the MCP layer thin and
route CLI commands through the existing shared business/service layer rather
than duplicating query behavior.

The CLI commands added during the implementation pass were:

- `syslog search`
- `syslog tail`
- `syslog errors`
- `syslog hosts`
- `syslog correlate`
- `syslog stats`

Each command supported compact human-readable output by default and `--json` for
the serialized service response shape. The implementation loaded the query-only
runtime and called the existing `SyslogService` methods used by MCP actions.

## Implementation Work Completed

Files changed during the CLI implementation pass:

- `src/cli.rs` -- new CLI parser, command model, service adapter, and output formatting.
- `src/cli_tests.rs` -- parser coverage for search, tail, correlate, and invalid options.
- `src/main.rs` -- routed direct CLI commands alongside existing `serve mcp` and `mcp` modes.
- `src/main_tests.rs` -- mode parser coverage for CLI routing.
- `README.md` -- quick direct CLI examples under command modes.

Important implementation details:

- `syslog serve mcp` remained the default daemon mode.
- `syslog mcp` remained query-only MCP stdio mode.
- Direct commands used `RuntimeCore::load_query_only()` and `runtime.service()`.
- CLI logging defaulted to `warn`, matching query-only behavior and keeping stdout clean for command output.
- No additional CLI dependency such as `clap` was introduced; parsing stayed small and local.

## Documentation Work Completed

Docs were expanded after the implementation pass:

- `docs/CLI.md` -- new full direct CLI reference, including config behavior, output mode, commands, flags, examples, and CLI-to-MCP action mapping.
- `docs/README.md` -- added `CLI.md` to the docs index.
- `README.md` -- added quick examples and a link to `docs/CLI.md`.
- `docs/INVENTORY.md` -- corrected stale inventory language from a flat multi-tool MCP pattern to the current single `syslog` action-dispatch MCP tool, and added direct CLI command inventory.
- `docs/mcp/TRANSPORT.md` -- distinguished direct CLI from MCP HTTP and MCP stdio transports.
- `docs/mcp/ENV.md` -- clarified that direct CLI uses `SYSLOG_MCP_DB_PATH` and does not use `SYSLOG_MCP_TOKEN`.
- `docs/mcp/TOOLS.md` -- linked to the direct CLI reference.
- `docs/plugin/CONFIG.md` -- clarified that direct CLI commands are useful for host-local diagnostics but are not plugin connection modes.

## Verification Evidence

After the implementation pass, these commands passed:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

The passing test run reported:

- `193` library unit tests passed.
- `7` binary tests passed, including new CLI parser tests.
- `3` `tests/rmcp_compat.rs` tests passed.
- `1` `tests/stdio_mcp.rs` test passed.
- doc tests passed with `0` doctests.

No tests were run after the docs-only update.

## Current Checkout State at Save Time

At the time this note was saved, `git status --short --branch` reported:

```text
## main...origin/main
 M deploy/README.md
 M deploy/rsyslog/30-swag.conf
 M deploy/rsyslog/35-authelia.conf
 M deploy/rsyslog/36-adguard.conf
 M deploy/rsyslog/40-ai-transcripts.conf
 M plugins/commands/deploy-dropins.md
?? deploy/apparmor/
?? deploy/rsyslog/11-imfile.conf
```

Notably, the status at save time no longer showed the earlier CLI/docs files
from this chat (`src/cli.rs`, `src/cli_tests.rs`, `README.md`, `docs/CLI.md`,
etc.). That means the checkout state changed between the CLI/docs work and this
session-save request, or those changes were otherwise no longer present in the
visible worktree when this note was written.

## Existing Dirty Worktree Caveat

Earlier in the CLI/docs work, unrelated dirty files were observed and left
untouched, including plugin/setup files and later broader source/runtime files.
At save time, the visible dirty set had changed to deploy/rsyslog-related files
listed above.

Do not assume the CLI/docs edits are currently present without checking the
working tree or git history.

## Open Questions

- Whether the CLI/docs implementation was committed, moved to another checkout,
  reverted, or replaced by the later deploy/rsyslog worktree state.
- Whether `docs/CLI.md` and the CLI source files should be restored/reapplied in
  the current checkout before the next commit.
- Whether this session note should be committed. `docs/sessions/` is ignored by
  `.gitignore`, so this file requires `git add -f` if it should be included in a
  commit.
