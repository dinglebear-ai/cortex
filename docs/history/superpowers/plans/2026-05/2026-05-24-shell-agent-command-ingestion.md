# Shell and Agent Command Ingestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture zsh history and Claude Code command executions as searchable, scrubbed log rows that participate in existing syslog-mcp correlation.

**Architecture:** Add two first-class ingest source kinds: `shell-history` for passive shell history backfill and `agent-command` for Claude Code shell-prefix command execution. Both normalize into `LogBatchEntry`; the Claude wrapper writes append-only JSONL spool records and never touches SQLite on the command hot path.

**Tech Stack:** Rust, rusqlite, serde JSONL, chrono, existing `LogBatchEntry`, `SyslogService::run_db`, `SourceKind`, and CLI dispatch modules.

---

## Findings Applied

- `source_kind` is a closed contract in `docs/contracts/source-kinds.md`; new sources must update the enum, docs, dispatch mapping, and tests together.
- A broad `LogSource` trait would be premature. The useful abstraction is a small mapper from source-specific records into `LogBatchEntry`.
- `CLAUDE_CODE_SHELL_PREFIX` is the intended Claude Code hook point for Bash tool calls, hook commands, and stdio MCP startup commands. The wrapper must preserve stdout, stderr, stdin, and exit status.
- The Claude wrapper must not write SQLite directly. It appends JSONL to a user-private spool path and a separate ingest command imports that spool.
- Command text is sensitive. Reuse the existing AI scrubber and metadata sanitizer, do not capture stdout/stderr by default, and document that command-read access implies access to scrubbed local operator activity.

## Files

- Modify `src/enrich/parser.rs`: add `ShellHistory` and `AgentCommand` source kinds.
- Modify `docs/contracts/source-kinds.md`: register `shell-history` and `agent-command` plus URI schemes.
- Create `src/command_log.rs`: zsh parser, agent spool JSONL model, command wrapper, import logic, and tests.
- Modify `src/lib.rs`: export `command_log`.
- Modify `src/app/service.rs`: add local DB methods for shell history and agent spool import.
- Modify `src/cli/args.rs`: add `ShellCommand`, `AgentCommandCommand`, and argument structs.
- Modify `src/cli/parse.rs`: route `shell` and `agent-command`.
- Create `src/cli/parse_command_log.rs`: parse the new CLI surfaces.
- Create `src/cli/dispatch_command_log.rs`: call service methods and run wrapper.
- Modify `src/cli/run.rs`: dispatch the new commands.
- Modify `src/cli.rs`: include the new modules.
- Modify `src/main.rs`: usage text.
- Modify `README.md` and `docs/CLI.md`: document setup, backfill, wrapper, and caveats.

## Task 1: Source Kinds and Row Mapping

- [ ] Add `ShellHistory` and `AgentCommand` to `SourceKind`.
- [ ] Update `as_str()` and dispatch string mapping.
- [ ] Add parser tests asserting kebab-case serialization for both variants.
- [ ] Update `docs/contracts/source-kinds.md` with `shell-history://<hostname>/<user>/<shell>` and `agent-command://<hostname>/<agent>/<session>`.

## Task 2: Command Log Module

- [ ] Create `src/command_log.rs` with `CommandLogImportResult`.
- [ ] Implement `parse_zsh_extended_history_line(": 1716500000:3;cargo test")`.
- [ ] Skip timestamp-less history rows by default and count them as skipped.
- [ ] Map zsh rows to `LogBatchEntry` with `facility=shell`, `app_name=zsh`, scrubbed command text, and metadata including `timestamp_quality`, `duration_secs`, `history_path`, `line_no`, and `content_scrubbed`.
- [ ] Implement `AgentCommandSpoolRecord` JSONL parsing and mapping to `LogBatchEntry`.
- [ ] Implement `run_agent_command_wrapper(spool_path, command_args)` that appends one JSONL record after the command exits, inherits stdin/stdout/stderr, and exits with the original status.
- [ ] Add unit tests for zsh parsing, skip counts, command scrubbing, metadata source kinds, and failed-command severity.

## Task 3: Service and CLI Integration

- [ ] Add service methods `import_shell_history(path, shell)` and `import_agent_command_spool(path)`.
- [ ] Add CLI commands:
  - `syslog shell index --path PATH [--shell zsh] [--json]`
  - `syslog agent-command ingest-spool --path PATH [--json]`
  - `syslog agent-command wrap --spool PATH -- COMMAND...`
- [ ] Keep `shell index` and `agent-command ingest-spool` local-only through `SyslogService`; the wrapper must not require DB config.
- [ ] Add parser tests for all new CLI commands.
- [ ] Add dispatch tests for HTTP-mode rejection if practical.

## Task 4: Documentation

- [ ] Document zsh backfill and the requirement for `EXTENDED_HISTORY` for reliable timestamps.
- [ ] Document Claude setup using `CLAUDE_CODE_SHELL_PREFIX`.
- [ ] Document spool privacy and permissions: create parent directories with `0700`; append records with user-readable permissions only.
- [ ] Document no stdout/stderr capture and no direct DB writes from wrapper.

## Task 5: Verification and Release Hygiene

- [ ] Run focused tests for `command_log`, `enrich::parser`, and CLI parsing.
- [ ] Run `cargo fmt`.
- [ ] Run `cargo test`.
- [ ] Verify the commit prefix, then run the normal bump tooling using the repo policy (`feat!:`/`BREAKING CHANGE` -> major, `feat:` -> minor, everything else -> patch) and add a `CHANGELOG.md` entry.
- [ ] Commit and push branch, then create PR if GitHub auth/remotes are available.
